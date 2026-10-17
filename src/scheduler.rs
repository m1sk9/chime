use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Timelike, Utc};
use chrono_tz::Tz;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, error, info, warn};
use url::Url;

use crate::fetch::{BodySource, Fetched};
use crate::notifier::{DiscordMessage, Notifier, NotifyError};
use crate::rss::{self, FEED_ACCEPT, RssState};
use crate::runtime::{RunConfig, RunFeed, RunRss, RunStatusPage};
use crate::status::{self, PageState, StatusSource};
use crate::watch::{self, Diff, FeedState, WATCH_ACCEPT};

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
    Updated(Sent),
    Failed,
    /// A shared feed was fetched, but at least one watch on it could not read its
    /// version. Counted as `failed`; the watches that could read were still diffed
    /// and sent, and their posts are counted as usual.
    PartlyUnreadable(Sent),
}

/// The Discord posts one poll made.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Sent {
    forwarded: u64,
    send_failed: u64,
}

impl Sent {
    fn record<E>(&mut self, result: &Result<(), E>) {
        match result {
            Ok(()) => self.forwarded += 1,
            Err(_) => self.send_failed += 1,
        }
    }
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
        let sent = match outcome {
            PollOutcome::NotModified => {
                self.not_modified += 1;
                return;
            }
            PollOutcome::Failed => {
                self.failed += 1;
                return;
            }
            PollOutcome::Updated(sent) => {
                self.updated += 1;
                sent
            }
            PollOutcome::PartlyUnreadable(sent) => {
                self.failed += 1;
                sent
            }
        };
        self.forwarded += sent.forwarded;
        self.send_failed += sent.send_failed;
    }
}

/// One pollable entry, addressed by position in its `RunConfig` list. Indexes are
/// stable because the config is immutable for the life of the scheduler, and two
/// lists may legitimately share a name. `Ord` is derived on purpose: variant order
/// then index is the tie-break in `most_overdue`, so pages beat watches, which beat
/// feeds, on an exact tie — the order of urgency — and declaration order settles
/// the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Job {
    Page(usize),
    Feed(usize),
    Rss(usize),
}

/// One closed summary window. A struct rather than a tuple of `PollStats`
/// because the parts have the same type and would be positionally ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    window_sec: i64,
    pages: PollStats,
    watches: PollStats,
    rss: PollStats,
}

pub struct Scheduler<N: Notifier, S: StatusSource, B: BodySource> {
    cfg: RunConfig,
    notifier: N,
    source: S,
    /// Serves both watch feeds and rss feeds; each poller sends its own `Accept`.
    feed_source: B,
    last_fired: HashMap<String, NaiveDateTime>,
    last_polled: HashMap<Job, DateTime<Tz>>,
    page_states: HashMap<usize, PageState>,
    feed_states: HashMap<usize, FeedState>,
    rss_states: HashMap<usize, RssState>,
    stats: PollStats,
    watch_stats: PollStats,
    rss_stats: PollStats,
    /// `None` until the first tick: the window is anchored to a real tick rather
    /// than to construction, so a scheduler built long before it runs does not
    /// report an oversized first window.
    summary_window_start: Option<DateTime<Tz>>,
}

impl<N: Notifier, S: StatusSource, B: BodySource> Scheduler<N, S, B> {
    pub fn new(cfg: RunConfig, notifier: N, source: S, feed_source: B) -> Self {
        Scheduler {
            cfg,
            notifier,
            source,
            feed_source,
            last_fired: HashMap::new(),
            last_polled: HashMap::new(),
            page_states: HashMap::new(),
            feed_states: HashMap::new(),
            rss_states: HashMap::new(),
            stats: PollStats::default(),
            watch_stats: PollStats::default(),
            rss_stats: PollStats::default(),
            summary_window_start: None,
        }
    }

    pub async fn run(self) -> std::io::Result<()> {
        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;
        self.run_until(async move {
            tokio::select! {
                _ = sigint.recv() => info!("received SIGINT, shutting down"),
                _ = sigterm.recv() => info!("received SIGTERM, shutting down"),
            }
        })
        .await;
        Ok(())
    }

