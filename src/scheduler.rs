use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, error, info, warn};
use url::Url;

use crate::notifier::{DiscordMessage, Notifier};
use crate::runtime::{RunConfig, RunStatusPage};
use crate::status::{Fetched, PageState, StatusSource, build_message, diff};

/// How often the status poller reports that it is alive.
///
/// Why this exists at all: a healthy page answers 304 and that path only logs at
/// `debug!`, so at `log_level = "info"` a fully working poller is indistinguishable
/// from a dead one for as long as no incident moves — days, on quiet status pages.
/// Why not a config knob: the interval only has to be shorter than an operator's
/// patience, and `log_level = "debug"` already exposes per-poll detail when a real
/// investigation needs it.
const STATUS_SUMMARY_INTERVAL: Duration = Duration::from_secs(3600);

/// What one poll did, folded into the summary counters by [`PollStats::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollOutcome {
    NotModified,
    Updated { forwarded: u64 },
    Failed,
}

/// Counters for one summary window. Reset when the window closes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PollStats {
    polls: u64,
    not_modified: u64,
    updated: u64,
    failed: u64,
    forwarded: u64,
}

impl PollStats {
    fn record(&mut self, outcome: PollOutcome) {
        self.polls += 1;
        match outcome {
            PollOutcome::NotModified => self.not_modified += 1,
            PollOutcome::Updated { forwarded } => {
                self.updated += 1;
                self.forwarded += forwarded;
            }
            PollOutcome::Failed => self.failed += 1,
        }
    }
}

pub struct Scheduler<N: Notifier, S: StatusSource> {
    cfg: RunConfig,
    notifier: N,
    source: S,
    last_fired: HashMap<String, DateTime<Tz>>,
    last_polled: HashMap<String, DateTime<Tz>>,
    page_states: HashMap<String, PageState>,
    stats: PollStats,
    /// `None` until the first tick: the window is anchored to a real tick rather
    /// than to construction, so a scheduler built long before it runs does not
    /// report an oversized first window.
    summary_window_start: Option<DateTime<Tz>>,
}

impl<N: Notifier, S: StatusSource> Scheduler<N, S> {
    pub fn new(cfg: RunConfig, notifier: N, source: S) -> Self {
        Scheduler {
            cfg,
            notifier,
            source,
            last_fired: HashMap::new(),
            last_polled: HashMap::new(),
            page_states: HashMap::new(),
            stats: PollStats::default(),
            summary_window_start: None,
        }
    }

