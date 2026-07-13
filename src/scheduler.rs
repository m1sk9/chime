use std::collections::HashMap;

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{error, info, warn};
use url::Url;

use crate::notifier::Notifier;
use crate::runtime::RunConfig;

pub struct Scheduler<N: Notifier> {
    cfg: RunConfig,
    notifier: N,
    last_fired: HashMap<String, DateTime<Tz>>,
}

impl<N: Notifier> Scheduler<N> {
    pub fn new(cfg: RunConfig, notifier: N) -> Self {
        Scheduler {
            cfg,
            notifier,
            last_fired: HashMap::new(),
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
            match self.notifier.send(&url, &message).await {
                Ok(()) => info!(reminder = %name, "reminder fired"),
                Err(e) => error!(reminder = %name, error = %e, "failed to send reminder"),
            }
        }
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

fn truncate_to_minute(t: &DateTime<Tz>) -> DateTime<Tz> {
    t.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(*t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LogLevel;
    use crate::notifier::NotifyError;
    use crate::runtime::RunReminder;
    use chrono::TimeZone;
    use chrono_tz::Asia::Tokyo;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[derive(Clone)]
    struct CountingNotifier {
        count: Arc<AtomicUsize>,
    }

    impl Notifier for CountingNotifier {
        async fn send(&self, _webhook: &Url, _message: &str) -> Result<(), NotifyError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
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

    #[tokio::test]
    async fn fires_once_within_same_minute() {
        let count = Arc::new(AtomicUsize::new(0));
        let notifier = CountingNotifier {
            count: count.clone(),
        };
        let cfg = RunConfig {
            log_level: LogLevel::Info,
            interval: Duration::from_secs(30),
            timezone: Tokyo,
            reminders: vec![mk_run_reminder("daily", 9, 30)],
            heartbeat_path: hb_path("fires_once"),
        };
        let mut scheduler = Scheduler::new(cfg, notifier);

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 30, 30)).await;
        scheduler.tick(at(9, 30, 59)).await;

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fires_again_in_next_matching_minute() {
        let count = Arc::new(AtomicUsize::new(0));
        let notifier = CountingNotifier {
            count: count.clone(),
        };
        let cfg = RunConfig {
            log_level: LogLevel::Info,
            interval: Duration::from_secs(30),
            timezone: Tokyo,
            reminders: vec![mk_run_reminder("hourly", 9, 30)],
            heartbeat_path: hb_path("fires_again"),
        };
        let mut scheduler = Scheduler::new(cfg, notifier);

        scheduler.tick(at(9, 30, 0)).await;
        scheduler.tick(at(9, 31, 0)).await;
        // The schedule only fires at 9:30, so 9:31 does not count.
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn does_not_fire_off_schedule() {
        let count = Arc::new(AtomicUsize::new(0));
        let notifier = CountingNotifier {
            count: count.clone(),
        };
        let cfg = RunConfig {
            log_level: LogLevel::Info,
            interval: Duration::from_secs(30),
            timezone: Tokyo,
            reminders: vec![mk_run_reminder("daily", 9, 30)],
            heartbeat_path: hb_path("does_not_fire"),
        };
        let mut scheduler = Scheduler::new(cfg, notifier);

        scheduler.tick(at(9, 29, 30)).await;
        scheduler.tick(at(9, 31, 0)).await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn tick_writes_heartbeat() {
        let count = Arc::new(AtomicUsize::new(0));
        let notifier = CountingNotifier { count };
        let path = hb_path("writes");
        let _ = std::fs::remove_file(&path);
        let cfg = RunConfig {
            log_level: LogLevel::Info,
            interval: Duration::from_secs(30),
            timezone: Tokyo,
            // Off-schedule time: no reminder fires, but the heartbeat must still be written.
            reminders: vec![mk_run_reminder("daily", 9, 30)],
            heartbeat_path: path.clone(),
        };
        let mut scheduler = Scheduler::new(cfg, notifier);

        scheduler.tick(at(0, 0, 0)).await;

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(!contents.is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
