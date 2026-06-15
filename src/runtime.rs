use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Datelike, Timelike};
use chrono_tz::Tz;
use url::Url;

use crate::config::{Config, LogLevel, Message, TimeOfDay, WebhookRef, WeekdaySet};
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
#[error("reminder `{name}`: {source}")]
pub struct ResolveError {
    pub name: String,
    #[source]
    pub source: WebhookError,
}

#[derive(Debug, Clone)]
pub struct RunReminder {
    pub name: String,
    pub time: TimeOfDay,
    pub days: WeekdaySet,
    pub message: String,
    pub webhook_url: Url,
}

impl RunReminder {
    pub fn fires_at(&self, now: &DateTime<Tz>) -> bool {
        now.hour() == self.time.hour
            && now.minute() == self.time.minute
            && self.days.contains(now.weekday())
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
        let webhook_url = resolve_webhook(&r.webhook).map_err(|source| ResolveError {
            name: name.clone(),
            source,
        })?;
        reminders.push(RunReminder {
            name,
            time: r.time,
            days: r.days,
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
    use crate::config::{TimeOfDay, WeekdaySet};

    pub(crate) fn mk_reminder(name: &str, hour: u32, minute: u32, days: Vec<&str>) -> RunReminder {
        let days_set =
            WeekdaySet::try_from(days.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        RunReminder {
            name: name.to_string(),
            time: TimeOfDay { hour, minute },
            days: days_set,
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