    pub async fn run(mut self) -> std::io::Result<()> {
        let mut ticker = interval(self.cfg.interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let now = Utc::now().with_timezone(&self.cfg.timezone);
                    self.tick(now).await;
                }
                _ = sigint.recv() => {
                    info!("received SIGINT, shutting down");
                    return Ok(());
                }
                _ = sigterm.recv() => {
                    info!("received SIGTERM, shutting down");
                    return Ok(());
                }
            }
        }
    }

    async fn tick(&mut self, now: DateTime<Tz>) {
        self.write_heartbeat();
        self.fire_reminders(now).await;
        self.poll_status_pages(now).await;
        self.emit_status_summary(now);
    }

    async fn fire_reminders(&mut self, now: DateTime<Tz>) {
        let current_minute = truncate_to_minute(&now);
        let mut to_fire: Vec<(String, Url, String)> = Vec::new();
        for r in &self.cfg.reminders {
            if !r.fires_at(&now) {
                continue;
            }
            let already_fired = self
                .last_fired
                .get(&r.name)
                .is_some_and(|t| *t == current_minute);
            if already_fired {
                continue;
            }
            self.last_fired.insert(r.name.clone(), current_minute);
            to_fire.push((r.name.clone(), r.webhook_url.clone(), r.message.clone()));
        }
        for (name, url, message) in to_fire {
            let payload = DiscordMessage::text(&message);
            match self.notifier.send(&url, &payload).await {
                Ok(()) => info!(reminder = %name, "reminder fired"),
                Err(e) => error!(reminder = %name, error = %e, "failed to send reminder"),
            }
        }
    }

    /// Poll at most one status page per tick.
    ///
    /// The heartbeat is written once, at the top of the tick, and `chime health`
    /// calls it stale past `2 * tick_interval`. Polling every due page in one tick
    /// would let N pages behind a network partition hold the tick for
    /// `N * FETCH_TIMEOUT` and get the container restarted over someone else's
    /// outage. One page per tick bounds that to a single fetch however many pages
    /// are configured, and picking the most overdue one keeps them from staying in
    /// the lockstep they start in — every page is due on the very first tick.
    async fn poll_status_pages(&mut self, now: DateTime<Tz>) {
        let Some(index) = self.most_overdue(now) else {
            return;
        };
        // Borrows are split by field rather than cloning the page: `poll_page` is a
        // free function so `cfg`, `page_states`, `source` and `notifier` can be held
        // at once.
        let page = &self.cfg.status_pages[index];
        // Recorded before the request: a slow or failing page must wait out its own
        // interval, not be retried on every tick.
        self.last_polled.insert(page.name.clone(), now);
        let state = self.page_states.entry(page.name.clone()).or_default();
        let outcome = poll_page(&self.source, &self.notifier, page, state).await;
        self.stats.record(outcome);
    }

    /// Log the closed summary window, if one is due.
    fn emit_status_summary(&mut self, now: DateTime<Tz>) {
        let Some((window_sec, stats)) = self.take_due_summary(now) else {
            return;
        };
        info!(
            window_sec,
            pages = self.cfg.status_pages.len(),
            polls = stats.polls,
            not_modified = stats.not_modified,
            updated = stats.updated,
            failed = stats.failed,
            forwarded = stats.forwarded,
            "status poll summary"
        );
    }

    /// Close the summary window and hand back its length and counters, or `None`
    /// while the window is still open. Split from the logging so the window
    /// arithmetic is testable without a tracing subscriber.
    fn take_due_summary(&mut self, now: DateTime<Tz>) -> Option<(i64, PollStats)> {
        // A reminder-only deployment has no poller to prove alive; staying silent
        // keeps this out of logs that would never contain a status line anyway.
        if self.cfg.status_pages.is_empty() {
            return None;
        }
        let Some(start) = self.summary_window_start else {
            self.summary_window_start = Some(now);
            return None;
        };
        let elapsed = now.signed_duration_since(start).num_seconds();
        // A clock stepping backwards must not park the summary until it catches up.
        if (0..STATUS_SUMMARY_INTERVAL.as_secs() as i64).contains(&elapsed) {
            return None;
        }
        self.summary_window_start = Some(now);
        Some((elapsed.max(0), std::mem::take(&mut self.stats)))
    }

    fn most_overdue(&self, now: DateTime<Tz>) -> Option<usize> {
        self.cfg
            .status_pages
            .iter()
            .enumerate()
            .filter(|(_, p)| is_due(self.last_polled.get(&p.name), p.poll_interval, now))
            .min_by_key(|(i, p)| {
                // Longest overdue first; declaration order settles a tie, so the
                // choice is deterministic rather than hash-order dependent.
                (
                    Reverse(overdue_secs(self.last_polled.get(&p.name), now)),
                    *i,
                )
            })
            .map(|(i, _)| i)
    }

    /// Write the liveness heartbeat. Called at the start of every tick, before any
    /// network send, so the signal is independent of Discord reachability. A write
    /// failure is logged and ignored: a persistent failure ages the mtime and the
    /// `health` subcommand fails on its own, which is the detection path we want.
    fn write_heartbeat(&self) {
        let body = Utc::now().to_rfc3339();
        if let Err(e) = std::fs::write(&self.cfg.heartbeat_path, body) {
            warn!(
                path = %self.cfg.heartbeat_path.display(),
                error = %e,
                "failed to write heartbeat"
            );
        }
    }
}

