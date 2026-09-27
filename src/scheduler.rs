use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, error, info, warn};
use url::Url;

use crate::fetch::Fetched;
use crate::notifier::{DiscordMessage, Notifier};
use crate::runtime::{RunConfig, RunFeed, RunStatusPage};
use crate::status::{self, PageState, StatusSource};
use crate::watch::{self, Diff, FeedState, WatchSource};

/// How often the pollers report that they are alive.
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
    Updated {
        forwarded: u64,
        send_failed: u64,
    },
    Failed,
    /// A shared feed was fetched, but at least one watch on it could not read its
    /// version. Counted as `failed`; the watches that could read were still diffed
    /// and sent, and their posts are counted as usual.
    PartlyUnreadable {
        forwarded: u64,
        send_failed: u64,
    },
}

/// Counters for one summary window. Reset when the window closes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PollStats {
    polls: u64,
    not_modified: u64,
    updated: u64,
    failed: u64,
    forwarded: u64,
    send_failed: u64,
}

impl PollStats {
    fn record(&mut self, outcome: PollOutcome) {
        self.polls += 1;
        match outcome {
            PollOutcome::NotModified => self.not_modified += 1,
            PollOutcome::Updated {
                forwarded,
                send_failed,
            } => {
                self.updated += 1;
                self.forwarded += forwarded;
                self.send_failed += send_failed;
            }
            PollOutcome::Failed => self.failed += 1,
            PollOutcome::PartlyUnreadable {
                forwarded,
                send_failed,
            } => {
                self.failed += 1;
                self.forwarded += forwarded;
                self.send_failed += send_failed;
            }
        }
    }
}

/// One pollable entry, addressed by position in its `RunConfig` list. Indexes are
/// stable because the config is immutable for the life of the scheduler, and two
/// lists may legitimately share a name. `Ord` is derived on purpose: variant order
/// then index is the tie-break in `most_overdue`, so pages win an exact tie and
/// declaration order settles the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Job {
    Page(usize),
    Feed(usize),
}

/// One closed summary window. A struct rather than a tuple of two `PollStats`
/// because the two halves have the same type and would be positionally ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    window_sec: i64,
    pages: PollStats,
    watches: PollStats,
}

pub struct Scheduler<N: Notifier, S: StatusSource, W: WatchSource> {
    cfg: RunConfig,
    notifier: N,
    source: S,
    watch_source: W,
    last_fired: HashMap<String, DateTime<Tz>>,
    last_polled: HashMap<Job, DateTime<Tz>>,
    page_states: HashMap<String, PageState>,
    feed_states: HashMap<usize, FeedState>,
    stats: PollStats,
    watch_stats: PollStats,
    /// `None` until the first tick: the window is anchored to a real tick rather
    /// than to construction, so a scheduler built long before it runs does not
    /// report an oversized first window.
    summary_window_start: Option<DateTime<Tz>>,
}

