use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

use chrono::Weekday;
use chrono_tz::Tz;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error("config has no reminders")]
    NoReminders,
    #[error("duplicate reminder name: {0}")]
    DuplicateName(String),
    #[error("reminder `{name}`: {source}")]
    Schedule {
        name: String,
        #[source]
        source: ScheduleError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum TimeError {
    #[error("time must be HH:MM, got `{0}`")]
    Format(String),
    #[error("hour must be 0..=23, got {0}")]
    Hour(u32),
    #[error("minute must be 0..=59, got {0}")]
    Minute(u32),
}

#[derive(Debug, thiserror::Error)]
pub enum WeekdayError {
    #[error("days must not be empty")]
    Empty,
    #[error("unknown weekday: `{0}`")]
    Unknown(String),
}

#[derive(Debug, thiserror::Error)]
pub enum DayOfMonthError {
    #[error("day_of_month must not be empty")]
    Empty,
    #[error("day_of_month must be 1..=31, got {0}")]
    OutOfRange(i64),
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("must specify exactly one of `days` or `day_of_month`")]
    Missing,
    #[error("`days` and `day_of_month` are mutually exclusive")]
    Conflict,
}

#[derive(Debug, thiserror::Error)]
pub enum IntervalError {
    #[error("tick_interval_sec must be 1..=60, got {0}")]
    OutOfRange(i64),
}

#[derive(Debug, thiserror::Error)]
#[error("{0} must be non-empty")]
pub struct EmptyFieldError(pub &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct TimeOfDay {
    pub hour: u32,
    pub minute: u32,
}

impl FromStr for TimeOfDay {
    type Err = TimeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (h, m) = s
            .split_once(':')
            .ok_or_else(|| TimeError::Format(s.to_string()))?;
        let hour: u32 = h.parse().map_err(|_| TimeError::Format(s.to_string()))?;
        let minute: u32 = m.parse().map_err(|_| TimeError::Format(s.to_string()))?;
        if hour > 23 {
            return Err(TimeError::Hour(hour));
        }
        if minute > 59 {
            return Err(TimeError::Minute(minute));
        }
        Ok(TimeOfDay { hour, minute })
    }
}

impl TryFrom<String> for TimeOfDay {
    type Error = TimeError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Vec<String>")]
pub struct WeekdaySet(u8);

impl WeekdaySet {
    const ALL: u8 = 0b0111_1111;

    pub fn contains(self, day: Weekday) -> bool {
        (self.0 & (1u8 << day.num_days_from_monday())) != 0
    }
}

impl TryFrom<Vec<String>> for WeekdaySet {
    type Error = WeekdayError;
    fn try_from(days: Vec<String>) -> Result<Self, Self::Error> {
        if days.is_empty() {
            return Err(WeekdayError::Empty);
        }
        let mut bits: u8 = 0;
        for d in days {
            let lower = d.to_ascii_lowercase();
            if lower == "every" {
                bits = Self::ALL;
                continue;
            }
            let w = match lower.as_str() {
                "mon" => Weekday::Mon,
                "tue" => Weekday::Tue,
                "wed" => Weekday::Wed,
                "thu" => Weekday::Thu,
                "fri" => Weekday::Fri,
                "sat" => Weekday::Sat,
                "sun" => Weekday::Sun,
                _ => return Err(WeekdayError::Unknown(d)),
            };
            bits |= 1u8 << w.num_days_from_monday();
        }
        Ok(WeekdaySet(bits))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Vec<i64>")]
pub struct DayOfMonthSet(u32);

impl DayOfMonthSet {
    pub fn contains(self, day: u32) -> bool {
        (1..=31).contains(&day) && (self.0 & (1u32 << (day - 1))) != 0
    }
}

impl TryFrom<Vec<i64>> for DayOfMonthSet {
    type Error = DayOfMonthError;
    fn try_from(days: Vec<i64>) -> Result<Self, Self::Error> {
        if days.is_empty() {
            return Err(DayOfMonthError::Empty);
        }
        let mut bits: u32 = 0;
        for d in days {
            if !(1..=31).contains(&d) {
                return Err(DayOfMonthError::OutOfRange(d));
            }
            bits |= 1u32 << (d as u32 - 1);
        }
        Ok(DayOfMonthSet(bits))
    }
}

/// A reminder fires on either weekdays or days of the month, never both.
/// The exclusivity is resolved once via `Reminder::schedule` so the runtime
/// carries a value that is correct by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    Weekly(WeekdaySet),
    Monthly(DayOfMonthSet),
}

macro_rules! non_empty_str {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl $name {
            #[allow(dead_code)]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = EmptyFieldError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return Err(EmptyFieldError($label));
                }
                Ok($name(trimmed.to_string()))
            }
        }
    };
}

non_empty_str!(ReminderName, "name");
non_empty_str!(Message, "message");
non_empty_str!(WebhookRef, "webhook");

