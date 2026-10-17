use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

use chrono::Weekday;
use chrono_tz::Tz;
use serde::Deserialize;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error(
        "config defines none of `[[reminders]]`, `[[status_pages]]`, `[[watches]]` or `[[rss]]`"
    )]
    NoSources,
    #[error("duplicate reminder name: {0}")]
    DuplicateName(String),
    #[error("duplicate status page name: {0}")]
    DuplicateStatusPageName(String),
    #[error("duplicate watch name: {0}")]
    DuplicateWatchName(String),
    #[error("duplicate rss name: {0}")]
    DuplicateRssName(String),
    #[error("watch `{name}`: chrome has no `extended` channel on `{platform}`")]
    ChromeNoExtendedChannel {
        name: String,
        platform: &'static str,
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
#[error("reminder `{name}`: {source}")]
pub struct ReminderError {
    name: String,
    #[source]
    source: ScheduleError,
}

#[derive(Debug, thiserror::Error)]
pub enum StatusPageError {
    #[error("status page `{0}`: kind `custom` requires `mapping`")]
    CustomMappingMissing(String),
    #[error("status page `{0}`: `mapping` is only allowed with kind `custom`")]
    MappingNotCustom(String),
}

#[derive(Debug, thiserror::Error)]
pub enum IntervalError {
    #[error("tick_interval_sec must be 1..=60, got {0}")]
    OutOfRange(i64),
}

#[derive(Debug, thiserror::Error)]
pub enum PollIntervalError {
    #[error("poll_interval_sec must be 60..=3600, got {0}")]
    OutOfRange(i64),
}

#[derive(Debug, thiserror::Error)]
pub enum HttpsUrlError {
    #[error("invalid url: {0}")]
    Parse(#[from] url::ParseError),
    #[error("url scheme must be https, got `{0}`")]
    Scheme(String),
    #[error("url must have a host")]
    NoHost,
}

#[derive(Debug, thiserror::Error)]
pub enum JsonPointerError {
    #[error("pointer must start with `/` (RFC 6901), got {0:?}")]
    NotAbsolute(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ThreadIdError {
    #[error("thread_id must be a Discord snowflake (non-zero digits within 64 bits), got {0:?}")]
    NotSnowflake(String),
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
/// The exclusivity is resolved while deserializing a `Reminder`, so no reminder
/// exists without exactly one.
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
non_empty_str!(StatusPageName, "name");
non_empty_str!(DisplayName, "display_name");
non_empty_str!(WatchName, "name");
non_empty_str!(RssName, "name");

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

/// A Discord thread inside the webhook's channel.
///
/// Why a string, not an integer: a snowflake is a `u64` and a TOML integer is an
/// `i64`, and Discord itself carries snowflakes as JSON strings. A quoted value
/// keeps overflow out of the config type altogether.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct ThreadId(String);

impl ThreadId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ThreadId {
    type Error = ThreadIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        let trimmed = s.trim();
        // Why the digit check on top of `parse`: `u64::from_str` also accepts a
        // leading `+`, which Discord would reject.
        let is_snowflake = trimmed.bytes().all(|b| b.is_ascii_digit())
            && trimmed.parse::<u64>().is_ok_and(|n| n != 0);
        if !is_snowflake {
            return Err(ThreadIdError::NotSnowflake(s));
        }
        Ok(ThreadId(trimmed.to_string()))
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

/// Statuspage incident severity. Ordering is the filter contract for
/// `min_impact`, so variants are declared least- to most-severe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Impact {
    #[default]
    None,
    Maintenance,
    Minor,
    Major,
    Critical,
}

impl Impact {
    /// Why not derive this from `Deserialize`: config must reject typos, but the
    /// Statuspage API may add severities we do not know. Wire values are parsed
    /// leniently here and `None` lets the caller notify rather than drop silently.
    pub fn from_wire(s: &str) -> Option<Impact> {
        match s {
            "none" => Some(Impact::None),
            "maintenance" => Some(Impact::Maintenance),
            "minor" => Some(Impact::Minor),
            "major" => Some(Impact::Major),
            "critical" => Some(Impact::Critical),
            _ => None,
        }
    }
}

impl std::fmt::Display for Impact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Impact::None => "None",
            Impact::Maintenance => "Maintenance",
            Impact::Minor => "Minor",
            Impact::Major => "Major",
            Impact::Critical => "Critical",
        })
    }
}

fn parse_https(s: &str) -> Result<Url, HttpsUrlError> {
    let url = Url::parse(s.trim())?;
    if url.scheme() != "https" {
        return Err(HttpsUrlError::Scheme(url.scheme().to_string()));
    }
    if url.host_str().is_none() {
        return Err(HttpsUrlError::NoHost);
    }
    Ok(url)
}

/// Any https URL, kept verbatim: avatars, watch sources, watch links and status
/// pages. Why not normalize a trailing `/` here for `Url::join`: that would turn
/// `https://x/api/v.json` into `.../v.json/`, so it is done where a path is joined
/// (`status::join_base`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct HttpsUrl(Url);

impl HttpsUrl {
    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0.into()
    }
}

impl TryFrom<String> for HttpsUrl {
    type Error = HttpsUrlError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        parse_https(&s).map(HttpsUrl)
    }
}

