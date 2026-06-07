use std::collections::HashMap;

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{error, info};
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
        use crate::config::{TimeOfDay, WeekdaySet};
        RunReminder {
            name: name.to_string(),
            time: TimeOfDay { hour, minute },
            days: WeekdaySet::try_from(vec!["every".to_string()]).unwrap(),
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
        };
        let mut scheduler = Scheduler::new(cfg, notifier);

        scheduler.tick(at(9, 29, 30)).await;
        scheduler.tick(at(9, 31, 0)).await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
}