impl<N: Notifier, S: StatusSource, W: WatchSource> Scheduler<N, S, W> {
    pub fn new(cfg: RunConfig, notifier: N, source: S, watch_source: W) -> Self {
        Scheduler {
            cfg,
            notifier,
            source,
            watch_source,
            last_fired: HashMap::new(),
            last_polled: HashMap::new(),
            page_states: HashMap::new(),
            feed_states: HashMap::new(),
            stats: PollStats::default(),
            watch_stats: PollStats::default(),
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
        // Before this tick's poll, not after: the window has to close on the same
        // boundary it opened on, or the closing tick's poll is counted in the window
        // that ends and the first window reports one poll more than every window
        // after it.
        self.emit_poll_summaries(now);
        self.poll_one(now).await;
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

    /// Poll at most one status page or watch per tick.
    ///
    /// The heartbeat is written once, at the top of the tick, and `chime health`
    /// calls it stale past `2 * tick_interval`. Polling every due page and watch in
    /// one tick would let N endpoints behind a network partition hold the tick for
    /// `N * FETCH_TIMEOUT` and get the container restarted over someone else's
    /// outage. One fetch per tick — shared by both lists — bounds that however many
    /// are configured, and picking the most overdue one keeps them from staying in
    /// the lockstep they start in — every entry is due on the very first tick.
    async fn poll_one(&mut self, now: DateTime<Tz>) {
        let Some(job) = self.most_overdue(now) else {
            return;
        };
        // Recorded before the request: a slow or failing endpoint must wait out its
        // own interval, not be retried on every tick.
        self.last_polled.insert(job, now);
        // Borrows are split by field rather than cloning the entry: `poll_page` and
        // `poll_watch` are free functions so `cfg`, the state maps, the sources and
        // `notifier` can be held at once.
        match job {
            Job::Page(i) => {
                let page = &self.cfg.status_pages[i];
                let state = self.page_states.entry(page.name.clone()).or_default();
                let outcome = poll_page(&self.source, &self.notifier, page, state).await;
                self.stats.record(outcome);
            }
            Job::Feed(i) => {
                let feed = &self.cfg.feeds[i];
                let state = self.feed_states.entry(i).or_default();
                let detected_at = now.with_timezone(&Utc);
                let outcome =
                    poll_feed(&self.watch_source, &self.notifier, feed, state, detected_at).await;
                self.watch_stats.record(outcome);
            }
        }
    }

    /// Log the closed summary window, one line per non-empty list, if one is due.
    fn emit_poll_summaries(&mut self, now: DateTime<Tz>) {
        let Some(summary) = self.take_due_summary(now) else {
            return;
        };
        let window_sec = summary.window_sec;
        if !self.cfg.status_pages.is_empty() {
            let stats = summary.pages;
            info!(
                window_sec,
                pages = self.cfg.status_pages.len(),
                polls = stats.polls,
                not_modified = stats.not_modified,
                updated = stats.updated,
                failed = stats.failed,
                forwarded = stats.forwarded,
                send_failed = stats.send_failed,
                "status poll summary"
            );
        }
        if !self.cfg.feeds.is_empty() {
            let stats = summary.watches;
            info!(
                window_sec,
                watches = self.cfg.watch_count(),
                polls = stats.polls,
                not_modified = stats.not_modified,
                updated = stats.updated,
                failed = stats.failed,
                forwarded = stats.forwarded,
                send_failed = stats.send_failed,
                "watch poll summary"
            );
        }
    }

    /// Close the summary window and hand back its length and counters, or `None`
    /// while the window is still open. Split from the logging so the window
    /// arithmetic is testable without a tracing subscriber.
    fn take_due_summary(&mut self, now: DateTime<Tz>) -> Option<Summary> {
        // A reminder-only deployment has no poller to prove alive; staying silent
        // keeps this out of logs that would never contain a poll line anyway.
        if self.cfg.status_pages.is_empty() && self.cfg.feeds.is_empty() {
            return None;
        }
        let Some(start) = self.summary_window_start else {
            self.summary_window_start = Some(now);
            return None;
        };
        let elapsed = now.signed_duration_since(start).num_seconds();
        if elapsed < 0 {
            // A clock stepping backwards must not park the summary until the clock
            // catches up, so the window is re-anchored here. Why not close it and
            // report: an NTP correction would emit a rate over a zero-length window
            // and throw away however much of the hour had already been counted. The
            // counters ride across instead, so the next `window_sec` covers less
            // real time than the counters it reports and the implied poll rate runs
            // high — cheaper than either artifact.
            self.summary_window_start = Some(now);
            return None;
        }
        if elapsed < STATUS_SUMMARY_INTERVAL.as_secs() as i64 {
            return None;
        }
        self.summary_window_start = Some(now);
        Some(Summary {
            window_sec: elapsed,
            pages: std::mem::take(&mut self.stats),
            watches: std::mem::take(&mut self.watch_stats),
        })
    }

    /// Every pollable entry with its interval, pages first.
    fn jobs(&self) -> impl Iterator<Item = (Job, Duration)> + '_ {
        let pages = (self.cfg.status_pages.iter().enumerate())
            .map(|(i, p)| (Job::Page(i), p.poll_interval));
        let feeds =
            (self.cfg.feeds.iter().enumerate()).map(|(i, f)| (Job::Feed(i), f.poll_interval));
        pages.chain(feeds)
    }

    fn most_overdue(&self, now: DateTime<Tz>) -> Option<Job> {
        self.jobs()
            .filter(|(job, interval)| is_due(self.last_polled.get(job), *interval, now))
            // Longest overdue first; `Job`'s `Ord` settles a tie, so the choice is
            // deterministic rather than hash-order dependent.
            .min_by_key(|(job, _)| (Reverse(overdue_secs(self.last_polled.get(job), now)), *job))
            .map(|(job, _)| job)
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
        Fetched::Modified { value, etag } => (value, etag),
    };

    state.etag = new_etag;
    let events = status::diff(state, &incidents, page.min_impact);
    let mut forwarded = 0;
    let mut send_failed = 0;
    for event in events {
        let message = status::build_message(page, &event);
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
            Err(e) => {
                send_failed += 1;
                error!(
                    status_page = %page.name,
                    incident = %event.incident_id,
                    error = %e,
                    "failed to forward status update"
                )
            }
        }
    }
    PollOutcome::Updated {
        forwarded,
        send_failed,
    }
}

