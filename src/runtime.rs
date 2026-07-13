use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Datelike, Timelike};
use chrono_tz::Tz;
use url::Url;

use crate::config::{Config, LogLevel, Message, Schedule, ScheduleError, TimeOfDay, WebhookRef};
use crate::heartbeat::heartbeat_path;

#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("env var {0} is not set")]
    Missing(String),
    #[error("failed to read file {path}: {source}")]
    File {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("webhook value is empty")]
    Empty,
    #[error(transparent)]
    Url(#[from] url::ParseError),
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("reminder `{name}`: {source}")]
    Webhook {
        name: String,
        #[source]
        source: WebhookError,
    },
    #[error("reminder `{name}`: {source}")]
    Schedule {
        name: String,
        #[source]
        source: ScheduleError,
    },
}

#[derive(Debug, Clone)]
pub struct RunReminder {
    pub name: String,
    pub time: TimeOfDay,
    pub schedule: Schedule,
    pub message: String,
    pub webhook_url: Url,
}

impl RunReminder {
    pub fn fires_at(&self, now: &DateTime<Tz>) -> bool {
        if now.hour() != self.time.hour || now.minute() != self.time.minute {
            return false;
        }
        match &self.schedule {
            Schedule::Weekly(days) => days.contains(now.weekday()),
            Schedule::Monthly(days) => days.contains(now.day()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub log_level: LogLevel,
    pub interval: Duration,
    pub timezone: Tz,
    pub reminders: Vec<RunReminder>,
    pub heartbeat_path: PathBuf,
}

pub fn resolve(cfg: Config) -> Result<RunConfig, ResolveError> {
    let mut reminders = Vec::with_capacity(cfg.reminders.len());
    for r in cfg.reminders {
        let name = r.name.as_str().to_string();
        let schedule = r.schedule().map_err(|source| ResolveError::Schedule {
            name: name.clone(),
            source,
        })?;
        let webhook_url = resolve_webhook(&r.webhook).map_err(|source| ResolveError::Webhook {
            name: name.clone(),
            source,
        })?;
        reminders.push(RunReminder {
            name,
            time: r.time,
            schedule,
            message: take_string(r.message),
            webhook_url,
        });
    }
    Ok(RunConfig {
        log_level: cfg.system.log_level,
        interval: cfg.system.tick_interval_sec.as_duration(),
        timezone: cfg.system.timezone,
        reminders,
        heartbeat_path: heartbeat_path(),
    })
}

fn take_string(m: Message) -> String {
    m.as_str().to_string()
}

fn resolve_webhook(reference: &WebhookRef) -> Result<Url, WebhookError> {
    let key = reference.env_key();
    let file_key = format!("{key}_FILE");
    let raw = match std::env::var(&file_key) {
        Ok(path) => {
            std::fs::read_to_string(&path).map_err(|source| WebhookError::File { path, source })?
        }
        Err(_) => std::env::var(&key).map_err(|_| WebhookError::Missing(key.clone()))?,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(WebhookError::Empty);
    }
    Ok(Url::parse(trimmed)?)
}

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use test_support::*;

#[cfg(test)]
mod test_support {
    use super::*;
    use crate::config::{DayOfMonthSet, Schedule, TimeOfDay, WeekdaySet};

    pub(crate) fn mk_reminder(name: &str, hour: u32, minute: u32, days: Vec<&str>) -> RunReminder {
        let days_set =
            WeekdaySet::try_from(days.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        mk_reminder_with_schedule(name, hour, minute, Schedule::Weekly(days_set))
    }

    pub(crate) fn mk_reminder_monthly(
        name: &str,
        hour: u32,
        minute: u32,
        days: Vec<i64>,
    ) -> RunReminder {
        let days_set = DayOfMonthSet::try_from(days).unwrap();
        mk_reminder_with_schedule(name, hour, minute, Schedule::Monthly(days_set))
    }

    pub(crate) fn mk_reminder_with_schedule(
        name: &str,
        hour: u32,
        minute: u32,
        schedule: Schedule,
    ) -> RunReminder {
        RunReminder {
            name: name.to_string(),
            time: TimeOfDay { hour, minute },
            schedule,
            message: format!("{name} message"),
            webhook_url: Url::parse("https://discord.example/webhook").unwrap(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Asia::Tokyo;
    use unsafe_env_guard::EnvGuard;

    fn dt(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Tz> {
        Tokyo
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn fires_at_matches_minute_and_weekday() {
        // 2026-06-05 is a Friday.
        let r = mk_reminder("daily", 9, 30, vec!["fri"]);
        assert!(r.fires_at(&dt(2026, 6, 5, 9, 30)));
        // Different minute.
        assert!(!r.fires_at(&dt(2026, 6, 5, 9, 31)));
        // Different hour.
        assert!(!r.fires_at(&dt(2026, 6, 5, 10, 30)));
        // Wrong weekday (Saturday).
        assert!(!r.fires_at(&dt(2026, 6, 6, 9, 30)));
    }

    #[test]
    fn fires_at_ignores_seconds() {
        let r = mk_reminder("daily", 9, 30, vec!["fri"]);
        let with_seconds = Tokyo
            .with_ymd_and_hms(2026, 6, 5, 9, 30, 45)
            .single()
            .unwrap();
        assert!(r.fires_at(&with_seconds));
    }

    #[test]
    fn fires_at_matches_day_of_month() {
        let r = mk_reminder_monthly("salary", 15, 0, vec![18]);
        assert!(r.fires_at(&dt(2026, 6, 18, 15, 0)));
        // Same day, different month still fires.
        assert!(r.fires_at(&dt(2026, 7, 18, 15, 0)));
        // Different day of month.
        assert!(!r.fires_at(&dt(2026, 6, 17, 15, 0)));
        // Different minute.
        assert!(!r.fires_at(&dt(2026, 6, 18, 15, 1)));
    }

    #[test]
    fn fires_at_matches_multiple_days_of_month() {
        let r = mk_reminder_monthly("payday", 9, 0, vec![1, 15, 25]);
        assert!(r.fires_at(&dt(2026, 6, 1, 9, 0)));
        assert!(r.fires_at(&dt(2026, 6, 15, 9, 0)));
        assert!(r.fires_at(&dt(2026, 6, 25, 9, 0)));
        assert!(!r.fires_at(&dt(2026, 6, 10, 9, 0)));
    }

    #[test]
    fn fires_at_skips_nonexistent_day_of_month() {
        // The 31st never occurs in June (30 days) or February, so it never fires there.
        let r = mk_reminder_monthly("month-end", 9, 0, vec![31]);
        assert!(!r.fires_at(&dt(2026, 6, 30, 9, 0)));
        assert!(!r.fires_at(&dt(2026, 2, 28, 9, 0)));
        // It still fires in a 31-day month.
        assert!(r.fires_at(&dt(2026, 7, 31, 9, 0)));
    }

    #[test]
    fn resolve_builds_monthly_schedule() {
        let _g = EnvGuard::set("CHIME_WEBHOOK_RESOLVE_MONTHLY", "https://example.com/hook");
        let toml = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "salary-day"
time = "15:00"
day_of_month = [18]
message = "Payday!"
webhook = "resolve-monthly"
"#;
        let cfg = Config::from_toml(toml).unwrap();
        let run = resolve(cfg).unwrap();
        assert!(matches!(run.reminders[0].schedule, Schedule::Monthly(_)));
    }

    #[test]
    fn resolve_reports_missing_webhook_with_reminder_name() {
        let _g1 = EnvGuard::unset("CHIME_WEBHOOK_RESOLVE_NOHOOK");
        let _g2 = EnvGuard::unset("CHIME_WEBHOOK_RESOLVE_NOHOOK_FILE");
        let toml = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "orphan"
time = "09:30"
days = ["mon"]
message = "hi"
webhook = "resolve-nohook"
"#;
        let cfg = Config::from_toml(toml).unwrap();
        assert!(matches!(
            resolve(cfg),
            Err(ResolveError::Webhook { name, .. }) if name == "orphan"
        ));
    }

    #[test]
    fn resolve_reports_schedule_error_with_reminder_name() {
        // `from_toml` normally rejects a reminder with no schedule; clearing the
        // fields afterwards exercises `resolve`'s own defensive check.
        let toml = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "orphan"
time = "09:30"
days = ["mon"]
message = "hi"
webhook = "team"
"#;
        let mut cfg = Config::from_toml(toml).unwrap();
        cfg.reminders[0].days = None;
        cfg.reminders[0].day_of_month = None;
        assert!(matches!(
            resolve(cfg),
            Err(ResolveError::Schedule { name, .. }) if name == "orphan"
        ));
    }

    #[test]
    fn env_guard_restores_previous_value_on_drop() {
        let key = "CHIME_TEST_GUARD_RESTORE";
        let _outer = EnvGuard::set(key, "original");
        {
            let _inner = EnvGuard::set(key, "override");
            assert_eq!(std::env::var(key).unwrap(), "override");
        }
        assert_eq!(std::env::var(key).unwrap(), "original");
    }

    #[test]
    fn resolve_webhook_reads_env() {
        let _g = EnvGuard::set("CHIME_WEBHOOK_TEST_DIRECT", "https://example.com/hook");
        let r = WebhookRef::try_from("test-direct".to_string()).unwrap();
        let url = resolve_webhook(&r).unwrap();
        assert_eq!(url.as_str(), "https://example.com/hook");
    }

    #[test]
    fn resolve_webhook_prefers_file() {
        let mut path = std::env::temp_dir();
        path.push(format!("chime-test-webhook-{}", std::process::id()));
        std::fs::write(&path, "https://example.com/from-file\n").unwrap();
        let _g_file = EnvGuard::set("CHIME_WEBHOOK_TEST_FILE_FILE", path.to_str().unwrap());
        let _g_direct = EnvGuard::set("CHIME_WEBHOOK_TEST_FILE", "https://example.com/direct");
        let r = WebhookRef::try_from("test-file".to_string()).unwrap();
        let url = resolve_webhook(&r).unwrap();
        assert_eq!(url.as_str(), "https://example.com/from-file");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_webhook_missing_errors() {
        let r = WebhookRef::try_from("unset-name".to_string()).unwrap();
        let _g1 = EnvGuard::unset("CHIME_WEBHOOK_UNSET_NAME");
        let _g2 = EnvGuard::unset("CHIME_WEBHOOK_UNSET_NAME_FILE");
        assert!(matches!(resolve_webhook(&r), Err(WebhookError::Missing(_))));
    }

    #[test]
    fn resolve_webhook_empty_errors() {
        let _g = EnvGuard::set("CHIME_WEBHOOK_TEST_EMPTY", "   ");
        let r = WebhookRef::try_from("test-empty".to_string()).unwrap();
        assert!(matches!(resolve_webhook(&r), Err(WebhookError::Empty)));
    }

    #[test]
    fn resolve_webhook_bad_url_errors() {
        let _g = EnvGuard::set("CHIME_WEBHOOK_TEST_BADURL", "not a url");
        let r = WebhookRef::try_from("test-badurl".to_string()).unwrap();
        assert!(matches!(resolve_webhook(&r), Err(WebhookError::Url(_))));
    }
}

#[cfg(test)]
mod unsafe_env_guard {
    pub struct EnvGuard {
        key: String,
        previous: Option<String>,
    }

    impl EnvGuard {
        pub fn set(key: &str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            EnvGuard {
                key: key.to_string(),
                previous,
            }
        }

        pub fn unset(key: &str) -> Self {
            let previous = std::env::var(key).ok();
            unsafe {
                std::env::remove_var(key);
            }
            EnvGuard {
                key: key.to_string(),
                previous,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.previous.take() {
                    Some(v) => std::env::set_var(&self.key, v),
                    None => std::env::remove_var(&self.key),
                }
            }
        }
    }
}