    /// Ticks until `shutdown` completes, abandoning a tick that is still running.
    ///
    /// Why the tick races `shutdown` too: one tick can spend tens of seconds on
    /// sends, longer than the grace period `docker stop` waits before SIGKILL.
    /// Dropping it mid-send loses nothing the dedup records do not already treat
    /// as attempted.
    async fn run_until(mut self, shutdown: impl Future<Output = ()>) {
        let mut ticker = interval(self.cfg.interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tokio::pin!(shutdown);

        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = &mut shutdown => return,
            }
            let now = Utc::now().with_timezone(&self.cfg.timezone);
            tokio::select! {
                _ = self.tick(now) => {}
                _ = &mut shutdown => return,
            }
        }
    }

    async fn tick(&mut self, now: DateTime<Tz>) {
        write_heartbeat(&self.cfg.heartbeat_path);
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
        let notifier = Heartbeating {
            inner: &self.notifier,
            path: &self.cfg.heartbeat_path,
        };
        for r in &self.cfg.reminders {
            if !r.fires_at(&now) {
                continue;
            }
            if self.last_fired.insert(r.name.clone(), current_minute) == Some(current_minute) {
                continue;
            }
            let payload = DiscordMessage::text(&r.message);
            match notifier.send(&r.webhook_url, &payload).await {
                Ok(()) => info!(reminder = %r.name, "reminder fired"),
                Err(e) => error!(reminder = %r.name, error = %e, "failed to send reminder"),
            }
        }
    }

    /// Poll at most one status page, watch or feed per tick.
    ///
    /// The heartbeat is written once, at the top of the tick, and `chime health`
    /// calls it stale past `2 * tick_interval`. Polling every due entry in
    /// one tick would let N endpoints behind a network partition hold the tick for
    /// `N * FETCH_TIMEOUT` and get the container restarted over someone else's
    /// outage. One fetch per tick — shared by all lists — bounds that however many
    /// are configured, and picking the most overdue one keeps them from staying in
    /// the lockstep they start in — every entry is due on the very first tick.
    async fn poll_one(&mut self, now: DateTime<Tz>) {
        let Some(job) = self.most_overdue(now) else {
            return;
        };
        // Recorded before the request: a slow or failing endpoint must wait out its
        // own interval, not be retried on every tick.
        self.last_polled.insert(job, now);
        // Borrows are split by field rather than cloning the entry: `poll_page`,
        // `poll_feed` and `poll_rss` are free functions so `cfg`, the state maps, the sources and
        // `notifier` can be held at once.
        let notifier = Heartbeating {
            inner: &self.notifier,
            path: &self.cfg.heartbeat_path,
        };
        match job {
            Job::Page(i) => {
                let page = &self.cfg.status_pages[i];
                let state = self.page_states.entry(i).or_default();
                let outcome = poll_page(&self.source, &notifier, page, state).await;
                self.stats.record(outcome);
            }
            Job::Feed(i) => {
                let feed = &self.cfg.feeds[i];
                let state = self.feed_states.entry(i).or_default();
                let detected_at = now.with_timezone(&Utc);
                let outcome =
                    poll_feed(&self.feed_source, &notifier, feed, state, detected_at).await;
                self.watch_stats.record(outcome);
            }
            Job::Rss(i) => {
                let feed = &self.cfg.rss[i];
                let state = self.rss_states.entry(i).or_default();
                let detected_at = now.with_timezone(&Utc);
                let outcome =
                    poll_rss(&self.feed_source, &notifier, feed, state, detected_at).await;
                self.rss_stats.record(outcome);
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
        if !self.cfg.rss.is_empty() {
            let stats = summary.rss;
            info!(
                window_sec,
                feeds = self.cfg.rss.len(),
                polls = stats.polls,
                not_modified = stats.not_modified,
                updated = stats.updated,
                failed = stats.failed,
                forwarded = stats.forwarded,
                send_failed = stats.send_failed,
                "rss poll summary"
            );
        }
    }

    /// Close the summary window and hand back its length and counters, or `None`
    /// while the window is still open. Split from the logging so the window
    /// arithmetic is testable without a tracing subscriber.
    fn take_due_summary(&mut self, now: DateTime<Tz>) -> Option<Summary> {
        // A reminder-only deployment has no poller to prove alive; staying silent
        // keeps this out of logs that would never contain a poll line anyway.
        if !self.cfg.has_pollers() {
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
            rss: std::mem::take(&mut self.rss_stats),
        })
    }

    /// Every pollable entry with its interval, in `Job` order.
    fn jobs(&self) -> impl Iterator<Item = (Job, Duration)> + '_ {
        let pages = (self.cfg.status_pages.iter().enumerate())
            .map(|(i, p)| (Job::Page(i), p.poll_interval));
        let feeds =
            (self.cfg.feeds.iter().enumerate()).map(|(i, f)| (Job::Feed(i), f.poll_interval));
        let rss = (self.cfg.rss.iter().enumerate()).map(|(i, r)| (Job::Rss(i), r.poll_interval));
        pages.chain(feeds).chain(rss)
    }

    fn most_overdue(&self, now: DateTime<Tz>) -> Option<Job> {
        self.jobs()
            .filter(|(job, interval)| is_due(self.last_polled.get(job), *interval, now))
            // Longest overdue first; `Job`'s `Ord` settles a tie, so the choice is
            // deterministic rather than hash-order dependent.
            .min_by_key(|(job, _)| (Reverse(overdue_secs(self.last_polled.get(job), now)), *job))
            .map(|(job, _)| job)
    }
}

/// Write the liveness heartbeat. Called at the start of every tick, before any
/// network send, so the signal is independent of Discord reachability, and again
/// after every send (see [`Heartbeating`]). A write failure is logged and ignored:
/// a persistent failure ages the mtime and the `health` subcommand fails on its
/// own, which is the detection path we want.
fn write_heartbeat(path: &Path) {
    let body = Utc::now().to_rfc3339();
    if let Err(e) = std::fs::write(path, body) {
        warn!(
            path = %path.display(),
            error = %e,
            "failed to write heartbeat"
        );
    }
}