async fn poll_feed<N: Notifier, W: WatchSource>(
    source: &W,
    notifier: &N,
    feed: &RunFeed,
    state: &mut FeedState,
    detected_at: DateTime<Utc>,
) -> PollOutcome {
    let fetched = match source.fetch(&feed.url, state.etag()).await {
        Ok(f) => f,
        Err(e) => {
            warn!(watch = %feed.names(), error = %e, "failed to poll watch");
            return PollOutcome::Failed;
        }
    };
    let (body, etag) = match fetched {
        Fetched::NotModified => {
            debug!(watch = %feed.names(), "watch not modified");
            return PollOutcome::NotModified;
        }
        Fetched::Modified { value, etag } => (value, etag),
    };

    let results = watch::read_feed(state, &feed.watches, &body, etag);
    let mut unreadable = false;
    let mut forwarded = 0;
    let mut send_failed = 0;
    for (w, result) in feed.watches.iter().zip(results) {
        let event = match result {
            Err(e) => {
                unreadable = true;
                warn!(watch = %w.name, error = %e, "failed to read release from watch");
                continue;
            }
            Ok(Diff::Baseline(version)) => {
                info!(watch = %w.name, version = %version, "watch baseline recorded");
                continue;
            }
            Ok(Diff::Unchanged) => {
                debug!(watch = %w.name, "watch unchanged");
                continue;
            }
            Ok(Diff::Changed(event)) => event,
        };
        let message = watch::build_message(w, &event, detected_at);
        match notifier.send(&w.webhook_url, &message).await {
            Ok(()) => {
                forwarded += 1;
                info!(
                    watch = %w.name,
                    version = %event.release.version,
                    previous = %event.previous,
                    "release forwarded"
                );
            }
            Err(e) => {
                send_failed += 1;
                error!(
                    watch = %w.name,
                    version = %event.release.version,
                    error = %e,
                    "failed to forward release"
                );
            }
        }
    }
    if unreadable {
        PollOutcome::PartlyUnreadable {
            forwarded,
            send_failed,
        }
    } else {
        PollOutcome::Updated {
            forwarded,
            send_failed,
        }
    }
}

fn is_due(last: Option<&DateTime<Tz>>, poll_interval: Duration, now: DateTime<Tz>) -> bool {
    match last {
        None => true,
        Some(previous) => {
            let elapsed = now.signed_duration_since(*previous).num_seconds();
            // A clock stepping backwards must not park an entry until it catches up.
            elapsed < 0 || elapsed >= poll_interval.as_secs() as i64
        }
    }
}