/// RFC 6901 pointer that must start with `/`.
///
/// Why not accept the empty pointer, which RFC 6901 allows: it addresses the whole
/// document, useful only for an endpoint whose body is a bare string or number, and
/// that is rare. A pointer without a leading `/` is far more often a typo
/// (`version` for `/version`), and rejecting it here surfaces the typo at startup
/// instead of as a warning on every poll.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct JsonPointer(String);

impl JsonPointer {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for JsonPointer {
    type Error = JsonPointerError;
    // Why not trim like the other string types: in RFC 6901 whitespace is part of
    // a reference token, so `/v ` and `/v` address different keys.
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if !s.starts_with('/') {
            return Err(JsonPointerError::NotAbsolute(s));
        }
        Ok(JsonPointer(s))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "i64")]
pub struct PollInterval(Duration);

impl PollInterval {
    pub fn as_duration(&self) -> Duration {
        self.0
    }
}

impl TryFrom<i64> for PollInterval {
    type Error = PollIntervalError;
    fn try_from(n: i64) -> Result<Self, Self::Error> {
        // The floor is politeness, not a protocol limit: chime polls a third-party
        // public endpoint indefinitely and has no business hammering it.
        if !(60..=3600).contains(&n) {
            return Err(PollIntervalError::OutOfRange(n));
        }
        Ok(PollInterval(Duration::from_secs(n as u64)))
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
#[serde(try_from = "RawReminder")]
pub struct Reminder {
    pub name: ReminderName,
    pub time: TimeOfDay,
    pub schedule: Schedule,
    pub message: Message,
    pub webhook: WebhookRef,
    pub thread_id: Option<ThreadId>,
}

/// The TOML shape of a `Reminder`: `days` and `day_of_month` are separate keys.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawReminder {
    name: ReminderName,
    time: TimeOfDay,
    days: Option<WeekdaySet>,
    day_of_month: Option<DayOfMonthSet>,
    message: Message,
    webhook: WebhookRef,
    thread_id: Option<ThreadId>,
}

impl TryFrom<RawReminder> for Reminder {
    type Error = ReminderError;
    fn try_from(r: RawReminder) -> Result<Self, Self::Error> {
        let schedule = match (r.days, r.day_of_month) {
            (Some(w), None) => Ok(Schedule::Weekly(w)),
            (None, Some(d)) => Ok(Schedule::Monthly(d)),
            (None, None) => Err(ScheduleError::Missing),
            (Some(_), Some(_)) => Err(ScheduleError::Conflict),
        }
        .map_err(|source| ReminderError {
            name: r.name.as_str().to_string(),
            source,
        })?;
        Ok(Reminder {
            name: r.name,
            time: r.time,
            schedule,
            message: r.message,
            webhook: r.webhook,
            thread_id: r.thread_id,
        })
    }
}

fn default_poll_interval() -> PollInterval {
    PollInterval(Duration::from_secs(300))
}

/// Which status page software serves `url`, and so which document is fetched.
// Why a flat unit enum rather than a nested `source` table like `WatchKind`: a unit
// variant needs no `flatten`, so `deny_unknown_fields` still holds, and existing
// configs keep their top-level `url` unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum StatusKind {
    #[default]
    Statuspage,
    Instatus,
    Incidentio,
    Datadog,
    Custom,
}

/// Where the fields of a `custom` status page live. `incidents` addresses the
/// incident array in the document; every other pointer is relative to one element.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomMapping {
    /// Absent means the document itself is the array.
    pub incidents: Option<JsonPointer>,
    pub id: JsonPointer,
    pub title: JsonPointer,
    pub status: Option<JsonPointer>,
    pub body: Option<JsonPointer>,
    pub updated_at: Option<JsonPointer>,
    pub link: Option<JsonPointer>,
    pub impact: Option<JsonPointer>,
}

/// The software serving a status page, with what that software needs: only
/// `custom` carries a mapping, so no page can be `custom` without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusProvider {
    Statuspage,
    Instatus,
    Incidentio,
    Datadog,
    Custom(CustomMapping),
}

/// A status page to watch. Incidents are pulled from a public document chosen by
/// `kind`; chime never receives an inbound webhook.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawStatusPage")]
pub struct StatusPage {
    pub name: StatusPageName,
    pub provider: StatusProvider,
    pub url: HttpsUrl,
    pub webhook: WebhookRef,
    pub thread_id: Option<ThreadId>,
    pub poll_interval_sec: PollInterval,
    pub min_impact: Impact,
    pub display_name: Option<DisplayName>,
    pub avatar_url: Option<HttpsUrl>,
}

/// The TOML shape of a `StatusPage`: `kind` and `mapping` are separate keys.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStatusPage {
    name: StatusPageName,
    #[serde(default)]
    kind: StatusKind,
    url: HttpsUrl,
    mapping: Option<CustomMapping>,
    webhook: WebhookRef,
    thread_id: Option<ThreadId>,
    #[serde(default = "default_poll_interval")]
    poll_interval_sec: PollInterval,
    #[serde(default)]
    min_impact: Impact,
    display_name: Option<DisplayName>,
    avatar_url: Option<HttpsUrl>,
}