/// A notifier that refreshes the heartbeat after every send, failed or not.
///
/// Why: one tick may send several messages, each allowed the full Discord timeout
/// — every channel of a shared Firefox feed on release day, or a burst of incident
/// updates. With the heartbeat written only at the top of the tick, a slow but
/// working Discord would age it past `2 * tick_interval` and get a loop that is
/// still making progress restarted by `chime health`.
struct Heartbeating<'a, N> {
    inner: &'a N,
    path: &'a Path,
}

impl<N: Notifier> Notifier for Heartbeating<'_, N> {
    async fn send(&self, webhook: &Url, message: &DiscordMessage) -> Result<(), NotifyError> {
        let result = self.inner.send(webhook, message).await;
        write_heartbeat(self.path);
        result
    }
}

async fn poll_page<N: Notifier, S: StatusSource>(
    source: &S,
    notifier: &N,
    page: &RunStatusPage,
    state: &mut PageState,
) -> PollOutcome {
    let fetched = match source.fetch(page, state.etag.as_deref()).await {
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
    let mut sent = Sent::default();
    for event in events {
        let message = status::build_message(page, &event);
        let result = notifier.send(&page.webhook_url, &message).await;
        sent.record(&result);
        match result {
            Ok(()) => info!(
                status_page = %page.name,
                incident = %event.incident_id,
                state = event.state.label(),
                "status update forwarded"
            ),
            Err(e) => error!(
                status_page = %page.name,
                incident = %event.incident_id,
                error = %e,
                "failed to forward status update"
            ),
        }
    }
    PollOutcome::Updated(sent)
}

async fn poll_feed<N: Notifier, B: BodySource>(
    source: &B,
    notifier: &N,
    feed: &RunFeed,
    state: &mut FeedState,
    detected_at: DateTime<Utc>,
) -> PollOutcome {
    let fetched = match source.fetch(&feed.url, state.etag(), WATCH_ACCEPT).await {
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
    let mut sent = Sent::default();
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
        let result = notifier.send(&w.webhook_url, &message).await;
        sent.record(&result);
        match result {
            Ok(()) => info!(
                watch = %w.name,
                version = %event.release.version,
                previous = %event.previous,
                "release forwarded"
            ),
            Err(e) => error!(
                watch = %w.name,
                version = %event.release.version,
                error = %e,
                "failed to forward release"
            ),
        }
    }
    if unreadable {
        PollOutcome::PartlyUnreadable(sent)
    } else {
        PollOutcome::Updated(sent)
    }
}

async fn poll_rss<N: Notifier, B: BodySource>(
    source: &B,
    notifier: &N,
    feed: &RunRss,
    state: &mut RssState,
    detected_at: DateTime<Utc>,
) -> PollOutcome {
    let fetched = match source.fetch(&feed.url, state.etag(), FEED_ACCEPT).await {
        Ok(f) => f,
        Err(e) => {
            warn!(rss = %feed.name, error = %e, "failed to poll rss feed");
            return PollOutcome::Failed;
        }
    };
    let (body, etag) = match fetched {
        Fetched::NotModified => {
            debug!(rss = %feed.name, "rss feed not modified");
            return PollOutcome::NotModified;
        }
        Fetched::Modified { value, etag } => (value, etag),
    };
    let entries = match rss::read_feed(state, &body, &feed.url, etag) {
        Ok(entries) => entries,
        Err(e) => {
            warn!(rss = %feed.name, error = %e, "failed to parse rss feed");
            return PollOutcome::Failed;
        }
    };

    let first_poll = !state.is_initialized();
    let entry_count = entries.len();
    let events = rss::diff(state, entries);
    if first_poll && state.is_initialized() {
        info!(rss = %feed.name, entries = entry_count, "rss baseline recorded");
    }
    let mut sent = Sent::default();
    for entry in events.into_iter().map(rss::FeedEntry::render) {
        let message = rss::build_message(feed, &entry, detected_at);
        let result = notifier.send(&feed.webhook_url, &message).await;
        sent.record(&result);
        match result {
            Ok(()) => info!(rss = %feed.name, entry = %entry.id, "rss entry forwarded"),
            Err(e) => error!(
                rss = %feed.name,
                entry = %entry.id,
                error = %e,
                "failed to forward rss entry"
            ),
        }
    }
    PollOutcome::Updated(sent)
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

// Wall-clock, not `DateTime<Tz>`: truncating an ambiguous local time fails in
// chrono, and the two instants of a repeated DST minute compare unequal, so either
// would send a reminder more than once in the hour that falls back.
fn truncate_to_minute(t: &DateTime<Tz>) -> NaiveDateTime {
    let local = t.naive_local();
    local
        .with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Impact, LogLevel};
    use crate::fetch::FetchError;
    use crate::runtime::{
        RunReminder, mk_run_feed, mk_run_rss, mk_run_status_page, mk_run_watch, mk_shared_feed,
    };
    use crate::status::{DecodeError, Incident, StatusError, mk_incident, mk_update};
    use chrono::TimeZone;
    use chrono_tz::Asia::Tokyo;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct CountingNotifier {
        /// Every send is still counted, then answered with an error.
        reject: bool,
        count: Arc<AtomicUsize>,
        messages: Arc<Mutex<Vec<DiscordMessage>>>,
        timestamps: Arc<Mutex<Vec<Option<String>>>>,
        titles: Arc<Mutex<Vec<String>>>,
    }

    impl CountingNotifier {
        fn new() -> Self {
            CountingNotifier {
                reject: false,
                count: Arc::new(AtomicUsize::new(0)),
                messages: Arc::new(Mutex::new(Vec::new())),
                timestamps: Arc::new(Mutex::new(Vec::new())),
                titles: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn rejecting() -> Self {
            CountingNotifier {
                reject: true,
                ..CountingNotifier::new()
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
            if let Some(embed) = message.embeds.first() {
                self.titles.lock().unwrap().push(embed.title.clone());
            }
            self.messages.lock().unwrap().push(DiscordMessage {
                content: message.content.clone(),
                username: message.username.clone(),
                avatar_url: message.avatar_url.clone(),
                embeds: Vec::new(),
            });
            if self.reject {
                return Err(NotifyError::Status {
                    status: 404,
                    body: "unknown webhook".to_string(),
                });
            }
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
    /// queue drains. `fail` makes every fetch error instead, `undecodable` makes
    /// every body fail to decode, `not_modified` makes every fetch answer 304.
    #[derive(Clone)]
    struct FakeSource {
        calls: Arc<AtomicUsize>,
        queue: Arc<Mutex<VecDeque<Vec<Incident>>>>,
        fail: bool,
        undecodable: bool,
        not_modified: bool,
    }

    impl FakeSource {
        fn empty() -> Self {
            FakeSource {
                calls: Arc::new(AtomicUsize::new(0)),
                queue: Arc::new(Mutex::new(VecDeque::new())),
                fail: false,
                undecodable: false,
                not_modified: false,
            }
        }

        fn undecodable() -> Self {
            FakeSource {
                undecodable: true,
                ..FakeSource::empty()
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
            _page: &RunStatusPage,
            _etag: Option<&str>,
        ) -> Result<Fetched<Vec<Incident>>, StatusError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(StatusError::Status {
                    status: 503,
                    body: "unavailable".to_string(),
                });
            }
            if self.undecodable {
                return Err(StatusError::Decode {
                    kind: "Atlassian Statuspage",
                    source: DecodeError::Json(
                        serde_json::from_str::<serde_json::Value>("<html>").unwrap_err(),
                    ),
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

    /// Returns the queued bodies in order, repeating the last one once the queue
    /// drains, with the same `fail` / `not_modified` switches as `FakeSource`.
    /// Every `If-None-Match` it receives is recorded in `seen_etags`.
    #[derive(Clone)]
    struct FakeBodies {
        calls: Arc<AtomicUsize>,
        queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
        seen_etags: Arc<Mutex<Vec<Option<String>>>>,
        fail: bool,
        not_modified: bool,
    }

    impl FakeBodies {
        fn empty() -> Self {
            FakeBodies {
                calls: Arc::new(AtomicUsize::new(0)),
                queue: Arc::new(Mutex::new(VecDeque::new())),
                seen_etags: Arc::new(Mutex::new(Vec::new())),
                fail: false,
                not_modified: false,
            }
        }

        fn with<B: AsRef<str>>(bodies: Vec<B>) -> Self {
            FakeBodies {
                queue: Arc::new(Mutex::new(
                    bodies
                        .into_iter()
                        .map(|b| b.as_ref().as_bytes().to_vec())
                        .collect(),
                )),
                ..FakeBodies::empty()
            }
        }

        fn failing() -> Self {
            FakeBodies {
                fail: true,
                ..FakeBodies::empty()
            }
        }

        fn unchanged() -> Self {
            FakeBodies {
                not_modified: true,
                ..FakeBodies::empty()
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn respond(&self, etag: Option<&str>) -> Result<Fetched<Vec<u8>>, FetchError> {
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

    /// Routes on `Accept`, so a test configuring both lists queues their bodies
    /// separately and can count each list's requests.
    #[derive(Clone)]
    struct FakeFeeds {
        watches: FakeBodies,
        rss: FakeBodies,
    }

    impl FakeFeeds {
        fn new(watches: &FakeBodies, rss: &FakeBodies) -> Self {
            FakeFeeds {
                watches: watches.clone(),
                rss: rss.clone(),
            }
        }
    }

    impl BodySource for FakeFeeds {
        async fn fetch(
            &self,
            _url: &Url,
            etag: Option<&str>,
            accept: &'static str,
        ) -> Result<Fetched<Vec<u8>>, FetchError> {
            match accept {
                WATCH_ACCEPT => self.watches.respond(etag),
                FEED_ACCEPT => self.rss.respond(etag),
                other => panic!("unexpected Accept: {other}"),
            }
        }
    }

    /// A scheduler whose watch and rss sources are never consulted, for tests that
    /// configure neither.
    fn sched<N: Notifier, S: StatusSource>(
        cfg: RunConfig,
        notifier: N,
        source: S,
    ) -> Scheduler<N, S, FakeFeeds> {
        sched_with_watches(cfg, notifier, source, FakeBodies::unchanged())
    }

    /// A scheduler whose rss source is never consulted.
    fn sched_with_watches<N: Notifier, S: StatusSource>(
        cfg: RunConfig,
        notifier: N,
        source: S,
        watches: FakeBodies,
    ) -> Scheduler<N, S, FakeFeeds> {
        let feeds = FakeFeeds::new(&watches, &FakeBodies::unchanged());
        Scheduler::new(cfg, notifier, source, feeds)
    }

    /// A scheduler whose status and watch sources are never consulted.
    fn sched_with_rss<N: Notifier>(
        cfg: RunConfig,
        notifier: N,
        rss: FakeBodies,
    ) -> Scheduler<N, FakeSource, FakeFeeds> {
        let feeds = FakeFeeds::new(&FakeBodies::unchanged(), &rss);
        Scheduler::new(cfg, notifier, FakeSource::empty(), feeds)
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
        cfg_with_all(tag, reminders, status_pages, feeds, Vec::new())
    }

    fn cfg_with_rss(tag: &str, rss: Vec<RunRss>) -> RunConfig {
        cfg_with_all(tag, Vec::new(), Vec::new(), Vec::new(), rss)
    }

    fn cfg_with_all(
        tag: &str,
        reminders: Vec<RunReminder>,
        status_pages: Vec<RunStatusPage>,
        feeds: Vec<RunFeed>,
        rss: Vec<RunRss>,
    ) -> RunConfig {
        RunConfig {
            log_level: LogLevel::Info,
            interval: Duration::from_secs(30),
            timezone: Tokyo,
            reminders,
            status_pages,
            feeds,
            rss,
            heartbeat_path: hb_path(tag),
        }
    }

    /// Never answers, standing in for an endpoint that hangs until its timeout.
    struct HangingSource;

    impl StatusSource for HangingSource {
        async fn fetch(
            &self,
            _page: &RunStatusPage,
            _etag: Option<&str>,
        ) -> Result<Fetched<Vec<Incident>>, StatusError> {
            std::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_interrupts_a_tick_that_is_still_running() {
        let cfg = cfg_with(
            "shutdown_mid_tick",
            vec![],
            vec![mk_run_status_page("hangs", Duration::from_secs(60))],
        );
        let scheduler = sched(cfg, CountingNotifier::new(), HangingSource);

        let stopped = tokio::time::timeout(
            Duration::from_secs(60),
            scheduler.run_until(tokio::time::sleep(Duration::from_secs(1))),
        )
        .await;

        assert!(stopped.is_ok(), "the hanging poll held up shutdown");
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

    /// 2026-11-01 is the DST fall-back day in New York: 01:00-01:59 occurs twice.
    fn new_york_fall_back(hour_utc: u32, minute: u32, second: u32) -> DateTime<Tz> {
        Utc.with_ymd_and_hms(2026, 11, 1, hour_utc, minute, second)
            .unwrap()
            .with_timezone(&chrono_tz::America::New_York)
    }

    #[tokio::test]
    async fn fires_once_within_an_ambiguous_minute() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with(
            "ambiguous_minute",
            vec![mk_run_reminder("night", 1, 30)],
            vec![],
        );
        let mut scheduler = sched(cfg, notifier.clone(), FakeSource::empty());

        // 05:30 UTC is 01:30 EDT, the first of the two 01:30s.
        scheduler.tick(new_york_fall_back(5, 30, 0)).await;
        scheduler.tick(new_york_fall_back(5, 30, 30)).await;
        scheduler.tick(new_york_fall_back(5, 30, 59)).await;

        assert_eq!(notifier.sent(), 1);
    }

    #[tokio::test]
    async fn fires_once_across_a_repeated_wall_clock_minute() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with(
            "repeated_minute",
            vec![mk_run_reminder("night", 1, 30)],
            vec![],
        );
        let mut scheduler = sched(cfg, notifier.clone(), FakeSource::empty());

        // 01:30 EDT, then 01:30 EST an hour later.
        scheduler.tick(new_york_fall_back(5, 30, 0)).await;
        scheduler.tick(new_york_fall_back(6, 30, 0)).await;

        assert_eq!(notifier.sent(), 1);
    }

    #[tokio::test]
    async fn a_failed_reminder_send_is_not_retried_in_the_same_minute() {
        let notifier = CountingNotifier::rejecting();
        let cfg = cfg_with(
            "reminder_fail",
            vec![mk_run_reminder("daily", 9, 30)],
            vec![],
        );
        let mut scheduler = sched(cfg, notifier.clone(), FakeSource::empty());

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 30, 30)).await;

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
    async fn a_body_that_fails_to_decode_leaves_the_etag_unstored() {
        // Covers the scheduler half only: an `Err` from the source must not replace
        // the stored ETag. That `StatusEndpoint` reports a decode failure as `Err`
        // is its own contract, not exercised by this fake.
        let cfg = cfg_with(
            "decode_fail",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(60))],
        );
        let mut scheduler = sched(cfg, CountingNotifier::new(), FakeSource::undecodable());
        let mut state = PageState::default();
        state.etag = Some("W/\"last-good\"".to_string());
        scheduler.page_states.insert(0, state);

        scheduler.tick(at(9, 0, 0)).await;

        assert_eq!(
            scheduler.page_states[&0].etag.as_deref(),
            Some("W/\"last-good\"")
        );
        let stats = scheduler.take_due_summary(at(10, 0, 0)).unwrap().pages;
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.updated, 0);
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
    async fn summary_is_silent_without_pages_watches_or_feeds() {
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
        let source = FakeBodies::with(vec![r#"{"version":"1"}"#]);
        let cfg = one_watch("watch_interval");
        let mut scheduler = sched_with_watches(
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
        let source = FakeBodies::with(vec![r#"{"version":"1"}"#]);
        let cfg = one_watch("watch_cold");
        let mut scheduler = sched_with_watches(cfg, notifier.clone(), FakeSource::empty(), source);

        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(notifier.sent(), 0);
    }

    #[tokio::test]
    async fn new_version_is_forwarded_as_an_embed() {
        let notifier = CountingNotifier::new();
        let source = FakeBodies::with(vec![r#"{"version":"1"}"#, r#"{"version":"2"}"#]);
        let cfg = one_watch("watch_forward");
        let mut scheduler = sched_with_watches(cfg, notifier.clone(), FakeSource::empty(), source);

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
        let source = FakeBodies::with(vec![r#"{"version":"1"}"#, r#"{"version":"2"}"#]);
        let cfg = one_watch("watch_detected_at");
        let mut scheduler = sched_with_watches(cfg, notifier.clone(), FakeSource::empty(), source);

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
        let source = FakeBodies::with(vec![r#"{"version":"1"}"#]);
        let cfg = one_watch("watch_same");
        let mut scheduler =
            sched_with_watches(cfg, notifier.clone(), FakeSource::empty(), source.clone());

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 5, 0)).await;
        scheduler.tick(at(9, 10, 0)).await;

        assert_eq!(source.calls(), 3);
        assert_eq!(notifier.sent(), 0);
    }

    #[tokio::test]
    async fn watch_poll_failure_does_not_stop_the_loop() {
        let notifier = CountingNotifier::new();
        let source = FakeBodies::failing();
        let cfg = cfg_with_watches(
            "watch_fail",
            vec![mk_run_reminder("daily", 9, 30)],
            vec![],
            vec![mk_run_feed("node", Duration::from_secs(60))],
        );
        let mut scheduler =
            sched_with_watches(cfg, notifier.clone(), FakeSource::empty(), source.clone());

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 31, 0)).await;

        assert_eq!(source.calls(), 2);
        assert_eq!(notifier.sent(), 1, "only the reminder was sent");
    }

    #[tokio::test]
    async fn extract_failure_does_not_store_the_etag() {
        let source = FakeBodies::with(vec![r#"{"nope":1}"#, r#"{"version":"1"}"#]);
        let cfg = one_watch("watch_etag");
        let mut scheduler = sched_with_watches(
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
        let watches = FakeBodies::with(vec![r#"{"version":"1"}"#]);
        let cfg = cfg_with_watches(
            "shared_budget",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(60))],
            vec![mk_run_feed("node", Duration::from_secs(60))],
        );
        let mut scheduler =
            sched_with_watches(cfg, CountingNotifier::new(), pages.clone(), watches.clone());

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
        let watches = FakeBodies::with(vec![r#"{"version":"1"}"#]);
        let pages = FakeSource::empty();
        let mut scheduler =
            sched_with_watches(cfg, CountingNotifier::new(), pages.clone(), watches.clone());
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
        let watches = FakeBodies::with(vec![r#"{"version":"1"}"#]);
        let pages = FakeSource::empty();
        let mut scheduler =
            sched_with_watches(cfg, CountingNotifier::new(), pages.clone(), watches.clone());

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
        let mut scheduler = sched_with_watches(cfg, RejectingNotifier, FakeSource::empty(), {
            // `ok` and `bad` share the fake, so its queue is consumed in poll order:
            // ok=1 (baseline), bad={} (extract failure), ok=2 (change, rejected post).
            FakeBodies::with(vec![r#"{"version":"1"}"#, r#"{}"#, r#"{"version":"2"}"#])
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
        let source = FakeBodies::unchanged();
        let cfg = one_watch("watch_summary_only");
        let mut scheduler =
            sched_with_watches(cfg, CountingNotifier::new(), FakeSource::empty(), source);

        scheduler.tick(at(9, 0, 0)).await;

        let summary = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(summary.watches.polls, 1);
        assert_eq!(summary.watches.not_modified, 1);
    }

    fn pointer_watch(name: &str, pointer: &str) -> crate::runtime::RunWatch {
        let mut w = mk_run_watch(name);
        w.extractor = crate::watch::Extractor::JsonPointer {
            pointer: pointer.to_string(),
            link: None,
        };
        w
    }

    #[tokio::test]
    async fn watches_on_one_url_share_a_single_request() {
        let notifier = CountingNotifier::new();
        let source = FakeBodies::with(vec![r#"{"a":"1","b":"1"}"#, r#"{"a":"2","b":"1"}"#]);
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
            sched_with_watches(cfg, notifier.clone(), FakeSource::empty(), source.clone());

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
        let source = FakeBodies::with(vec![r#"{"a":"1"}"#, r#"{"a":"2"}"#]);
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
            sched_with_watches(cfg, notifier.clone(), FakeSource::empty(), source.clone());

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

    /// Deletes the heartbeat on every send, standing in for a send slow enough to
    /// age it: the file exists afterwards only if something rewrote it.
    #[derive(Clone)]
    struct HeartbeatEatingNotifier {
        path: std::path::PathBuf,
    }

    impl Notifier for HeartbeatEatingNotifier {
        async fn send(&self, _webhook: &Url, _message: &DiscordMessage) -> Result<(), NotifyError> {
            let _ = std::fs::remove_file(&self.path);
            Ok(())
        }
    }

    #[tokio::test]
    async fn every_send_refreshes_the_heartbeat() {
        let path = hb_path("refresh");
        let mut cfg = cfg_with_watches(
            "refresh",
            vec![mk_run_reminder("daily", 9, 30)],
            vec![],
            vec![mk_run_feed("node", Duration::from_secs(300))],
        );
        cfg.heartbeat_path = path.clone();
        let notifier = HeartbeatEatingNotifier { path: path.clone() };
        let source = FakeBodies::with(vec![r#"{"version":"1"}"#, r#"{"version":"2"}"#]);
        let mut scheduler = sched_with_watches(cfg, notifier, FakeSource::empty(), source);

        // 09:30 sends the reminder; 09:35 sends the release.
        scheduler.tick(at(9, 30, 0)).await;
        assert!(path.exists(), "rewritten after the reminder was sent");
        scheduler.tick(at(9, 35, 0)).await;
        assert!(path.exists(), "rewritten after the release was sent");
        let _ = std::fs::remove_file(&path);
    }

    /// An RSS 2.0 body of `(guid, pubDate)` items, titled after their guid.
    fn rss_body(items: &[(&str, &str)]) -> String {
        let items: String = items
            .iter()
            .map(|(guid, date)| {
                format!(
                    "<item><title>{guid}</title><guid>{guid}</guid><pubDate>{date}</pubDate></item>"
                )
            })
            .collect();
        format!("<rss version=\"2.0\"><channel><title>t</title>{items}</channel></rss>")
    }

    const OLD: (&str, &str) = ("old", "Mon, 01 Jun 2026 00:00:00 GMT");

    fn one_rss(tag: &str) -> RunConfig {
        cfg_with_rss(tag, vec![mk_run_rss("news", Duration::from_secs(900))])
    }

    #[tokio::test]
    async fn rss_feed_is_polled_once_per_interval() {
        let source = FakeBodies::with(vec![rss_body(&[OLD])]);
        let mut scheduler = sched_with_rss(
            one_rss("rss_interval"),
            CountingNotifier::new(),
            source.clone(),
        );

        scheduler.tick(at(9, 0, 0)).await;
        assert_eq!(source.calls(), 1);
        scheduler.tick(at(9, 14, 59)).await;
        assert_eq!(source.calls(), 1);
        scheduler.tick(at(9, 15, 0)).await;
        assert_eq!(source.calls(), 2);
    }

    #[tokio::test]
    async fn first_rss_poll_does_not_forward_existing_entries() {
        let notifier = CountingNotifier::new();
        let source = FakeBodies::with(vec![rss_body(&[
            OLD,
            ("older", "Sun, 31 May 2026 00:00:00 GMT"),
        ])]);
        let mut scheduler = sched_with_rss(one_rss("rss_baseline"), notifier.clone(), source);

        scheduler.tick(at(9, 0, 0)).await;

        assert_eq!(notifier.sent(), 0);
    }

    #[tokio::test]
    async fn new_rss_entries_are_forwarded_oldest_first() {
        let notifier = CountingNotifier::new();
        let source = FakeBodies::with(vec![
            rss_body(&[OLD]),
            rss_body(&[
                ("newer", "Wed, 03 Jun 2026 00:00:00 GMT"),
                ("older", "Tue, 02 Jun 2026 00:00:00 GMT"),
                OLD,
            ]),
        ]);
        let mut scheduler = sched_with_rss(one_rss("rss_new"), notifier.clone(), source);

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 15, 0)).await;

        assert_eq!(*notifier.titles.lock().unwrap(), ["older", "newer"]);
        let sent = notifier.messages.lock().unwrap();
        assert!(
            sent.iter()
                .all(|m| m.username.as_deref() == Some("news Feed"))
        );
    }

    #[tokio::test]
    async fn an_unchanged_rss_body_posts_nothing() {
        let notifier = CountingNotifier::new();
        let source = FakeBodies::with(vec![rss_body(&[OLD])]);
        let mut scheduler =
            sched_with_rss(one_rss("rss_unchanged"), notifier.clone(), source.clone());

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 15, 0)).await;
        scheduler.tick(at(9, 30, 0)).await;

        assert_eq!(source.calls(), 3);
        assert_eq!(notifier.sent(), 0);
    }

    #[tokio::test]
    async fn rss_poll_failure_does_not_stop_the_loop() {
        let notifier = CountingNotifier::new();
        let cfg = cfg_with_all(
            "rss_failure",
            vec![mk_run_reminder("daily", 9, 15)],
            vec![],
            vec![],
            vec![mk_run_rss("news", Duration::from_secs(900))],
        );
        let source = FakeBodies::failing();
        let mut scheduler = sched_with_rss(cfg, notifier.clone(), source.clone());

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 15, 0)).await;

        assert_eq!(source.calls(), 2);
        assert_eq!(notifier.sent(), 1, "the reminder still fires");
    }

    #[tokio::test]
    async fn rss_parse_failure_keeps_the_last_good_etag_and_counts_as_failed() {
        let good = rss_body(&[OLD]);
        let source = FakeBodies::with(vec![
            good.clone(),
            "<html>maintenance</html>".to_string(),
            good,
        ]);
        let mut scheduler = sched_with_rss(
            one_rss("rss_parse_failure"),
            CountingNotifier::new(),
            source.clone(),
        );

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 15, 0)).await;
        scheduler.tick(at(9, 30, 0)).await;

        let fake = Some("W/\"fake\"".to_string());
        assert_eq!(
            *source.seen_etags.lock().unwrap(),
            [None, fake.clone(), fake]
        );
        let summary = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(summary.rss.failed, 1);
        assert_eq!(summary.rss.updated, 2);
    }

    #[tokio::test]
    async fn a_watch_outranks_an_rss_feed_on_an_exact_tie() {
        let cfg = cfg_with_all(
            "rss_tie_watch",
            vec![],
            vec![],
            vec![mk_run_feed("node", Duration::from_secs(60))],
            vec![mk_run_rss("news", Duration::from_secs(60))],
        );
        let watches = FakeBodies::with(vec![r#"{"version":"1"}"#]);
        let rss = FakeBodies::with(vec![rss_body(&[OLD])]);
        let mut scheduler = Scheduler::new(
            cfg,
            CountingNotifier::new(),
            FakeSource::empty(),
            FakeFeeds::new(&watches, &rss),
        );

        scheduler.tick(at(9, 0, 0)).await;

        assert_eq!(watches.calls(), 1);
        assert_eq!(rss.calls(), 0);
    }

    #[tokio::test]
    async fn a_page_outranks_an_rss_feed_on_an_exact_tie() {
        let cfg = cfg_with_all(
            "rss_tie_page",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(60))],
            vec![],
            vec![mk_run_rss("news", Duration::from_secs(60))],
        );
        let pages = FakeSource::empty();
        let rss = FakeBodies::with(vec![rss_body(&[OLD])]);
        let mut scheduler = Scheduler::new(
            cfg,
            CountingNotifier::new(),
            pages.clone(),
            FakeFeeds::new(&FakeBodies::unchanged(), &rss),
        );

        scheduler.tick(at(9, 0, 0)).await;

        assert_eq!(pages.calls(), 1);
        assert_eq!(rss.calls(), 0);
    }

    #[tokio::test]
    async fn pages_watches_and_feeds_share_the_one_poll_per_tick_budget() {
        let cfg = cfg_with_all(
            "rss_shared_budget",
            vec![],
            vec![mk_run_status_page("claude", Duration::from_secs(60))],
            vec![mk_run_feed("node", Duration::from_secs(60))],
            vec![mk_run_rss("news", Duration::from_secs(60))],
        );
        let pages = FakeSource::empty();
        let watches = FakeBodies::with(vec![r#"{"version":"1"}"#]);
        let rss = FakeBodies::with(vec![rss_body(&[OLD])]);
        let mut scheduler = Scheduler::new(
            cfg,
            CountingNotifier::new(),
            pages.clone(),
            FakeFeeds::new(&watches, &rss),
        );

        for (second, polls) in [(0, 1), (1, 2), (2, 3)] {
            scheduler.tick(at(9, 0, second)).await;
            assert_eq!(pages.calls() + watches.calls() + rss.calls(), polls);
        }
        assert_eq!((pages.calls(), watches.calls(), rss.calls()), (1, 1, 1));
    }

    #[tokio::test]
    async fn rss_summary_counts_every_poll_outcome() {
        let source = FakeBodies::with(vec![
            rss_body(&[OLD]),
            rss_body(&[("new", "Tue, 02 Jun 2026 00:00:00 GMT"), OLD]),
        ]);
        let mut scheduler = sched_with_rss(one_rss("rss_summary"), RejectingNotifier, source);

        scheduler.tick(at(9, 0, 0)).await;
        scheduler.tick(at(9, 15, 0)).await;

        let summary = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(summary.pages, PollStats::default());
        assert_eq!(summary.watches, PollStats::default());
        assert_eq!(
            summary.rss,
            PollStats {
                polls: 2,
                not_modified: 0,
                updated: 2,
                failed: 0,
                forwarded: 0,
                send_failed: 1,
            }
        );
    }

    #[tokio::test]
    async fn rss_summary_is_emitted_without_pages_or_watches() {
        let mut scheduler = sched_with_rss(
            one_rss("rss_summary_only"),
            CountingNotifier::new(),
            FakeBodies::unchanged(),
        );

        scheduler.tick(at(9, 0, 0)).await;

        let summary = scheduler.take_due_summary(at(10, 0, 0)).unwrap();
        assert_eq!(summary.rss.polls, 1);
        assert_eq!(summary.rss.not_modified, 1);
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