/// An entry that has never been polled outranks every entry that has.
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
    use crate::fetch::FetchError;
    use crate::notifier::NotifyError;
    use crate::runtime::{
        RunReminder, mk_run_feed, mk_run_status_page, mk_run_watch, mk_shared_feed,
    };
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
        timestamps: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl CountingNotifier {
        fn new() -> Self {
            CountingNotifier {
                count: Arc::new(AtomicUsize::new(0)),
                messages: Arc::new(Mutex::new(Vec::new())),
                timestamps: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn sent(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    impl Notifier for CountingNotifier {
        async fn send(&self, _webhook: &Url, message: &DiscordMessage) -> Result<(), NotifyError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.timestamps
                .lock()
                .unwrap()
                .push(message.embeds.first().and_then(|e| e.timestamp.clone()));
            self.messages.lock().unwrap().push(DiscordMessage {
                content: message.content.clone(),
                username: message.username.clone(),
                avatar_url: message.avatar_url.clone(),
                embeds: Vec::new(),
            });
            Ok(())
        }
    }

    /// Rejects every send, standing in for a webhook Discord no longer accepts.
    #[derive(Clone)]
    struct RejectingNotifier;

    impl Notifier for RejectingNotifier {
        async fn send(&self, _webhook: &Url, _message: &DiscordMessage) -> Result<(), NotifyError> {
            Err(NotifyError::Status {
                status: 404,
                body: "unknown webhook".to_string(),
            })
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
        async fn fetch(
            &self,
            _url: &Url,
            _etag: Option<&str>,
        ) -> Result<Fetched<Vec<Incident>>, StatusError> {
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
                value: incidents,
                etag: None,
            })
        }
    }

    /// Returns the queued JSON bodies in order, repeating the last one once the
    /// queue drains, with the same `fail` / `not_modified` switches as
    /// `FakeSource`. Every `If-None-Match` it receives is recorded in `seen_etags`.
    #[derive(Clone)]
    struct FakeWatchSource {
        calls: Arc<AtomicUsize>,
        queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
        seen_etags: Arc<Mutex<Vec<Option<String>>>>,
        fail: bool,
        not_modified: bool,
    }

    impl FakeWatchSource {
        fn empty() -> Self {
            FakeWatchSource {
                calls: Arc::new(AtomicUsize::new(0)),
                queue: Arc::new(Mutex::new(VecDeque::new())),
                seen_etags: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                not_modified: false,
            }
        }

        fn with(bodies: Vec<&str>) -> Self {
            FakeWatchSource {
                queue: Arc::new(Mutex::new(
                    bodies.into_iter().map(|b| b.as_bytes().to_vec()).collect(),
                )),
                ..FakeWatchSource::empty()
            }
        }

        fn failing() -> Self {
            FakeWatchSource {
                fail: true,
                ..FakeWatchSource::empty()
            }
        }

        fn unchanged() -> Self {
            FakeWatchSource {
                not_modified: true,
                ..FakeWatchSource::empty()
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl WatchSource for FakeWatchSource {
        async fn fetch(
            &self,
            _url: &Url,
            etag: Option<&str>,
        ) -> Result<Fetched<Vec<u8>>, FetchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen_etags
                .lock()
                .unwrap()
                .push(etag.map(str::to_string));
            if self.fail {
                return Err(FetchError::Status {
                    status: 503,
                    body: "unavailable".to_string(),
                });
            }
            if self.not_modified {
                return Ok(Fetched::NotModified);
            }
            let mut queue = self.queue.lock().unwrap();
            let body = if queue.len() > 1 {
                queue.pop_front().unwrap()
            } else {
                queue.front().cloned().unwrap_or_default()
            };
            Ok(Fetched::Modified {
                value: body,
                etag: Some("W/\"fake\"".to_string()),
            })
        }
    }

    /// A scheduler whose watch source is never consulted, for tests that do not
    /// configure watches.
    fn sched<N: Notifier, S: StatusSource>(
        cfg: RunConfig,
        notifier: N,
        source: S,
    ) -> Scheduler<N, S, FakeWatchSource> {
        Scheduler::new(cfg, notifier, source, FakeWatchSource::unchanged())
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
        cfg_with_watches(tag, reminders, status_pages, Vec::new())
    }

    fn cfg_with_watches(
        tag: &str,
        reminders: Vec<RunReminder>,
        status_pages: Vec<RunStatusPage>,
        feeds: Vec<RunFeed>,
    ) -> RunConfig {
        RunConfig {
            log_level: LogLevel::Info,
            interval: Duration::from_secs(30),
            timezone: Tokyo,
            reminders,
            status_pages,
            feeds,
            heartbeat_path: hb_path(tag),
        }
    }

    #[tokio::test]
    async fn fires_once_within_same_minute() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with("fires_once", vec![mk_run_reminder("daily", 9, 30)], vec![]);
        let mut scheduler = sched(cfg, notifier.clone(), FakeSource::empty());

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 30, 30)).await;
        scheduler.tick(at(9, 30, 59)).await;

        assert_eq!(notifier.sent(), 1);
    }

    #[tokio::test]
    async fn reminder_payload_is_plain_content() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with("payload", vec![mk_run_reminder("daily", 9, 30)], vec![]);
        let mut scheduler = sched(cfg, notifier.clone(), FakeSource::empty());

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
        let mut scheduler = sched(cfg, notifier.clone(), FakeSource::empty());

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
        let mut scheduler = sched(cfg, notifier.clone(), FakeSource::empty());

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
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::empty());

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
        let mut scheduler = sched(cfg, CountingNotifier::new(), source.clone());

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
        let mut scheduler = sched(cfg, notifier.clone(), source);

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
        let mut scheduler = sched(cfg, notifier.clone(), source);

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
        let mut scheduler = sched(cfg, notifier.clone(), source.clone());

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
        let mut scheduler = sched(cfg, CountingNotifier::new(), source.clone());

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
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::empty());
        scheduler.last_polled.insert(Job::Page(0), at(9, 0, 0));
        scheduler.last_polled.insert(Job::Page(1), at(8, 0, 0));

        // Both are due, but `b` has waited an hour longer.
        scheduler.tick(at(9, 5, 0)).await;

        assert_eq!(scheduler.last_polled[&Job::Page(1)], at(9, 5, 0));
        assert_eq!(
            scheduler.last_polled[&Job::Page(0)],
            at(9, 0, 0),
            "a waits its turn"
        );
    }

    #[tokio::test]
    async fn status_summary_stays_closed_until_the_window_elapses() {
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_open", vec![], vec![page]);
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::unchanged());

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
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::unchanged());

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;
        scheduler.tick(at(9, 10, 0)).await;

        let summary = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(summary.window_sec, 3600);
        assert_eq!(
            summary.pages,
            PollStats {
                polls: 3,
                not_modified: 3,
                updated: 0,
                failed: 0,
                forwarded: 0,
                send_failed: 0,
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
        let mut scheduler = sched(
            cfg,
            CountingNotifier::new(),
            FakeSource::with(vec![backlog, updated]),
        );

        // Cold-start baseline, then one incident that produces a single forward.
        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;

        let stats = scheduler.take_due_summary(at(10, 0, 0)).unwrap().pages;
        assert_eq!(stats.polls, 2);
        assert_eq!(stats.updated, 2);
        assert_eq!(stats.forwarded, 1);

        let failing = cfg_with(
            "summary_failed",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(300))],
        );
        let mut scheduler = sched(failing, CountingNotifier::new(), FakeSource::failing());
        scheduler.tick(at(9, 0, 0)).await;

        let stats = scheduler.take_due_summary(at(10, 0, 0)).unwrap().pages;
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.forwarded, 0);
    }

    #[tokio::test]
    async fn status_summary_separates_a_rejected_post_from_having_nothing_to_post() {
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
        let cfg = cfg_with(
            "summary_send_failed",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(300))],
        );
        let mut scheduler = sched(
            cfg,
            RejectingNotifier,
            FakeSource::with(vec![backlog, updated]),
        );

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;

        let stats = scheduler.take_due_summary(at(10, 0, 0)).unwrap().pages;
        assert_eq!(stats.updated, 2);
        assert_eq!(stats.forwarded, 0);
        assert_eq!(stats.send_failed, 1);
    }

    #[tokio::test]
    async fn status_summary_resets_between_windows() {
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_reset", vec![], vec![page]);
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::unchanged());

        // Three ticks an hour apart. The middle one closes the first window, and
        // its own poll belongs to the second — every window covers the same number
        // of ticks, rather than the first one counting both of its boundaries.
        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(10, 0, 0)).await;
        scheduler.tick(at(11, 0, 0)).await;

        let stats = scheduler.take_due_summary(at(12, 0, 0)).unwrap().pages;
        assert_eq!(stats.polls, 1, "only the 11:00 tick is in this window");
        assert_eq!(stats.not_modified, 1);
    }

    #[tokio::test]
    async fn status_summary_window_length_does_not_drift_on_the_first_window() {
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_first_window", vec![], vec![page]);
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::unchanged());

        // Ticks every 30 minutes, so each window holds exactly two of them.
        for hour in 9..=12 {
            scheduler.tick(at(hour, 0, 0)).await;
            scheduler.tick(at(hour, 30, 0)).await;
        }

        let summary = scheduler.take_due_summary(at(13, 0, 0)).unwrap();
        assert_eq!(summary.window_sec, 3600);
        assert_eq!(
            summary.pages.polls, 2,
            "the boundary tick is not counted twice"
        );
    }

    #[tokio::test]
    async fn summary_is_silent_without_pages_or_watches() {
        let cfg = cfg_with(
            "summary_no_pages",
            vec![mk_run_reminder("daily", 9, 30)],
            vec![],
        );
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::empty());

        scheduler.tick(at(9, 30, 0)).await;
        assert_eq!(scheduler.take_due_summary(at(23, 0, 0)), None);
    }

    #[tokio::test]
    async fn status_summary_recovers_from_a_backwards_clock() {
        let page = mk_run_status_page("claude", Duration::from_secs(300));
        let cfg = cfg_with("summary_backwards", vec![], vec![page]);
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::unchanged());

        scheduler.tick(at(9, 0, 0)).await;
        // Stepping backwards re-anchors the window: nothing is reported over the
        // negative span, and the counters already gathered survive.
        assert_eq!(scheduler.take_due_summary(at(8, 0, 0)), None);
        // The window now runs from the stepped-back time, so the liveness signal
        // returns one interval later rather than being parked until the clock
        // catches up to where it was.
        let summary = scheduler.take_due_summary(at(9, 0, 0)).unwrap();
        assert_eq!(summary.window_sec, 3600);
        assert_eq!(summary.pages.polls, 1, "the pre-step poll is not discarded");
    }

    fn one_watch(tag: &str) -> RunConfig {
        cfg_with_watches(
            tag,
            vec![],
            vec![],
            vec![mk_run_feed("node", Duration::from_secs(300))],
        )
    }

    #[tokio::test]
    async fn watch_is_polled_once_per_interval() {
        let source = FakeWatchSource::with(vec![r#"{"version":"1"}"#]);
        let cfg = one_watch("watch_interval");
        let mut scheduler = Scheduler::new(
            cfg,
            CountingNotifier::new(),
            FakeSource::empty(),
            source.clone(),
        );

        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(source.calls(), 1);
        scheduler.tick(at(9, 4, 59)).await;
        assert_eq!(source.calls(), 1);
        scheduler.tick(at(9, 5, 0)).await;
        assert_eq!(source.calls(), 2);
    }

    #[tokio::test]
    async fn first_watch_poll_does_not_forward_the_current_version() {
        let notifier = CountingNotifier::new();
        let source = FakeWatchSource::with(vec![r#"{"version":"1"}"#]);
        let cfg = one_watch("watch_cold");
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), FakeSource::empty(), source);

        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(notifier.sent(), 0);
    }

    #[tokio::test]
    async fn new_version_is_forwarded_as_an_embed() {
        let notifier = CountingNotifier::new();
        let source = FakeWatchSource::with(vec![r#"{"version":"1"}"#, r#"{"version":"2"}"#]);
        let cfg = one_watch("watch_forward");
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), FakeSource::empty(), source);

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;

        assert_eq!(notifier.sent(), 1);
        let sent = notifier.messages.lock().unwrap();
        assert_eq!(sent[0].username.as_deref(), Some("node Releases"));
        assert!(sent[0].content.is_none());
    }

    #[tokio::test]
    async fn a_release_without_its_own_time_is_stamped_with_the_tick_that_saw_it() {
        let notifier = CountingNotifier::new();
        let source = FakeWatchSource::with(vec![r#"{"version":"1"}"#, r#"{"version":"2"}"#]);
        let cfg = one_watch("watch_detected_at");
        let mut scheduler = Scheduler::new(cfg, notifier.clone(), FakeSource::empty(), source);

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;

        assert_eq!(
            *notifier.timestamps.lock().unwrap(),
            vec![Some("2026-06-05T00:05:00+00:00".to_string())],
            "09:05 in Tokyo, rendered in UTC"
        );
    }

    #[tokio::test]
    async fn an_unchanged_watch_body_posts_nothing() {
        let notifier = CountingNotifier::new();
        let source = FakeWatchSource::with(vec![r#"{"version":"1"}"#]);
        let cfg = one_watch("watch_same");
        let mut scheduler =
            Scheduler::new(cfg, notifier.clone(), FakeSource::empty(), source.clone());

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;
        scheduler.tick(at(9, 10, 0)).await;

        assert_eq!(source.calls(), 3);
        assert_eq!(notifier.sent(), 0);
    }

    #[tokio::test]
    async fn watch_poll_failure_does_not_stop_the_loop() {
        let notifier = CountingNotifier::new();
        let source = FakeWatchSource::failing();
        let cfg = cfg_with_watches(
            "watch_fail",
            vec![mk_run_reminder("daily", 9, 30)],
            vec![],
            vec![mk_run_feed("node", Duration::from_secs(60))],
        );
        let mut scheduler =
            Scheduler::new(cfg, notifier.clone(), FakeSource::empty(), source.clone());

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 31, 0)).await;

        assert_eq!(source.calls(), 2);
        assert_eq!(notifier.sent(), 1, "only the reminder was sent");
    }

    #[tokio::test]
    async fn extract_failure_does_not_store_the_etag() {
        let source = FakeWatchSource::with(vec![r#"{"nope":1}"#, r#"{"version":"1"}"#]);
        let cfg = one_watch("watch_etag");
        let mut scheduler = Scheduler::new(
            cfg,
            CountingNotifier::new(),
            FakeSource::empty(),
            source.clone(),
        );

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;
        scheduler.tick(at(9, 10, 0)).await;

        assert_eq!(
            *source.seen_etags.lock().unwrap(),
            vec![None, None, Some("W/\"fake\"".to_string())],
            "the unreadable body's ETag is not replayed; the readable one's is"
        );
    }

    #[tokio::test]
    async fn pages_and_watches_share_the_one_poll_per_tick_budget() {
        let pages = FakeSource::empty();
        let watches = FakeWatchSource::with(vec![r#"{"version":"1"}"#]);
        let cfg = cfg_with_watches(
            "shared_budget",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(60))],
            vec![mk_run_feed("node", Duration::from_secs(60))],
        );
        let mut scheduler =
            Scheduler::new(cfg, CountingNotifier::new(), pages.clone(), watches.clone());

        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(pages.calls() + watches.calls(), 1);
        scheduler.tick(at(9, 0, 1)).await;
        assert_eq!(pages.calls(), 1);
        assert_eq!(watches.calls(), 1);
    }

    #[tokio::test]
    async fn the_most_overdue_job_is_polled_first_across_pages_and_watches() {
        let cfg = cfg_with_watches(
            "overdue_mixed",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(60))],
            vec![mk_run_feed("node", Duration::from_secs(60))],
        );
        let watches = FakeWatchSource::with(vec![r#"{"version":"1"}"#]);
        let pages = FakeSource::empty();
        let mut scheduler =
            Scheduler::new(cfg, CountingNotifier::new(), pages.clone(), watches.clone());
        scheduler.last_polled.insert(Job::Page(0), at(9, 0, 0));
        scheduler.last_polled.insert(Job::Feed(0), at(8, 0, 0));

        scheduler.tick(at(9, 5, 0)).await;

        assert_eq!(watches.calls(), 1, "the watch has waited an hour longer");
        assert_eq!(pages.calls(), 0);
    }

    #[tokio::test]
    async fn a_page_outranks_a_watch_on_an_exact_tie() {
        let cfg = cfg_with_watches(
            "overdue_tie",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(60))],
            vec![mk_run_feed("node", Duration::from_secs(60))],
        );
        let watches = FakeWatchSource::with(vec![r#"{"version":"1"}"#]);
        let pages = FakeSource::empty();
        let mut scheduler =
            Scheduler::new(cfg, CountingNotifier::new(), pages.clone(), watches.clone());

        // Neither has been polled, so both are infinitely overdue.
        scheduler.tick(at(9, 0, 0)).await;

        assert_eq!(pages.calls(), 1);
        assert_eq!(watches.calls(), 0);
    }

    #[tokio::test]
    async fn watch_summary_counts_every_poll_outcome() {
        let cfg = cfg_with_watches(
            "watch_summary",
            vec![],
            vec![],
            vec![
                mk_run_feed("ok", Duration::from_secs(300)),
                mk_run_feed("bad", Duration::from_secs(300)),
            ],
        );
        let mut scheduler = Scheduler::new(cfg, RejectingNotifier, FakeSource::empty(), {
            // `ok` and `bad` share the fake, so its queue is consumed in poll order:
            // ok=1 (baseline), bad={} (extract failure), ok=2 (change, rejected post).
            FakeWatchSource::with(vec![r#"{"version":"1"}"#, r#"{}"#, r#"{"version":"2"}"#])
        });

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 0, 30)).await;
        scheduler.tick(at(9, 5, 0)).await;

        let summary = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(summary.pages, PollStats::default());
        assert_eq!(
            summary.watches,
            PollStats {
                polls: 3,
                not_modified: 0,
                updated: 2,
                failed: 1,
                forwarded: 0,
                send_failed: 1,
            }
        );
    }

    #[tokio::test]
    async fn watch_summary_is_emitted_without_status_pages() {
        let source = FakeWatchSource::unchanged();
        let cfg = one_watch("watch_summary_only");
        let mut scheduler =
            Scheduler::new(cfg, CountingNotifier::new(), FakeSource::empty(), source);

        scheduler.tick(at(9, 0, 0)).await;

        let summary = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(summary.watches.polls, 1);
        assert_eq!(summary.watches.not_modified, 1);
    }

    fn pointer_watch(name: &str, pointer: &str) -> crate::runtime::RunWatch {
        let mut w = mk_run_watch(name);
        w.extractor = crate::watch::Extractor::JsonPointer {
            pointer: pointer.to_string(),
        };
        w
    }

    #[tokio::test]
    async fn watches_on_one_url_share_a_single_request() {
        let notifier = CountingNotifier::new();
        let source = FakeWatchSource::with(vec![r#"{"a":"1","b":"1"}"#, r#"{"a":"2","b":"1"}"#]);
        let cfg = cfg_with_watches(
            "shared_feed",
            vec![],
            vec![],
            vec![mk_shared_feed(
                "shared",
                Duration::from_secs(300),
                vec![pointer_watch("a", "/a"), pointer_watch("b", "/b")],
            )],
        );
        let mut scheduler =
            Scheduler::new(cfg, notifier.clone(), FakeSource::empty(), source.clone());

        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(source.calls(), 1, "both watches baselined from one request");
        scheduler.tick(at(9, 5, 0)).await;

        assert_eq!(source.calls(), 2);
        assert_eq!(notifier.sent(), 1, "only `a` changed");
        let sent = notifier.messages.lock().unwrap();
        assert_eq!(sent[0].username.as_deref(), Some("a Releases"));
    }

    #[tokio::test]
    async fn one_unreadable_watch_does_not_silence_the_others_on_its_feed() {
        let notifier = CountingNotifier::new();
        let source = FakeWatchSource::with(vec![r#"{"a":"1"}"#, r#"{"a":"2"}"#]);
        let cfg = cfg_with_watches(
            "partly_unreadable",
            vec![],
            vec![],
            vec![mk_shared_feed(
                "partly",
                Duration::from_secs(300),
                vec![
                    pointer_watch("a", "/a"),
                    pointer_watch("broken", "/missing"),
                ],
            )],
        );
        let mut scheduler =
            Scheduler::new(cfg, notifier.clone(), FakeSource::empty(), source.clone());

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;

        assert_eq!(notifier.sent(), 1, "`a` still reports its change");
        assert_eq!(
            *source.seen_etags.lock().unwrap(),
            vec![None, None],
            "the feed never answers 304 while one of its watches cannot read it"
        );
        let summary = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(
            summary.watches,
            PollStats {
                polls: 2,
                not_modified: 0,
                updated: 0,
                failed: 2,
                forwarded: 1,
                send_failed: 0,
            }
        );
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