async fn poll_page<N: Notifier, S: StatusSource>(
    source: &S,
    notifier: &N,
    page: &RunStatusPage,
    state: &mut PageState,
) -> PollOutcome {
    let etag = state.etag.clone();
    let fetched = match source.fetch(&page.api_url, etag.as_deref()).await {
        Ok(f) => f,
        Err(e) => {
            // A status page being unreachable is not chime's outage to report:
            // log it and try again next interval, never notify Discord.
            warn!(status_page = %page.name, error = %e, "failed to poll status page");
            return PollOutcome::Failed;
        }
    };
    let (incidents, new_etag) = match fetched {
        Fetched::NotModified => {
            debug!(status_page = %page.name, "status page not modified");
            return PollOutcome::NotModified;
        }
        Fetched::Modified { incidents, etag } => (incidents, etag),
    };

    state.etag = new_etag;
    let events = diff(state, &incidents, page.min_impact);
    let mut forwarded = 0;
    for event in events {
        let message = build_message(page, &event);
        match notifier.send(&page.webhook_url, &message).await {
            Ok(()) => {
                forwarded += 1;
                info!(
                    status_page = %page.name,
                    incident = %event.incident_id,
                    state = event.state.label(),
                    "status update forwarded"
                )
            }
            Err(e) => error!(
                status_page = %page.name,
                incident = %event.incident_id,
                error = %e,
                "failed to forward status update"
            ),
        }
    }
    PollOutcome::Updated { forwarded }
}

fn is_due(last: Option<&DateTime<Tz>>, poll_interval: Duration, now: DateTime<Tz>) -> bool {
    match last {
        None => true,
        Some(previous) => {
            let elapsed = now.signed_duration_since(*previous).num_seconds();
            // A clock stepping backwards must not park a page until it catches up.
            elapsed < 0 || elapsed >= poll_interval.as_secs() as i64
        }
    }
}

/// A page that has never been polled outranks every page that has.
fn overdue_secs(last: Option<&DateTime<Tz>>, now: DateTime<Tz>) -> i64 {
    match last {
        None => i64::MAX,
        Some(previous) => now.signed_duration_since(*previous).num_seconds(),
    }
}