impl TryFrom<RawStatusPage> for StatusPage {
    type Error = StatusPageError;
    fn try_from(p: RawStatusPage) -> Result<Self, Self::Error> {
        let provider = match (p.kind, p.mapping) {
            (StatusKind::Statuspage, None) => StatusProvider::Statuspage,
            (StatusKind::Instatus, None) => StatusProvider::Instatus,
            (StatusKind::Incidentio, None) => StatusProvider::Incidentio,
            (StatusKind::Datadog, None) => StatusProvider::Datadog,
            (StatusKind::Custom, Some(mapping)) => StatusProvider::Custom(mapping),
            (StatusKind::Custom, None) => {
                return Err(StatusPageError::CustomMappingMissing(
                    p.name.as_str().to_string(),
                ));
            }
            (_, Some(_)) => {
                return Err(StatusPageError::MappingNotCustom(
                    p.name.as_str().to_string(),
                ));
            }
        };
        Ok(StatusPage {
            name: p.name,
            provider,
            url: p.url,
            webhook: p.webhook,
            thread_id: p.thread_id,
            poll_interval_sec: p.poll_interval_sec,
            min_impact: p.min_impact,
            display_name: p.display_name,
            avatar_url: p.avatar_url,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FirefoxChannel {
    #[default]
    Release,
    Esr,
    Beta,
    Devedition,
    Nightly,
}

impl FirefoxChannel {
    pub fn label(&self) -> &'static str {
        match self {
            FirefoxChannel::Release => "Release",
            FirefoxChannel::Esr => "ESR",
            FirefoxChannel::Beta => "Beta",
            FirefoxChannel::Devedition => "Developer Edition",
            FirefoxChannel::Nightly => "Nightly",
        }
    }
}

// `snake_case` rather than `lowercase`: the API spells one platform `mac_arm64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChromePlatform {
    #[default]
    Win,
    Win64,
    Mac,
    MacArm64,
    Linux,
    Android,
    Webview,
    Ios,
}

impl ChromePlatform {
    pub fn as_api_str(&self) -> &'static str {
        match self {
            ChromePlatform::Win => "win",
            ChromePlatform::Win64 => "win64",
            ChromePlatform::Mac => "mac",
            ChromePlatform::MacArm64 => "mac_arm64",
            ChromePlatform::Linux => "linux",
            ChromePlatform::Android => "android",
            ChromePlatform::Webview => "webview",
            ChromePlatform::Ios => "ios",
        }
    }

    // Why not leave this to the first poll: VersionHistory answers 400 for
    // `extended` on every other platform (measured 2026-09-27), so such a watch can
    // never succeed and has to fail at startup like any other config error.
    pub fn has_extended_channel(&self) -> bool {
        matches!(
            self,
            ChromePlatform::Win
                | ChromePlatform::Win64
                | ChromePlatform::Mac
                | ChromePlatform::MacArm64
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChromeChannel {
    #[default]
    Stable,
    Extended,
    Beta,
    Dev,
    Canary,
}

impl ChromeChannel {
    pub fn as_api_str(&self) -> &'static str {
        match self {
            ChromeChannel::Stable => "stable",
            ChromeChannel::Extended => "extended",
            ChromeChannel::Beta => "beta",
            ChromeChannel::Dev => "dev",
            ChromeChannel::Canary => "canary",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ChromeChannel::Stable => "Stable",
            ChromeChannel::Extended => "Extended Stable",
            ChromeChannel::Beta => "Beta",
            ChromeChannel::Dev => "Dev",
            ChromeChannel::Canary => "Canary",
        }
    }
}

/// Which served version counts as "released": the top version as soon as its
/// staged rollout starts, or only once it reaches every user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChromeRollout {
    #[default]
    Started,
    Complete,
}

// Why a nested `source` table rather than a flat `kind` key on `Watch`: sharing a
// table between the common fields and a kind-specific enum needs `#[serde(flatten)]`,
// which silently disables `deny_unknown_fields`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum WatchKind {
    Firefox {
        #[serde(default)]
        channel: FirefoxChannel,
    },
    Chrome {
        #[serde(default)]
        platform: ChromePlatform,
        #[serde(default)]
        channel: ChromeChannel,
        #[serde(default)]
        rollout: ChromeRollout,
    },
    Json {
        url: HttpsUrl,
        pointer: JsonPointer,
        link: Option<HttpsUrl>,
    },
}

fn default_watch_poll_interval() -> PollInterval {
    PollInterval(Duration::from_secs(3600))
}

/// A release feed to watch. Like a status page it is pulled, and only a change of
/// the extracted version is forwarded.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    pub name: WatchName,
    pub webhook: WebhookRef,
    pub thread_id: Option<ThreadId>,
    pub source: WatchKind,
    #[serde(default = "default_watch_poll_interval")]
    pub poll_interval_sec: PollInterval,
    pub display_name: Option<DisplayName>,
    pub avatar_url: Option<HttpsUrl>,
}

fn default_rss_poll_interval() -> PollInterval {
    PollInterval(Duration::from_secs(900))
}