impl WebhookRef {
    pub fn env_key(&self) -> String {
        let mut s = String::from("CHIME_WEBHOOK_");
        for c in self.0.chars() {
            if c.is_ascii_alphanumeric() {
                s.push(c.to_ascii_uppercase());
            } else {
                s.push('_');
            }
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "i64")]
pub struct TickInterval(Duration);

impl TickInterval {
    pub fn as_duration(&self) -> Duration {
        self.0
    }
}

impl TryFrom<i64> for TickInterval {
    type Error = IntervalError;
    fn try_from(n: i64) -> Result<Self, Self::Error> {
        if !(1..=60).contains(&n) {
            return Err(IntervalError::OutOfRange(n));
        }
        Ok(TickInterval(Duration::from_secs(n as u64)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct System {
    #[serde(default)]
    pub log_level: LogLevel,
    pub tick_interval_sec: TickInterval,
    pub timezone: Tz,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reminder {
    pub name: ReminderName,
    pub time: TimeOfDay,
    pub days: Option<WeekdaySet>,
    pub day_of_month: Option<DayOfMonthSet>,
    pub message: Message,
    pub webhook: WebhookRef,
}

impl Reminder {
    pub fn schedule(&self) -> Result<Schedule, ScheduleError> {
        match (self.days, self.day_of_month) {
            (Some(w), None) => Ok(Schedule::Weekly(w)),
            (None, Some(d)) => Ok(Schedule::Monthly(d)),
            (None, None) => Err(ScheduleError::Missing),
            (Some(_), Some(_)) => Err(ScheduleError::Conflict),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub system: System,
    #[serde(default)]
    pub reminders: Vec<Reminder>,
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text)?;
        if cfg.reminders.is_empty() {
            return Err(ConfigError::NoReminders);
        }
        let mut seen = HashSet::new();
        for r in &cfg.reminders {
            if !seen.insert(r.name.as_str().to_string()) {
                return Err(ConfigError::DuplicateName(r.name.as_str().to_string()));
            }
            r.schedule().map_err(|source| ConfigError::Schedule {
                name: r.name.as_str().to_string(),
                source,
            })?;
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_of_day_valid() {
        assert_eq!(
            "09:30".parse::<TimeOfDay>().unwrap(),
            TimeOfDay {
                hour: 9,
                minute: 30
            }
        );
        assert_eq!(
            "00:00".parse::<TimeOfDay>().unwrap(),
            TimeOfDay { hour: 0, minute: 0 }
        );
        assert_eq!(
            "23:59".parse::<TimeOfDay>().unwrap(),
            TimeOfDay {
                hour: 23,
                minute: 59
            }
        );
    }

    #[test]
    fn time_of_day_invalid_hour() {
        assert!(matches!(
            "24:00".parse::<TimeOfDay>(),
            Err(TimeError::Hour(24))
        ));
    }

    #[test]
    fn time_of_day_invalid_minute() {
        assert!(matches!(
            "12:60".parse::<TimeOfDay>(),
            Err(TimeError::Minute(60))
        ));
    }

    #[test]
    fn time_of_day_invalid_format() {
        assert!(matches!(
            "9-30".parse::<TimeOfDay>(),
            Err(TimeError::Format(_))
        ));
        assert!(matches!(
            "ab:cd".parse::<TimeOfDay>(),
            Err(TimeError::Format(_))
        ));
    }

    #[test]
    fn weekday_set_every_expands_all() {
        let ws = WeekdaySet::try_from(vec!["every".to_string()]).unwrap();
        for d in [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ] {
            assert!(ws.contains(d), "every should contain {:?}", d);
        }
    }

    #[test]
    fn weekday_set_specific_days() {
        let ws = WeekdaySet::try_from(vec!["mon".to_string(), "fri".to_string()]).unwrap();
        assert!(ws.contains(Weekday::Mon));
        assert!(ws.contains(Weekday::Fri));
        assert!(!ws.contains(Weekday::Tue));
        assert!(!ws.contains(Weekday::Sun));
    }

    #[test]
    fn weekday_set_empty_errors() {
        assert!(matches!(
            WeekdaySet::try_from(vec![]),
            Err(WeekdayError::Empty)
        ));
    }

    #[test]
    fn weekday_set_unknown_errors() {
        assert!(matches!(
            WeekdaySet::try_from(vec!["funday".to_string()]),
            Err(WeekdayError::Unknown(_))
        ));
    }

    #[test]
    fn day_of_month_set_single_and_multiple() {
        let single = DayOfMonthSet::try_from(vec![18]).unwrap();
        assert!(single.contains(18));
        assert!(!single.contains(17));

        let multi = DayOfMonthSet::try_from(vec![1, 15, 31]).unwrap();
        assert!(multi.contains(1));
        assert!(multi.contains(15));
        assert!(multi.contains(31));
        assert!(!multi.contains(2));
        // Days outside 1..=31 are never contained.
        assert!(!multi.contains(0));
        assert!(!multi.contains(32));
    }

    #[test]
    fn day_of_month_set_empty_errors() {
        assert!(matches!(
            DayOfMonthSet::try_from(vec![]),
            Err(DayOfMonthError::Empty)
        ));
    }

    #[test]
    fn day_of_month_set_out_of_range_errors() {
        assert!(matches!(
            DayOfMonthSet::try_from(vec![0]),
            Err(DayOfMonthError::OutOfRange(0))
        ));
        assert!(matches!(
            DayOfMonthSet::try_from(vec![32]),
            Err(DayOfMonthError::OutOfRange(32))
        ));
    }

    #[test]
    fn non_empty_newtypes_reject_empty_and_whitespace() {
        assert!(ReminderName::try_from(String::new()).is_err());
        assert!(ReminderName::try_from("   ".to_string()).is_err());
        assert!(Message::try_from("\t\n".to_string()).is_err());
        assert!(WebhookRef::try_from("".to_string()).is_err());
        assert_eq!(
            ReminderName::try_from(" foo ".to_string())
                .unwrap()
                .as_str(),
            "foo"
        );
    }

    #[test]
    fn webhook_ref_env_key_normalizes() {
        let w = WebhookRef::try_from("team".to_string()).unwrap();
        assert_eq!(w.env_key(), "CHIME_WEBHOOK_TEAM");
        let w = WebhookRef::try_from("on-call".to_string()).unwrap();
        assert_eq!(w.env_key(), "CHIME_WEBHOOK_ON_CALL");
        let w = WebhookRef::try_from("team.alpha".to_string()).unwrap();
        assert_eq!(w.env_key(), "CHIME_WEBHOOK_TEAM_ALPHA");
    }

    #[test]
    fn tick_interval_bounds() {
        assert!(TickInterval::try_from(0).is_err());
        assert!(TickInterval::try_from(61).is_err());
        assert_eq!(
            TickInterval::try_from(1).unwrap().as_duration(),
            Duration::from_secs(1)
        );
        assert_eq!(
            TickInterval::try_from(60).unwrap().as_duration(),
            Duration::from_secs(60)
        );
    }

    fn good_toml() -> &'static str {
        r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "daily-standup"
time = "09:30"
days = ["mon", "tue", "wed", "thu", "fri"]
message = "Standup!"
webhook = "team"
"#
    }

    #[test]
    fn config_from_toml_ok() {
        let cfg = Config::from_toml(good_toml()).unwrap();
        assert_eq!(cfg.reminders.len(), 1);
        assert_eq!(cfg.system.log_level, LogLevel::Info);
        assert_eq!(
            cfg.system.tick_interval_sec.as_duration(),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn config_rejects_duplicate_names() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "x"
time = "09:30"
days = ["mon"]
message = "a"
webhook = "team"

[[reminders]]
name = "x"
time = "10:30"
days = ["tue"]
message = "b"
webhook = "team"
"#;
        assert!(matches!(
            Config::from_toml(t),
            Err(ConfigError::DuplicateName(_))
        ));
    }

    #[test]
    fn config_rejects_empty_reminders() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"
"#;
        assert!(matches!(
            Config::from_toml(t),
            Err(ConfigError::NoReminders)
        ));
    }

    #[test]
    fn config_rejects_unknown_weekday() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "x"
time = "09:30"
days = ["funday"]
message = "a"
webhook = "team"
"#;
        assert!(matches!(Config::from_toml(t), Err(ConfigError::Toml(_))));
    }

    #[test]
    fn config_rejects_invalid_time() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "x"
time = "24:00"
days = ["mon"]
message = "a"
webhook = "team"
"#;
        assert!(matches!(Config::from_toml(t), Err(ConfigError::Toml(_))));
    }

    #[test]
    fn config_rejects_unknown_timezone() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Mars/Olympus"

[[reminders]]
name = "x"
time = "09:30"
days = ["mon"]
message = "a"
webhook = "team"
"#;
        assert!(matches!(Config::from_toml(t), Err(ConfigError::Toml(_))));
    }

    #[test]
    fn config_rejects_unknown_field() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"
extra = "no"

[[reminders]]
name = "x"
time = "09:30"
days = ["mon"]
message = "a"
webhook = "team"
"#;
        assert!(matches!(Config::from_toml(t), Err(ConfigError::Toml(_))));
    }

    #[test]
    fn config_accepts_day_of_month() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "salary-day"
time = "15:00"
day_of_month = [18]
message = "Payday!"
webhook = "team"
"#;
        let cfg = Config::from_toml(t).unwrap();
        assert!(matches!(
            cfg.reminders[0].schedule(),
            Ok(Schedule::Monthly(_))
        ));
    }

    #[test]
    fn config_rejects_reminder_without_schedule() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "x"
time = "09:30"
message = "a"
webhook = "team"
"#;
        assert!(matches!(
            Config::from_toml(t),
            Err(ConfigError::Schedule {
                source: ScheduleError::Missing,
                ..
            })
        ));
    }

    #[test]
    fn config_rejects_reminder_with_both_schedules() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[reminders]]
name = "x"
time = "09:30"
days = ["mon"]
day_of_month = [1]
message = "a"
webhook = "team"
"#;
        assert!(matches!(
            Config::from_toml(t),
            Err(ConfigError::Schedule {
                source: ScheduleError::Conflict,
                ..
            })
        ));
    }
}