fn truncate_to_minute(t: &DateTime<Tz>) -> DateTime<Tz> {
    t.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(*t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Impact, LogLevel};
    use crate::notifier::NotifyError;
    use crate::runtime::{RunReminder, mk_run_status_page};
    use crate::status::{Incident, StatusError, mk_incident, mk_update};
    use chrono::TimeZone;
    use chrono_tz::Asia::Tokyo;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct CountingNotifier {
        count: Arc<AtomicUsize>,
        messages: Arc<Mutex<Vec<DiscordMessage>>>,
    }

    impl CountingNotifier {
        fn new() -> Self {
            CountingNotifier {
                count: Arc::new(AtomicUsize::new(0)),
                messages: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn sent(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    impl Notifier for CountingNotifier {
        async fn send(&self, _webhook: &Url, message: &DiscordMessage) -> Result<(), NotifyError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.messages.lock().unwrap().push(DiscordMessage {
                content: message.content.clone(),
                username: message.username.clone(),
                avatar_url: message.avatar_url.clone(),
                embeds: Vec::new(),
            });
            Ok(())
        }
    }

    /// Returns the queued incident lists in order, repeating the last one once the
    /// queue drains. `fail` makes every fetch error instead, `not_modified` makes
    /// every fetch answer 304.
    #[derive(Clone)]
    struct FakeSource {
        calls: Arc<AtomicUsize>,
        queue: Arc<Mutex<VecDeque<Vec<Incident>>>>,
        fail: bool,
        not_modified: bool,
    }

    impl FakeSource {
        fn empty() -> Self {
            FakeSource {
                calls: Arc::new(AtomicUsize::new(0)),
                queue: Arc::new(Mutex::new(VecDeque::new())),
                fail: false,
                not_modified: false,
            }
        }

        fn with(responses: Vec<Vec<Incident>>) -> Self {
            FakeSource {
                queue: Arc::new(Mutex::new(responses.into())),
                ..FakeSource::empty()
            }
        }

        fn failing() -> Self {
            FakeSource {
                fail: true,
                ..FakeSource::empty()
            }
        }

        fn unchanged() -> Self {
            FakeSource {
                not_modified: true,
                ..FakeSource::empty()
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl StatusSource for FakeSource {
        async fn fetch(&self, _url: &Url, _etag: Option<&str>) -> Result<Fetched, StatusError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(StatusError::Status {
                    status: 503,
                    body: "unavailable".to_string(),
                });
            }
            if self.not_modified {
                return Ok(Fetched::NotModified);
            }
            let mut queue = self.queue.lock().unwrap();
            let incidents = if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue.front().cloned().unwrap_or_default()
            };
            Ok(Fetched::Modified {
                incidents,
                etag: None,
            })
        }
    }

    fn mk_run_reminder(name: &str, hour: u32, minute: u32) -> RunReminder {
        use crate::config::{Schedule, TimeOfDay, WeekdaySet};
        RunReminder {
            name: name.to_string(),
            time: TimeOfDay { hour, minute },
            schedule: Schedule::Weekly(WeekdaySet::try_from(vec!["every".to_string()]).unwrap()),
            message: "ping".to_string(),
            webhook_url: Url::parse("https://example.com/hook").unwrap(),
        }
    }

    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Tz> {
        Tokyo
            .with_ymd_and_hms(2026, 6, 5, hour, minute, second)
            .single()
            .unwrap()
    }

    fn hb_path(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "chime-test-sched-hb-{}-{}",
            tag,
            std::process::id()
        ));
        p
    }

    fn cfg_with(
        tag: &str,
        reminders: Vec<RunReminder>,
        status_pages: Vec<RunStatusPage>,
    ) -> RunConfig {
        RunConfig {
            log_level: LogLevel::Info,
            interval: Duration::from_secs(30),
            timezone: Tokyo,
            reminders,
            status_pages,
            heartbeat_path: hb_path(tag),
        }
    }

    #[tokio::test]
    async fn fires_once_within_same_minute() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with("fires_once", vec![mk_run_reminder("daily", 9, 30)], vec![]);
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), FakeSource::empty());

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 30, 30)).await;
        scheduler.tick(at(9, 30, 59)).await;

        assert_eq!(notifier.sent(), 1);
    }

    #[tokio::test]
    async fn reminder_payload_is_plain_content() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with("payload", vec![mk_run_reminder("daily", 9, 30)], vec![]);
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), FakeSource::empty());

        scheduler.tick(at(9, 30, 0)).await;

        let sent = notifier.messages.lock().unwrap();
        assert_eq!(sent[0].content.as_deref(), Some("ping"));
        assert!(sent[0].username.is_none());
    }

    #[tokio::test]
    async fn fires_again_in_next_matching_minute() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with(
            "fires_again",
            vec![mk_run_reminder("hourly", 9, 30)],
            vec![],
        );
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), FakeSource::empty());

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 31, 0)).await;
        // The schedule only fires at 9:30, so 9:31 does not count.
        assert_eq!(notifier.sent(), 1);
    }

    #[tokio::test]
    async fn does_not_fire_off_schedule() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with(
            "does_not_fire",
            vec![mk_run_reminder("daily", 9, 30)],
            vec![],
        );
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), FakeSource::empty());

        scheduler.tick(at(9, 29, 30)).await;
        scheduler.tick(at(9, 31, 0)).await;
        assert_eq!(notifier.sent(), 0);
    }

    #[tokio::test]
    async fn tick_writes_heartbeat() {
        let path = hb_path("writes");
        let _ = std::fs::remove_file(&path);
        let mut cfg = cfg_with("writes", vec![mk_run_reminder("daily", 9, 30)], vec![]);
        cfg.heartbeat_path = path.clone();
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), FakeSource::empty());

        // Off-schedule time: no reminder fires, but the heartbeat must still be written.
        scheduler.tick(at(0, 0, 0)).await;

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(!contents.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn status_page_is_polled_once_per_interval() {
        let source = FakeSource::empty();
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("poll_interval", vec![], vec![page]);
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), source.clone());

        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(source.calls(), 1);
        // Well inside the interval: no second request.
        scheduler.tick(at(9, 2, 0)).await;
        scheduler.tick(at(9, 4, 59)).await;
        assert_eq!(source.calls(), 1);
        // Interval elapsed.
        scheduler.tick(at(9, 5, 0)).await;
        assert_eq!(source.calls(), 2);
    }

    #[tokio::test]
    async fn first_poll_does_not_forward_existing_incidents() {
        let backlog = vec![mk_incident(
            "old",
            Impact::Critical,
            vec![mk_update("old1", "resolved", "2026-06-04T10:00:00Z")],
        )];
        let source = FakeSource::with(vec![backlog]);
        let notifier = CountingNotifier::new();
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("cold_start", vec![], vec![page]);
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), source);

        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(notifier.sent(), 0);
    }

    #[tokio::test]
    async fn new_incident_is_forwarded_as_an_embed() {
        let backlog = vec![mk_incident(
            "old",
            Impact::Minor,
            vec![mk_update("old1", "resolved", "2026-06-04T10:00:00Z")],
        )];
        let mut updated = backlog.clone();
        updated.push(mk_incident(
            "new",
            Impact::Major,
            vec![mk_update("new1", "investigating", "2026-06-05T09:01:00Z")],
        ));
        let source = FakeSource::with(vec![backlog, updated]);
        let notifier = CountingNotifier::new();
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("forward", vec![], vec![page]);
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), source);

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;

        assert_eq!(notifier.sent(), 1);
        let sent = notifier.messages.lock().unwrap();
        assert_eq!(sent[0].username.as_deref(), Some("claude Status"));
        assert!(sent[0].content.is_none());
    }

    #[tokio::test]
    async fn poll_failure_does_not_stop_the_loop() {
        let notifier = CountingNotifier::new();
        let source = FakeSource::failing();
        let page = mk_run_status_page("claude", Duration::from_secs(60));
        let cfg = cfg_with(
            "poll_fail",
            vec![mk_run_reminder("daily", 9, 30)],
            vec![page],
        );
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), source.clone());

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 31, 0)).await;

        assert_eq!(source.calls(), 2);
        // The reminder still fired; the failing status page notified nothing.
        assert_eq!(notifier.sent(), 1);
    }

    #[tokio::test]
    async fn only_one_status_page_is_polled_per_tick() {
        let source = FakeSource::empty();
        let cfg = cfg_with(
            "one_per_tick",
            vec![],
            vec![
                mk_run_status_page("a", Duration::from_secs(60)),
                mk_run_status_page("b", Duration::from_secs(60)),
                mk_run_status_page("c", Duration::from_secs(60)),
            ],
        );
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), source.clone());

        // All three are due on the first tick, but they are spread over three ticks.
        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(source.calls(), 1);
        scheduler.tick(at(9, 0, 1)).await;
        assert_eq!(source.calls(), 2);
        scheduler.tick(at(9, 0, 2)).await;
        assert_eq!(source.calls(), 3);
        assert_eq!(scheduler.last_polled.len(), 3);

        // None is due again until its interval elapses.
        scheduler.tick(at(9, 0, 3)).await;
        assert_eq!(source.calls(), 3);
    }

    #[tokio::test]
    async fn the_most_overdue_page_is_polled_first() {
        let cfg = cfg_with(
            "overdue",
            vec![],
            vec![
                mk_run_status_page("a", Duration::from_secs(60)),
                mk_run_status_page("b", Duration::from_secs(60)),
            ],
        );
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), FakeSource::empty());
        scheduler.last_polled.insert("a".to_string(), at(9, 0, 0));
        scheduler.last_polled.insert("b".to_string(), at(8, 0, 0));

        // Both are due, but `b` has waited an hour longer.
        scheduler.tick(at(9, 5, 0)).await;

        assert_eq!(scheduler.last_polled["b"], at(9, 5, 0));
        assert_eq!(scheduler.last_polled["a"], at(9, 0, 0), "a waits its turn");
    }

    #[tokio::test]
    async fn status_summary_stays_closed_until_the_window_elapses() {
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_open", vec![], vec![page]);
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), FakeSource::unchanged());

        // The window is anchored to the first tick, so that tick reports nothing.
        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(scheduler.take_due_summary(at(9, 0, 0)), None);
        assert_eq!(scheduler.take_due_summary(at(9, 59, 59)), None);
        assert!(scheduler.take_due_summary(at(10, 0, 0)).is_some());
    }

    #[tokio::test]
    async fn status_summary_counts_every_poll_outcome() {
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_304", vec![], vec![page]);
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), FakeSource::unchanged());

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;
        scheduler.tick(at(9, 10, 0)).await;

        let (window_sec, stats) = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(window_sec, 3600);
        assert_eq!(
            stats,
            PollStats {
                polls: 3,
                not_modified: 3,
                updated: 0,
                failed: 0,
                forwarded: 0,
            }
        );
    }

    #[tokio::test]
    async fn status_summary_counts_failures_and_forwarded_updates() {
        let backlog = vec![mk_incident(
            "old",
            Impact::Minor,
            vec![mk_update("old1", "resolved", "2026-06-04T10:00:00Z")],
        )];
        let mut updated = backlog.clone();
        updated.push(mk_incident(
            "new",
            Impact::Major,
            vec![mk_update("new1", "investigating", "2026-06-05T09:01:00Z")],
        ));
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_mixed", vec![], vec![page]);
        let mut scheduler = Scheduler::new(
            cfg,
            CountingNotifier::new(),
            FakeSource::with(vec![backlog, updated]),
        );

        // Cold-start baseline, then one incident that produces a single forward.
        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;

        let (_, stats) = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(stats.polls, 2);
        assert_eq!(stats.updated, 2);
        assert_eq!(stats.forwarded, 1);

        let failing = cfg_with(
            "summary_failed",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(300))],
        );
        let mut scheduler = Scheduler::new(failing, CountingNotifier::new(), FakeSource::failing());
        scheduler.tick(at(9, 0, 0)).await;

        let (_, stats) = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.forwarded, 0);
    }

    #[tokio::test]
    async fn status_summary_resets_between_windows() {
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_reset", vec![], vec![page]);
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), FakeSource::unchanged());

        scheduler.tick(at(9, 0, 0)).await;
        // Closes the first window; the second must start from zero.
        scheduler.tick(at(10, 0, 0)).await;
        let (_, stats) = scheduler.take_due_summary(at(11, 0, 0)).unwrap();
        assert_eq!(stats, PollStats::default());
    }

    #[tokio::test]
    async fn status_summary_is_silent_without_status_pages() {
        let cfg = cfg_with(
            "summary_no_pages",
            vec![mk_run_reminder("daily", 9, 30)],
            vec![],
        );
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), FakeSource::empty());

        scheduler.tick(at(9, 30, 0)).await;
        assert_eq!(scheduler.take_due_summary(at(23, 0, 0)), None);
    }

    #[tokio::test]
    async fn status_summary_recovers_from_a_backwards_clock() {
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_backwards", vec![], vec![page]);
        let mut scheduler = Scheduler::new(cfg, CountingNotifier::new(), FakeSource::unchanged());

        scheduler.tick(at(9, 0, 0)).await;
        // Stepping backwards closes the window rather than parking it until the
        // clock catches up, which would suppress the liveness signal for an hour.
        let (window_sec, _) = scheduler.take_due_summary(at(8, 0, 0)).unwrap();
        assert_eq!(window_sec, 0);
    }

    #[test]
    fn is_due_handles_first_run_and_backwards_clock() {
        let interval = Duration::from_secs(300);
        assert!(is_due(None, interval, at(9, 0, 0)));
        assert!(!is_due(Some(&at(9, 0, 0)), interval, at(9, 4, 59)));
        assert!(is_due(Some(&at(9, 0, 0)), interval, at(9, 5, 0)));
        // Clock stepped backwards: poll rather than wait it out.
        assert!(is_due(Some(&at(9, 0, 0)), interval, at(8, 0, 0)));
    }
}