/// A syndication feed to subscribe to: RSS, Atom or JSON Feed. Each entry not seen
/// before is forwarded once; the format is detected from the body.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rss {
    pub name: RssName,
    pub url: HttpsUrl,
    pub webhook: WebhookRef,
    pub thread_id: Option<ThreadId>,
    #[serde(default = "default_rss_poll_interval")]
    pub poll_interval_sec: PollInterval,
    pub display_name: Option<DisplayName>,
    pub avatar_url: Option<HttpsUrl>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub system: System,
    #[serde(default)]
    pub reminders: Vec<Reminder>,
    #[serde(default)]
    pub status_pages: Vec<StatusPage>,
    #[serde(default)]
    pub watches: Vec<Watch>,
    #[serde(default)]
    pub rss: Vec<Rss>,
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text)?;
        if cfg.reminders.is_empty()
            && cfg.status_pages.is_empty()
            && cfg.watches.is_empty()
            && cfg.rss.is_empty()
        {
            return Err(ConfigError::NoSources);
        }
        let mut seen = HashSet::new();
        for r in &cfg.reminders {
            if !seen.insert(r.name.as_str()) {
                return Err(ConfigError::DuplicateName(r.name.as_str().to_string()));
            }
        }
        let mut seen_pages = HashSet::new();
        for p in &cfg.status_pages {
            if !seen_pages.insert(p.name.as_str()) {
                return Err(ConfigError::DuplicateStatusPageName(
                    p.name.as_str().to_string(),
                ));
            }
        }
        let mut seen_watches = HashSet::new();
        for w in &cfg.watches {
            if !seen_watches.insert(w.name.as_str()) {
                return Err(ConfigError::DuplicateWatchName(w.name.as_str().to_string()));
            }
            if let WatchKind::Chrome {
                platform,
                channel: ChromeChannel::Extended,
                ..
            } = w.source
                && !platform.has_extended_channel()
            {
                return Err(ConfigError::ChromeNoExtendedChannel {
                    name: w.name.as_str().to_string(),
                    platform: platform.as_api_str(),
                });
            }
        }
        let mut seen_rss = HashSet::new();
        for r in &cfg.rss {
            if !seen_rss.insert(r.name.as_str()) {
                return Err(ConfigError::DuplicateRssName(r.name.as_str().to_string()));
            }
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
    fn non_empty_newtypes_display_inner_value() {
        let n = ReminderName::try_from("standup".to_string()).unwrap();
        assert_eq!(n.to_string(), "standup");
        let m = Message::try_from("hello".to_string()).unwrap();
        assert_eq!(format!("{m}"), "hello");
        let w = WebhookRef::try_from("team".to_string()).unwrap();
        assert_eq!(w.to_string(), "team");
    }

    #[test]
    fn non_empty_newtypes_reject_empty_and_whitespace() {
        assert!(ReminderName::try_from(String::new()).is_err());
        assert!(ReminderName::try_from("   ".to_string()).is_err());
        assert!(Message::try_from("\t\n".to_string()).is_err());
        assert!(WebhookRef::try_from("".to_string()).is_err());
        assert!(RssName::try_from(" ".to_string()).is_err());
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
    fn config_rejects_a_config_with_no_sources() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"
"#;
        assert!(matches!(Config::from_toml(t), Err(ConfigError::NoSources)));
    }

    #[test]
    fn config_accepts_status_pages_without_reminders() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[status_pages]]
name = "claude"
url = "https://status.claude.com"
webhook = "team"
"#;
        let cfg = Config::from_toml(t).unwrap();
        assert!(cfg.reminders.is_empty());
        assert_eq!(cfg.status_pages.len(), 1);
    }

    #[test]
    fn status_page_defaults_are_applied() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[status_pages]]
name = "claude"
url = "https://status.claude.com"
webhook = "team"
"#;
        let page = Config::from_toml(t).unwrap().status_pages.remove(0);
        assert_eq!(
            page.poll_interval_sec.as_duration(),
            Duration::from_secs(300)
        );
        assert_eq!(page.min_impact, Impact::None);
        assert!(page.display_name.is_none());
        assert!(page.avatar_url.is_none());
    }

    #[test]
    fn status_page_accepts_full_form() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[status_pages]]
name = "claude"
url = "https://status.claude.com"
webhook = "team"
poll_interval_sec = 120
min_impact = "major"
display_name = "Claude Status"
avatar_url = "https://example.com/claude.png"
"#;
        let page = Config::from_toml(t).unwrap().status_pages.remove(0);
        assert_eq!(
            page.poll_interval_sec.as_duration(),
            Duration::from_secs(120)
        );
        assert_eq!(page.min_impact, Impact::Major);
        assert_eq!(page.display_name.unwrap().as_str(), "Claude Status");
    }

    #[test]
    fn config_rejects_duplicate_status_page_names() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[status_pages]]
name = "dup"
url = "https://status.claude.com"
webhook = "team"

[[status_pages]]
name = "dup"
url = "https://status.proton.me"
webhook = "team"
"#;
        assert!(matches!(
            Config::from_toml(t),
            Err(ConfigError::DuplicateStatusPageName(_))
        ));
    }

    #[test]
    fn status_page_url_requires_https_and_is_kept_verbatim() {
        let r = Config::from_toml(&format!(
            "{SYSTEM}[[status_pages]]\nname = \"x\"\nurl = \"http://status.claude.com\"\nwebhook = \"team\"\n"
        ));
        assert!(matches!(r, Err(ConfigError::Toml(_))));

        // The trailing `/` that `Url::join` needs is added per kind at resolve time,
        // so a `custom` URL naming a file is not turned into a directory.
        let page = first_status_page(
            r#"
[[status_pages]]
name = "x"
url = "https://example.com/status"
webhook = "team"
"#,
        );
        assert_eq!(page.url.as_url().as_str(), "https://example.com/status");
    }

    #[test]
    fn poll_interval_bounds() {
        assert!(PollInterval::try_from(59).is_err());
        assert!(PollInterval::try_from(3601).is_err());
        assert_eq!(
            PollInterval::try_from(60).unwrap().as_duration(),
            Duration::from_secs(60)
        );
        assert_eq!(
            PollInterval::try_from(3600).unwrap().as_duration(),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn impact_orders_least_to_most_severe() {
        assert!(Impact::None < Impact::Maintenance);
        assert!(Impact::Maintenance < Impact::Minor);
        assert!(Impact::Minor < Impact::Major);
        assert!(Impact::Major < Impact::Critical);
    }

    #[test]
    fn impact_from_wire_is_lenient_but_config_is_strict() {
        assert_eq!(Impact::from_wire("critical"), Some(Impact::Critical));
        assert_eq!(Impact::from_wire("catastrophic"), None);

        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[status_pages]]
name = "claude"
url = "https://status.claude.com"
webhook = "team"
min_impact = "catastrophic"
"#;
        assert!(matches!(Config::from_toml(t), Err(ConfigError::Toml(_))));
    }

    #[test]
    fn config_rejects_unknown_status_page_field() {
        let t = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"

[[status_pages]]
name = "claude"
url = "https://status.claude.com"
webhook = "team"
provider = "statuspage"
"#;
        assert!(matches!(Config::from_toml(t), Err(ConfigError::Toml(_))));
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
        assert!(matches!(cfg.reminders[0].schedule, Schedule::Monthly(_)));
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
        let err = Config::from_toml(t).unwrap_err().to_string();
        assert!(
            err.contains("reminder `x`: must specify exactly one of `days` or `day_of_month`"),
            "{err}"
        );
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
        let err = Config::from_toml(t).unwrap_err().to_string();
        assert!(
            err.contains("reminder `x`: `days` and `day_of_month` are mutually exclusive"),
            "{err}"
        );
    }

    const SYSTEM: &str = r#"
[system]
tick_interval_sec = 30
timezone = "Asia/Tokyo"
"#;

    fn parse_watches(body: &str) -> Result<Config, ConfigError> {
        Config::from_toml(&format!("{SYSTEM}{body}"))
    }

    fn first_watch(body: &str) -> Watch {
        parse_watches(body).unwrap().watches.remove(0)
    }

    #[test]
    fn watch_defaults_are_applied() {
        let w = first_watch(
            r#"
[[watches]]
name = "firefox"
webhook = "team"
source = { kind = "firefox" }
"#,
        );
        assert_eq!(
            w.source,
            WatchKind::Firefox {
                channel: FirefoxChannel::Release
            }
        );
        assert_eq!(w.poll_interval_sec.as_duration(), Duration::from_secs(3600));
        assert!(w.display_name.is_none());
        assert!(w.avatar_url.is_none());
    }

    #[test]
    fn watch_accepts_full_form() {
        let w = first_watch(
            r#"
[[watches]]
name = "chrome"
webhook = "team"
source = { kind = "chrome", platform = "mac_arm64", channel = "beta", rollout = "complete" }
poll_interval_sec = 600
display_name = "Chrome Releases"
avatar_url = "https://example.com/chrome.png"
"#,
        );
        assert_eq!(
            w.source,
            WatchKind::Chrome {
                platform: ChromePlatform::MacArm64,
                channel: ChromeChannel::Beta,
                rollout: ChromeRollout::Complete,
            }
        );
        assert_eq!(w.poll_interval_sec.as_duration(), Duration::from_secs(600));
        assert_eq!(w.display_name.unwrap().as_str(), "Chrome Releases");
        assert!(w.avatar_url.is_some());
    }

    #[test]
    fn watch_source_accepts_inline_and_sub_table_forms() {
        let inline = first_watch(
            r#"
[[watches]]
name = "esr"
webhook = "team"
source = { kind = "firefox", channel = "esr" }
"#,
        );
        let sub_table = first_watch(
            r#"
[[watches]]
name = "esr"
webhook = "team"

[watches.source]
kind = "firefox"
channel = "esr"
"#,
        );
        assert_eq!(inline.source, sub_table.source);
        assert_eq!(
            inline.source,
            WatchKind::Firefox {
                channel: FirefoxChannel::Esr
            }
        );
    }

    #[test]
    fn watch_source_kind_is_required() {
        let r = parse_watches(
            r#"
[[watches]]
name = "x"
webhook = "team"
source = { channel = "esr" }
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn watch_source_rejects_an_unknown_kind() {
        let r = parse_watches(
            r#"
[[watches]]
name = "x"
webhook = "team"
source = { kind = "safari" }
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn watch_source_rejects_an_unknown_field_inside_a_kind() {
        let r = parse_watches(
            r#"
[[watches]]
name = "x"
webhook = "team"
source = { kind = "firefox", foo = 1 }
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));

        let r = parse_watches(
            r#"
[[watches]]
name = "x"
webhook = "team"

[watches.source]
kind = "firefox"
foo = 1
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn watch_source_rejects_a_field_that_belongs_to_another_kind() {
        let r = parse_watches(
            r#"
[[watches]]
name = "x"
webhook = "team"
source = { kind = "firefox", platform = "win" }
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn config_rejects_unknown_watch_field() {
        let r = parse_watches(
            r#"
[[watches]]
name = "x"
webhook = "team"
source = { kind = "firefox" }
min_impact = "major"
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn json_watch_requires_url_and_pointer() {
        for source in [
            r#"{ kind = "json", url = "https://nodejs.org/dist/index.json" }"#,
            r#"{ kind = "json", pointer = "/0/version" }"#,
        ] {
            let r = parse_watches(&format!(
                "[[watches]]\nname = \"node\"\nwebhook = \"team\"\nsource = {source}\n"
            ));
            assert!(matches!(r, Err(ConfigError::Toml(_))), "{source}");
        }
    }

    #[test]
    fn json_pointer_must_start_with_a_slash() {
        assert!(matches!(
            JsonPointer::try_from(String::new()),
            Err(JsonPointerError::NotAbsolute(_))
        ));
        assert!(matches!(
            JsonPointer::try_from("foo".to_string()),
            Err(JsonPointerError::NotAbsolute(_))
        ));
        assert!(matches!(
            JsonPointer::try_from(" /a".to_string()),
            Err(JsonPointerError::NotAbsolute(_))
        ));
    }

    #[test]
    fn json_pointer_keeps_whitespace_that_belongs_to_a_key() {
        assert_eq!(
            JsonPointer::try_from("/v ".to_string()).unwrap().as_str(),
            "/v "
        );
    }

    #[test]
    fn json_watch_url_requires_https_and_is_not_slash_normalized() {
        for bad in ["http://x.example/api/v.json", "ftp://x.example/v.json"] {
            assert!(
                matches!(
                    HttpsUrl::try_from(bad.to_string()),
                    Err(HttpsUrlError::Scheme(_))
                ),
                "{bad}"
            );
        }
        assert!(HttpsUrl::try_from("not a url".to_string()).is_err());
        let w = first_watch(
            r#"
[[watches]]
name = "node"
webhook = "team"
source = { kind = "json", url = "https://x.example/api/v.json", pointer = "/version", link = "https://x.example/notes" }
"#,
        );
        let WatchKind::Json { url, pointer, link } = w.source else {
            panic!("expected a json watch");
        };
        assert_eq!(url.as_url().as_str(), "https://x.example/api/v.json");
        assert_eq!(pointer.as_str(), "/version");
        assert_eq!(link.unwrap().as_url().as_str(), "https://x.example/notes");

        let r = parse_watches(
            r#"
[[watches]]
name = "node"
webhook = "team"
source = { kind = "json", url = "https://x.example/v.json", pointer = "/v", link = "http://x.example/" }
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn chrome_platform_names_match_the_api_path() {
        for (name, platform) in [
            ("win", ChromePlatform::Win),
            ("win64", ChromePlatform::Win64),
            ("mac", ChromePlatform::Mac),
            ("mac_arm64", ChromePlatform::MacArm64),
            ("linux", ChromePlatform::Linux),
            ("android", ChromePlatform::Android),
            ("webview", ChromePlatform::Webview),
            ("ios", ChromePlatform::Ios),
        ] {
            let w = first_watch(&format!(
                "[[watches]]\nname = \"c\"\nwebhook = \"team\"\nsource = {{ kind = \"chrome\", platform = \"{name}\" }}\n"
            ));
            let WatchKind::Chrome {
                platform: parsed, ..
            } = w.source
            else {
                panic!("expected a chrome watch");
            };
            assert_eq!(parsed, platform);
            assert_eq!(parsed.as_api_str(), name);
        }
    }

    #[test]
    fn firefox_and_chrome_enums_reject_typos() {
        for source in [
            r#"{ kind = "firefox", channel = "stable" }"#,
            r#"{ kind = "chrome", platform = "macos" }"#,
            r#"{ kind = "chrome", channel = "release" }"#,
            r#"{ kind = "chrome", rollout = "done" }"#,
        ] {
            let r = parse_watches(&format!(
                "[[watches]]\nname = \"x\"\nwebhook = \"team\"\nsource = {source}\n"
            ));
            assert!(matches!(r, Err(ConfigError::Toml(_))), "{source}");
        }
    }

    #[test]
    fn config_accepts_watches_without_reminders_or_status_pages() {
        let cfg = parse_watches(
            r#"
[[watches]]
name = "firefox"
webhook = "team"
source = { kind = "firefox" }
"#,
        )
        .unwrap();
        assert!(cfg.reminders.is_empty());
        assert!(cfg.status_pages.is_empty());
        assert_eq!(cfg.watches.len(), 1);
    }

    #[test]
    fn config_rejects_duplicate_watch_names() {
        let r = parse_watches(
            r#"
[[watches]]
name = "dup"
webhook = "team"
source = { kind = "firefox" }

[[watches]]
name = "dup"
webhook = "team"
source = { kind = "chrome" }
"#,
        );
        assert!(matches!(r, Err(ConfigError::DuplicateWatchName(_))));
    }

    #[test]
    fn chrome_extended_is_rejected_on_platforms_without_it() {
        for platform in ["linux", "android", "webview", "ios"] {
            let r = parse_watches(&format!(
                "[[watches]]\nname = \"c\"\nwebhook = \"team\"\nsource = {{ kind = \"chrome\", platform = \"{platform}\", channel = \"extended\" }}\n"
            ));
            assert!(
                matches!(
                    &r,
                    Err(ConfigError::ChromeNoExtendedChannel { name, platform: p })
                        if name == "c" && *p == platform
                ),
                "{platform}: {r:?}"
            );
        }
        for platform in ["win", "win64", "mac", "mac_arm64"] {
            let r = parse_watches(&format!(
                "[[watches]]\nname = \"c\"\nwebhook = \"team\"\nsource = {{ kind = \"chrome\", platform = \"{platform}\", channel = \"extended\" }}\n"
            ));
            assert!(r.is_ok(), "{platform}: {r:?}");
        }
    }

    fn parse_rss(body: &str) -> Result<Config, ConfigError> {
        Config::from_toml(&format!("{SYSTEM}{body}"))
    }

    fn first_rss(body: &str) -> Rss {
        parse_rss(body).unwrap().rss.remove(0)
    }

    #[test]
    fn rss_defaults_are_applied() {
        let r = first_rss(
            r#"
[[rss]]
name = "changelog"
url = "https://code.claude.com/docs/en/changelog/rss.xml"
webhook = "team"
"#,
        );
        assert_eq!(r.poll_interval_sec.as_duration(), Duration::from_secs(900));
        assert!(r.display_name.is_none());
        assert!(r.avatar_url.is_none());
    }

    #[test]
    fn rss_accepts_full_form() {
        let r = first_rss(
            r#"
[[rss]]
name = "changelog"
url = "https://code.claude.com/docs/en/changelog/rss.xml"
webhook = "team"
poll_interval_sec = 600
display_name = "Claude Code Changelog"
avatar_url = "https://example.com/claude.png"
"#,
        );
        assert_eq!(r.name.as_str(), "changelog");
        assert_eq!(
            r.url.as_url().as_str(),
            "https://code.claude.com/docs/en/changelog/rss.xml"
        );
        assert_eq!(r.webhook.as_str(), "team");
        assert_eq!(r.poll_interval_sec.as_duration(), Duration::from_secs(600));
        assert_eq!(r.display_name.unwrap().as_str(), "Claude Code Changelog");
        assert_eq!(
            r.avatar_url.unwrap().into_string(),
            "https://example.com/claude.png"
        );
    }

    #[test]
    fn rss_url_requires_https() {
        let r = parse_rss(
            r#"
[[rss]]
name = "changelog"
url = "http://example.com/rss.xml"
webhook = "team"
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn config_rejects_unknown_rss_field() {
        let r = parse_rss(
            r#"
[[rss]]
name = "changelog"
url = "https://example.com/rss.xml"
webhook = "team"
pointer = "/x"
"#,
        );
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn config_accepts_rss_without_reminders_status_pages_or_watches() {
        let cfg = parse_rss(
            r#"
[[rss]]
name = "changelog"
url = "https://example.com/rss.xml"
webhook = "team"
"#,
        )
        .unwrap();
        assert!(cfg.reminders.is_empty());
        assert!(cfg.status_pages.is_empty());
        assert!(cfg.watches.is_empty());
        assert_eq!(cfg.rss.len(), 1);
    }

    #[test]
    fn config_rejects_duplicate_rss_names() {
        let r = parse_rss(
            r#"
[[rss]]
name = "dup"
url = "https://a.example/rss.xml"
webhook = "team"

[[rss]]
name = "dup"
url = "https://b.example/rss.xml"
webhook = "team"
"#,
        );
        assert!(matches!(r, Err(ConfigError::DuplicateRssName(n)) if n == "dup"));
    }

    #[test]
    fn an_rss_feed_and_a_watch_may_share_a_name() {
        let cfg = parse_rss(
            r#"
[[watches]]
name = "firefox"
webhook = "team"
source = { kind = "firefox" }

[[rss]]
name = "firefox"
url = "https://blog.mozilla.org/feed/"
webhook = "team"
"#,
        )
        .unwrap();
        assert_eq!(cfg.watches.len(), 1);
        assert_eq!(cfg.rss.len(), 1);
    }

    #[test]
    fn thread_id_accepts_a_digit_string() {
        let id = ThreadId::try_from(" 1234567890123456789 ".to_string()).unwrap();
        assert_eq!(id.as_str(), "1234567890123456789");
    }

    #[test]
    fn thread_id_rejects_empty_and_non_digits() {
        for bad in ["", " ", "abc", "12 34", "-1", "+1", "12a"] {
            assert!(
                matches!(
                    ThreadId::try_from(bad.to_string()),
                    Err(ThreadIdError::NotSnowflake(_))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn thread_id_rejects_zero_and_values_beyond_u64() {
        for bad in [
            "0",
            "18446744073709551616",
            "123456789012345678901234567890",
        ] {
            assert!(
                matches!(
                    ThreadId::try_from(bad.to_string()),
                    Err(ThreadIdError::NotSnowflake(_))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn thread_id_as_toml_integer_is_rejected() {
        let r = Config::from_toml(&format!(
            "{SYSTEM}[[rss]]\nname = \"a\"\nurl = \"https://a.example/rss.xml\"\nwebhook = \"team\"\nthread_id = 1234\n"
        ));
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn every_webhook_section_accepts_thread_id() {
        let cfg = Config::from_toml(&format!(
            r#"{SYSTEM}
[[reminders]]
name = "standup"
time = "09:30"
days = ["mon"]
message = "Standup!"
webhook = "team"
thread_id = "1"

[[status_pages]]
name = "claude"
url = "https://status.claude.com"
webhook = "team"
thread_id = "2"

[[watches]]
name = "firefox"
webhook = "team"
source = {{ kind = "firefox" }}
thread_id = "3"

[[rss]]
name = "changelog"
url = "https://example.com/rss.xml"
webhook = "team"
thread_id = "4"
"#
        ))
        .unwrap();
        let ids = [
            cfg.reminders[0].thread_id.as_ref(),
            cfg.status_pages[0].thread_id.as_ref(),
            cfg.watches[0].thread_id.as_ref(),
            cfg.rss[0].thread_id.as_ref(),
        ]
        .map(|t| t.map(ThreadId::as_str));
        assert_eq!(ids, [Some("1"), Some("2"), Some("3"), Some("4")]);
    }

    fn first_status_page(body: &str) -> StatusPage {
        Config::from_toml(&format!("{SYSTEM}{body}"))
            .unwrap()
            .status_pages
            .remove(0)
    }

    const MAPPING: &str = r#"mapping = { incidents = "/incidents", id = "/id", title = "/title" }"#;

    #[test]
    fn status_page_kind_defaults_to_statuspage() {
        let page = first_status_page(
            r#"
[[status_pages]]
name = "claude"
url = "https://status.claude.com"
webhook = "team"
"#,
        );
        assert_eq!(page.provider, StatusProvider::Statuspage);
    }

    #[test]
    fn status_page_accepts_each_kind() {
        for (name, provider) in [
            ("statuspage", StatusProvider::Statuspage),
            ("instatus", StatusProvider::Instatus),
            ("incidentio", StatusProvider::Incidentio),
            ("datadog", StatusProvider::Datadog),
        ] {
            let page = first_status_page(&format!(
                "[[status_pages]]\nname = \"x\"\nkind = \"{name}\"\nurl = \"https://x.example\"\nwebhook = \"team\"\n"
            ));
            assert_eq!(page.provider, provider, "{name}");
        }
        let page = first_status_page(&format!(
            "[[status_pages]]\nname = \"x\"\nkind = \"custom\"\nurl = \"https://x.example/i.json\"\nwebhook = \"team\"\n{MAPPING}\n"
        ));
        let StatusProvider::Custom(mapping) = page.provider else {
            panic!("expected custom, got {:?}", page.provider);
        };
        assert_eq!(mapping.incidents.unwrap().as_str(), "/incidents");
        assert_eq!(mapping.id.as_str(), "/id");
        assert!(mapping.status.is_none());
    }

    #[test]
    fn status_page_rejects_unknown_kind() {
        for kind in ["uptimerobot", "Statuspage", "incident.io"] {
            let r = Config::from_toml(&format!(
                "{SYSTEM}[[status_pages]]\nname = \"x\"\nkind = \"{kind}\"\nurl = \"https://x.example\"\nwebhook = \"team\"\n"
            ));
            assert!(matches!(r, Err(ConfigError::Toml(_))), "{kind}");
        }
    }

    #[test]
    fn custom_status_page_requires_mapping() {
        let r = Config::from_toml(&format!(
            "{SYSTEM}[[status_pages]]\nname = \"mine\"\nkind = \"custom\"\nurl = \"https://x.example/i.json\"\nwebhook = \"team\"\n"
        ));
        let err = r.unwrap_err().to_string();
        assert!(
            err.contains("status page `mine`: kind `custom` requires `mapping`"),
            "{err}"
        );
    }

    #[test]
    fn mapping_is_rejected_on_non_custom_page() {
        for kind in ["", "kind = \"datadog\"\n"] {
            let r = Config::from_toml(&format!(
                "{SYSTEM}[[status_pages]]\nname = \"mine\"\n{kind}url = \"https://x.example\"\nwebhook = \"team\"\n{MAPPING}\n"
            ));
            let err = r.unwrap_err().to_string();
            assert!(
                err.contains("status page `mine`: `mapping` is only allowed with kind `custom`"),
                "{kind:?}: {err}"
            );
        }
    }

    #[test]
    fn mapping_rejects_unknown_key() {
        let r = Config::from_toml(&format!(
            "{SYSTEM}[[status_pages]]\nname = \"x\"\nkind = \"custom\"\nurl = \"https://x.example/i.json\"\nwebhook = \"team\"\nmapping = {{ id = \"/id\", title = \"/t\", severity = \"/s\" }}\n"
        ));
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }

    #[test]
    fn mapping_requires_id_and_title() {
        for mapping in [r#"{ id = "/id" }"#, r#"{ title = "/t" }"#] {
            let r = Config::from_toml(&format!(
                "{SYSTEM}[[status_pages]]\nname = \"x\"\nkind = \"custom\"\nurl = \"https://x.example/i.json\"\nwebhook = \"team\"\nmapping = {mapping}\n"
            ));
            assert!(matches!(r, Err(ConfigError::Toml(_))), "{mapping}");
        }
    }

    #[test]
    fn mapping_pointer_must_be_absolute() {
        let r = Config::from_toml(&format!(
            "{SYSTEM}[[status_pages]]\nname = \"x\"\nkind = \"custom\"\nurl = \"https://x.example/i.json\"\nwebhook = \"team\"\nmapping = {{ id = \"id\", title = \"/t\" }}\n"
        ));
        assert!(matches!(r, Err(ConfigError::Toml(_))));
    }
}
