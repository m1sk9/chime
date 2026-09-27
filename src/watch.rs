use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use url::Url;

use crate::config::{ChromeChannel, ChromePlatform, ChromeRollout, FirefoxChannel, WatchKind};
use crate::fetch::{FetchError, Fetched, fetch_json};
use crate::notifier::{DiscordMessage, Embed};
use crate::runtime::RunWatch;

pub const FIREFOX_VERSIONS_URL: &str =
    "https://product-details.mozilla.org/1.0/firefox_versions.json";
const CHROME_VERSION_HISTORY: &str = "https://versionhistory.googleapis.com/v1/chrome/platforms";
const CHROME_RELEASES_BLOG: &str = "https://chromereleases.googleblog.com/";
const FIREFOX_RELEASE_NOTES: &str = "https://www.firefox.com/firefox/";
const FIREFOX_NIGHTLY_NOTES: &str = "https://www.firefox.com/firefox/nightly/notes/";

/// Discord blurple. A release has no severity, so every kind shares one colour.
const COLOR_RELEASE: u32 = 0x5865F2;

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("response is not the expected JSON: {0}")]
    Decode(#[source] serde_json::Error),
    #[error(transparent)]
    Extract(#[from] ExtractError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtractError {
    #[error("nothing at JSON pointer `{0}`")]
    Missing(String),
    #[error("value at JSON pointer `{0}` is not a string or number")]
    NotScalar(String),
    #[error("value at JSON pointer `{0}` is a fractional number; serve the version as a string")]
    Fractional(String),
    #[error("value at JSON pointer `{0}` is empty")]
    Empty(String),
    #[error("product-details has no version for the {0} channel")]
    FirefoxChannelEmpty(&'static str),
    #[error("version history returned no release")]
    ChromeNoRelease,
}

// Why not `deny_unknown_fields`: like the Statuspage wire types, these mirror
// third-party APIs that add keys without notice.
// Why every key is an `Option`: product-details publishes channels that are empty for
// part of the release cycle (`FIREFOX_ESR_NEXT`, `FIREFOX_AURORA`). Only the channel a
// watch selects has to be present, and that is checked at extraction time. Why not
// `String` with `default`: that covers a missing key but not a `null`, and one `null`
// in a channel nobody watches would fail the decode for every Firefox watch.
// Why not `LAST_RELEASE_DATE`: it is the date of the last major release, not of the
// version in the embed title — a dot release does not move it.
#[derive(Debug, Deserialize)]
struct FirefoxVersions {
    #[serde(rename = "LATEST_FIREFOX_VERSION", default)]
    release: Option<String>,
    #[serde(rename = "FIREFOX_ESR", default)]
    esr: Option<String>,
    #[serde(rename = "LATEST_FIREFOX_RELEASED_DEVEL_VERSION", default)]
    beta: Option<String>,
    #[serde(rename = "FIREFOX_DEVEDITION", default)]
    devedition: Option<String>,
    #[serde(rename = "FIREFOX_NIGHTLY", default)]
    nightly: Option<String>,
    #[serde(rename = "NEXT_RELEASE_DATE", default)]
    next_release_date: Option<String>,
}

impl FirefoxVersions {
    fn get(&self, channel: FirefoxChannel) -> &str {
        match channel {
            FirefoxChannel::Release => &self.release,
            FirefoxChannel::Esr => &self.esr,
            FirefoxChannel::Beta => &self.beta,
            FirefoxChannel::Devedition => &self.devedition,
            FirefoxChannel::Nightly => &self.nightly,
        }
        .as_deref()
        .unwrap_or_default()
        .trim()
    }
}

#[derive(Debug, Deserialize)]
struct ChromeReleases {
    #[serde(default)]
    releases: Vec<ChromeRelease>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChromeRelease {
    version: String,
    #[serde(default)]
    fraction: Option<f64>,
    #[serde(default)]
    serving: Option<ChromeServing>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChromeServing {
    #[serde(default)]
    start_time: Option<String>,
}

/// One observed release: only what the fetched body said. What the watch's own
/// config already determines (channel, platform, link) is read from `RunWatch`
/// when the message is built.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// The dedup key; everything else is display.
    pub version: String,
    pub detail: ReleaseDetail,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReleaseDetail {
    Firefox {
        /// `(label, version)` for every non-empty channel, in the order Release,
        /// ESR, Beta, Developer Edition, Nightly.
        channels: Vec<(&'static str, String)>,
        /// Only for the release channel; product-details has no dates for the others.
        next_release_date: Option<String>,
    },
    Chrome {
        fraction: Option<f64>,
        /// When this version started serving, per the API.
        serving_since: Option<DateTime<Utc>>,
    },
    Plain,
}

/// How a version is read out of a fetched body. Adding a source is a new variant
/// here plus the `WatchKind` that selects it; the poller never learns the difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extractor {
    Firefox {
        channel: FirefoxChannel,
    },
    Chrome {
        platform: ChromePlatform,
        channel: ChromeChannel,
    },
    JsonPointer {
        pointer: String,
    },
}

impl Extractor {
    /// Pure: parses `body` and builds the `Release`. Never touches the network.
    pub fn extract(&self, body: &[u8]) -> Result<Release, WatchError> {
        match self {
            Extractor::Firefox { channel } => extract_firefox(*channel, body),
            Extractor::Chrome { .. } => extract_chrome(body),
            Extractor::JsonPointer { pointer } => extract_pointer(pointer, body),
        }
    }
}

const FIREFOX_CHANNELS: [FirefoxChannel; 5] = [
    FirefoxChannel::Release,
    FirefoxChannel::Esr,
    FirefoxChannel::Beta,
    FirefoxChannel::Devedition,
    FirefoxChannel::Nightly,
];

fn extract_firefox(channel: FirefoxChannel, body: &[u8]) -> Result<Release, WatchError> {
    let wire: FirefoxVersions = serde_json::from_slice(body).map_err(WatchError::Decode)?;
    let version = wire.get(channel);
    if version.is_empty() {
        return Err(ExtractError::FirefoxChannelEmpty(channel.label()).into());
    }
    let channels = FIREFOX_CHANNELS
        .iter()
        .filter(|c| !wire.get(**c).is_empty())
        .map(|c| (c.label(), wire.get(*c).to_string()))
        .collect();
    let next_release_date = wire
        .next_release_date
        .as_deref()
        .map(str::trim)
        .filter(|d| channel == FirefoxChannel::Release && !d.is_empty())
        .map(str::to_string);
    Ok(Release {
        version: version.to_string(),
        detail: ReleaseDetail::Firefox {
            channels,
            next_release_date,
        },
    })
}

fn extract_chrome(body: &[u8]) -> Result<Release, WatchError> {
    let wire: ChromeReleases = serde_json::from_slice(body).map_err(WatchError::Decode)?;
    let top = wire
        .releases
        .into_iter()
        .next()
        .ok_or(ExtractError::ChromeNoRelease)?;
    let version = top.version.trim();
    if version.is_empty() {
        return Err(ExtractError::ChromeNoRelease.into());
    }
    let serving_since = top
        .serving
        .and_then(|s| s.start_time)
        .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
        .map(|t| t.with_timezone(&Utc));
    Ok(Release {
        version: version.to_string(),
        detail: ReleaseDetail::Chrome {
            fraction: top.fraction,
            serving_since,
        },
    })
}

fn extract_pointer(pointer: &str, body: &[u8]) -> Result<Release, WatchError> {
    let doc: serde_json::Value = serde_json::from_slice(body).map_err(WatchError::Decode)?;
    let version = match doc.pointer(pointer) {
        None => return Err(ExtractError::Missing(pointer.to_string()).into()),
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        // Why integers only: a fractional number is parsed as `f64`, so `1.10` would
        // read back as `1.1` and then equal a later, genuinely different `1.1`.
        Some(serde_json::Value::Number(n)) if n.is_i64() || n.is_u64() => n.to_string(),
        Some(serde_json::Value::Number(_)) => {
            return Err(ExtractError::Fractional(pointer.to_string()).into());
        }
        Some(_) => return Err(ExtractError::NotScalar(pointer.to_string()).into()),
    };
    if version.is_empty() {
        return Err(ExtractError::Empty(pointer.to_string()).into());
    }
    Ok(Release {
        version,
        detail: ReleaseDetail::Plain,
    })
}

/// The notes URL does not accept the product-details spelling of pre-release
/// versions: `140.16.0esr` and `157.0b5` are 404, `140.16.0` and `157.0beta` are not.
fn firefox_release_notes(channel: FirefoxChannel, version: &str) -> String {
    let path = match channel {
        FirefoxChannel::Release => version.to_string(),
        FirefoxChannel::Esr => version.strip_suffix("esr").unwrap_or(version).to_string(),
        FirefoxChannel::Beta | FirefoxChannel::Devedition => version
            .split_once('b')
            .map(|(v, _)| format!("{v}beta"))
            .unwrap_or_else(|| version.to_string()),
        FirefoxChannel::Nightly => return FIREFOX_NIGHTLY_NOTES.to_string(),
    };
    format!("{FIREFOX_RELEASE_NOTES}{path}/releasenotes/")
}

fn chrome_channel_link(channel: ChromeChannel) -> String {
    let label = match channel {
        ChromeChannel::Stable => "Stable%20updates",
        ChromeChannel::Extended => "Extended%20Stable%20updates",
        ChromeChannel::Beta => "Beta%20updates",
        ChromeChannel::Dev => "Dev%20updates",
        // Canary builds are not announced on the blog, so there is no label to link.
        ChromeChannel::Canary => return CHROME_RELEASES_BLOG.to_string(),
    };
    format!("{CHROME_RELEASES_BLOG}search/label/{label}")
}

/// Where the embed title links for `version`: derived for the presets, the
/// configured `link` for `json`.
fn release_link(watch: &RunWatch, version: &str) -> Option<String> {
    match watch.extractor {
        Extractor::Firefox { channel } => Some(firefox_release_notes(channel, version)),
        Extractor::Chrome { channel, .. } => Some(chrome_channel_link(channel)),
        Extractor::JsonPointer { .. } => watch.link.clone(),
    }
}

/// Everything the poller needs, derived from one `WatchKind` at resolve time.
#[derive(Debug, Clone)]
pub struct SourceSpec {
    pub label: String,
    pub url: Url,
    pub extractor: Extractor,
    /// Only `json` has one; the presets derive theirs from the version.
    pub link: Option<String>,
}

/// Called from `runtime::resolve`. The only fallible step is `Url::parse` of the
/// preset URLs, which cannot fail for the fixed inputs but is surfaced rather than
/// unwrapped.
pub fn resolve_source(name: &str, kind: &WatchKind) -> Result<SourceSpec, url::ParseError> {
    match kind {
        WatchKind::Firefox { channel } => Ok(SourceSpec {
            label: match channel {
                FirefoxChannel::Release => "Firefox".to_string(),
                other => format!("Firefox {}", other.label()),
            },
            url: Url::parse(FIREFOX_VERSIONS_URL)?,
            extractor: Extractor::Firefox { channel: *channel },
            link: None,
        }),
        WatchKind::Chrome {
            platform,
            channel,
            rollout,
        } => {
            let filter = match rollout {
                ChromeRollout::Started => "endtime%3Dnone",
                ChromeRollout::Complete => "fraction%3D1,endtime%3Dnone",
            };
            let url = format!(
                "{CHROME_VERSION_HISTORY}/{}/channels/{}/versions/all/releases?order_by=version%20desc&filter={filter}&pageSize=1",
                platform.as_api_str(),
                channel.as_api_str(),
            );
            Ok(SourceSpec {
                label: format!("Chrome {}", channel.label()),
                url: Url::parse(&url)?,
                extractor: Extractor::Chrome {
                    platform: *platform,
                    channel: *channel,
                },
                link: None,
            })
        }
        WatchKind::Json { url, pointer, link } => Ok(SourceSpec {
            label: name.to_string(),
            url: url.as_url().clone(),
            extractor: Extractor::JsonPointer {
                pointer: pointer.as_str().to_string(),
            },
            link: link.as_ref().map(|l| l.as_url().to_string()),
        }),
    }
}

/// Per-watch dedup state. In-memory only: a restart re-baselines.
#[derive(Debug, Default)]
pub struct WatchState {
    last: Option<String>,
}

/// Per-feed polling state: the validator for the shared request, and one
/// `WatchState` per watch on the feed, in the feed's order.
#[derive(Debug, Default)]
pub struct FeedState {
    etag: Option<String>,
    watches: Vec<WatchState>,
}

impl FeedState {
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseEvent {
    pub previous: String,
    pub release: Release,
}

/// What one `diff` did. `Baseline` is only ever returned once per state.
#[derive(Debug, Clone, PartialEq)]
pub enum Diff {
    Baseline(String),
    Unchanged,
    Changed(ReleaseEvent),
}

/// Records `release.version` in `state` before returning, so a Discord failure
/// never causes the same release to be re-sent. The first call is the cold-start
/// baseline and never reports.
///
/// Why any change and not only an increase: Chrome pulls rollouts, and the top
/// served version then goes backwards — which is worth knowing. Comparing versions
/// would also need a common ordering across `140.16.0esr`, `157.0b5` and `v24.9.0`.
pub fn diff(state: &mut WatchState, release: Release) -> Diff {
    match state.last.replace(release.version.clone()) {
        None => Diff::Baseline(release.version),
        Some(previous) if previous == release.version => Diff::Unchanged,
        Some(previous) => Diff::Changed(ReleaseEvent { previous, release }),
    }
}

/// Pure: runs every watch on a feed over one fetched body and diffs each result,
/// returning one entry per watch in `watches` order.
///
/// The ETag is recorded here, and only when every watch could read its version:
/// storing it for a body some watch could not read would turn every later poll
/// into a 304, and a broken pointer would stop surfacing as a failure.
pub fn read_feed(
    state: &mut FeedState,
    watches: &[RunWatch],
    body: &[u8],
    etag: Option<String>,
) -> Vec<Result<Diff, WatchError>> {
    state
        .watches
        .resize_with(watches.len(), WatchState::default);
    let results: Vec<_> = watches
        .iter()
        .zip(&mut state.watches)
        .map(|(w, s)| w.extractor.extract(body).map(|r| diff(s, r)))
        .collect();
    if results.iter().all(Result::is_ok) {
        state.etag = etag;
    }
    results
}

/// `detected_at` is always the embed timestamp.
///
/// Why not Chrome's serving start time, although it is the more precise release
/// time: after a pulled rollout the served version goes back to an older release,
/// and its start time would date the post days in the past. It is shown as a field
/// instead.
pub fn build_message(
    watch: &RunWatch,
    event: &ReleaseEvent,
    detected_at: DateTime<Utc>,
) -> DiscordMessage {
    let release = &event.release;
    let mut embed = Embed::new(
        &format!("{} {}", watch.label, release.version),
        COLOR_RELEASE,
    )
    .with_field("Previous", &event.previous, true);
    if let Some(link) = release_link(watch, &release.version) {
        embed = embed.with_url(&link);
    }

    match watch.extractor {
        Extractor::Firefox { channel } => {
            embed = embed.with_field("Channel", channel.label(), true);
        }
        Extractor::Chrome { platform, channel } => {
            embed = embed
                .with_field("Channel", channel.label(), true)
                .with_field("Platform", platform.as_api_str(), true);
        }
        Extractor::JsonPointer { .. } => {}
    }

    match &release.detail {
        ReleaseDetail::Firefox {
            channels,
            next_release_date,
        } => {
            if let Some(d) = next_release_date {
                embed = embed.with_field("Next release", d, true);
            }
            let all = channels
                .iter()
                .map(|(label, v)| format!("{label} {v}"))
                .collect::<Vec<_>>()
                .join(" · ");
            if !all.is_empty() {
                embed = embed.with_field("Channels", &all, false);
            }
        }
        ReleaseDetail::Chrome {
            fraction,
            serving_since,
        } => {
            if let Some(f) = fraction {
                embed = embed.with_field("Rollout", &rollout_percent(*f), true);
            }
            if let Some(m) = chrome_milestone(&release.version) {
                embed = embed.with_field("Milestone", &m.to_string(), true);
            }
            if let Some(t) = serving_since {
                embed = embed.with_field(
                    "Serving since",
                    &t.format("%Y-%m-%d %H:%M UTC").to_string(),
                    true,
                );
            }
        }
        ReleaseDetail::Plain => {}
    }

    embed = embed
        .with_footer(&watch.host)
        .with_timestamp(&detected_at.to_rfc3339());
    DiscordMessage::embed(embed).with_identity(&watch.display_name, watch.avatar_url.as_deref())
}

fn chrome_milestone(version: &str) -> Option<u32> {
    version.split('.').next().and_then(|m| m.parse().ok())
}

fn rollout_percent(fraction: f64) -> String {
    if fraction >= 1.0 {
        return "100%".to_string();
    }
    if fraction <= 0.0 {
        return "0%".to_string();
    }
    // Clamped so one-decimal rounding never shows a partial rollout as `100.0%`
    // or a started one as `0.0%`.
    format!("{:.1}%", (fraction * 100.0).clamp(0.1, 99.9))
}

#[allow(async_fn_in_trait)]
pub(crate) trait WatchSource {
    async fn fetch(&self, url: &Url, etag: Option<&str>) -> Result<Fetched<Vec<u8>>, FetchError>;
}

#[derive(Debug, Clone)]
pub struct JsonEndpoint {
    client: Client,
}

impl JsonEndpoint {
    pub fn new(client: Client) -> Self {
        JsonEndpoint { client }
    }
}

impl WatchSource for JsonEndpoint {
    async fn fetch(&self, url: &Url, etag: Option<&str>) -> Result<Fetched<Vec<u8>>, FetchError> {
        fetch_json(&self.client, url, etag).await
    }
}

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use test_support::*;

#[cfg(test)]
mod test_support {
    use super::*;

    /// Measured from product-details on 2026-09-27.
    pub(crate) const FIREFOX_FIXTURE: &str = r#"{
    "FIREFOX_AURORA": "",
    "FIREFOX_DEVEDITION": "157.0b5",
    "FIREFOX_ESR": "140.16.0esr",
    "FIREFOX_ESR115": "115.41.0esr",
    "FIREFOX_ESR_NEXT": "153.3.0esr",
    "FIREFOX_NIGHTLY": "159.0a1",
    "LAST_MERGE_DATE": "2026-09-24",
    "LAST_RELEASE_DATE": "2026-09-25",
    "LAST_STRINGFREEZE_DATE": "2026-09-23",
    "LATEST_FIREFOX_DEVEL_VERSION": "157.0b5",
    "LATEST_FIREFOX_OLDER_VERSION": "3.6.28",
    "LATEST_FIREFOX_RELEASED_DEVEL_VERSION": "157.0b5",
    "LATEST_FIREFOX_VERSION": "156.0.1",
    "NEXT_MERGE_DATE": "2026-10-08",
    "NEXT_RELEASE_DATE": "2026-10-09",
    "NEXT_STRINGFREEZE_DATE": "2026-10-07"
}"#;

    /// The first two releases of the Chrome stable/win feed on 2026-09-27: one
    /// rollout just started, one fully served.
    pub(crate) const CHROME_FIXTURE: &str = r#"{
  "releases": [
    {
      "name": "chrome/platforms/win/channels/stable/versions/155.0.8059.12/releases/1790189442",
      "serving": { "startTime": "2026-09-23T18:50:42.380821Z" },
      "fraction": 0.005,
      "version": "155.0.8059.12",
      "fractionGroup": "152",
      "pinnable": false,
      "rolloutData": [ { "rolloutName": "155.0.8059.12 Rollout", "tag": [ "rollout" ] } ]
    },
    {
      "name": "chrome/platforms/win/channels/stable/versions/154.0.8037.58/releases/1790100520",
      "serving": { "startTime": "2026-09-22T18:08:40.926452Z" },
      "fraction": 1,
      "version": "154.0.8037.58",
      "fractionGroup": "151",
      "pinnable": true,
      "rolloutData": []
    }
  ],
  "nextPageToken": ""
}"#;

    pub(crate) fn plain(version: &str) -> Release {
        Release {
            version: version.to_string(),
            detail: ReleaseDetail::Plain,
        }
    }

    /// Drive a state past its cold-start baseline so a test can assert on real diffs.
    pub(crate) fn baselined(version: &str) -> WatchState {
        let mut state = WatchState::default();
        assert_eq!(
            diff(&mut state, plain(version)),
            Diff::Baseline(version.to_string())
        );
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HttpsUrl, JsonPointer};
    use crate::runtime::{RunWatch, mk_run_watch};

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn firefox(channel: FirefoxChannel) -> Extractor {
        Extractor::Firefox { channel }
    }

    fn chrome(channel: ChromeChannel) -> Extractor {
        Extractor::Chrome {
            platform: ChromePlatform::Win,
            channel,
        }
    }

    fn pointer(p: &str) -> Extractor {
        Extractor::JsonPointer {
            pointer: p.to_string(),
        }
    }

    fn watch_with(name: &str, extractor: Extractor) -> RunWatch {
        let mut w = mk_run_watch(name);
        w.extractor = extractor;
        w
    }

    fn chrome_kind(
        platform: ChromePlatform,
        channel: ChromeChannel,
        rollout: ChromeRollout,
    ) -> WatchKind {
        WatchKind::Chrome {
            platform,
            channel,
            rollout,
        }
    }

    #[test]
    fn firefox_preset_uses_product_details_for_every_channel() {
        for (channel, label) in [
            (FirefoxChannel::Release, "Firefox"),
            (FirefoxChannel::Esr, "Firefox ESR"),
            (FirefoxChannel::Beta, "Firefox Beta"),
            (FirefoxChannel::Devedition, "Firefox Developer Edition"),
            (FirefoxChannel::Nightly, "Firefox Nightly"),
        ] {
            let spec = resolve_source("ff", &WatchKind::Firefox { channel }).unwrap();
            assert_eq!(spec.label, label);
            assert_eq!(spec.url.as_str(), FIREFOX_VERSIONS_URL);
            assert_eq!(spec.extractor, firefox(channel));
            assert!(spec.link.is_none(), "derived from the version instead");
        }
    }

    #[test]
    fn chrome_preset_builds_the_version_history_url() {
        let started = resolve_source(
            "c",
            &chrome_kind(
                ChromePlatform::MacArm64,
                ChromeChannel::Beta,
                ChromeRollout::Started,
            ),
        )
        .unwrap();
        assert_eq!(
            started.url.as_str(),
            "https://versionhistory.googleapis.com/v1/chrome/platforms/mac_arm64/channels/beta/versions/all/releases?order_by=version%20desc&filter=endtime%3Dnone&pageSize=1"
        );
        assert_eq!(started.label, "Chrome Beta");
        assert_eq!(
            started.extractor,
            Extractor::Chrome {
                platform: ChromePlatform::MacArm64,
                channel: ChromeChannel::Beta,
            }
        );

        let complete = resolve_source(
            "c",
            &chrome_kind(
                ChromePlatform::Win,
                ChromeChannel::Stable,
                ChromeRollout::Complete,
            ),
        )
        .unwrap();
        assert_eq!(
            complete.url.as_str(),
            "https://versionhistory.googleapis.com/v1/chrome/platforms/win/channels/stable/versions/all/releases?order_by=version%20desc&filter=fraction%3D1,endtime%3Dnone&pageSize=1"
        );
        assert_eq!(complete.label, "Chrome Stable");
    }

    #[test]
    fn json_kind_uses_the_given_url_pointer_and_link() {
        let kind = WatchKind::Json {
            url: HttpsUrl::try_from("https://nodejs.org/dist/index.json".to_string()).unwrap(),
            pointer: JsonPointer::try_from("/0/version".to_string()).unwrap(),
            link: Some(
                HttpsUrl::try_from("https://nodejs.org/en/blog/release".to_string()).unwrap(),
            ),
        };
        let spec = resolve_source("node", &kind).unwrap();
        assert_eq!(spec.label, "node");
        assert_eq!(spec.url.as_str(), "https://nodejs.org/dist/index.json");
        assert_eq!(
            spec.extractor,
            Extractor::JsonPointer {
                pointer: "/0/version".to_string(),
            }
        );
        assert_eq!(
            spec.link.as_deref(),
            Some("https://nodejs.org/en/blog/release")
        );
    }

    #[test]
    fn firefox_payload_extracts_the_configured_channel() {
        for (channel, version) in [
            (FirefoxChannel::Release, "156.0.1"),
            (FirefoxChannel::Esr, "140.16.0esr"),
            (FirefoxChannel::Beta, "157.0b5"),
            (FirefoxChannel::Devedition, "157.0b5"),
            (FirefoxChannel::Nightly, "159.0a1"),
        ] {
            let release = firefox(channel)
                .extract(FIREFOX_FIXTURE.as_bytes())
                .unwrap();
            assert_eq!(release.version, version, "{channel:?}");
        }
    }

    #[test]
    fn firefox_payload_carries_every_channel_and_the_next_release_date() {
        let release = firefox(FirefoxChannel::Release)
            .extract(FIREFOX_FIXTURE.as_bytes())
            .unwrap();
        let ReleaseDetail::Firefox {
            channels,
            next_release_date,
        } = release.detail
        else {
            panic!("expected firefox detail");
        };
        assert_eq!(
            channels,
            vec![
                ("Release", "156.0.1".to_string()),
                ("ESR", "140.16.0esr".to_string()),
                ("Beta", "157.0b5".to_string()),
                ("Developer Edition", "157.0b5".to_string()),
                ("Nightly", "159.0a1".to_string()),
            ]
        );
        assert_eq!(next_release_date.as_deref(), Some("2026-10-09"));

        let esr = firefox(FirefoxChannel::Esr)
            .extract(FIREFOX_FIXTURE.as_bytes())
            .unwrap();
        let ReleaseDetail::Firefox {
            next_release_date, ..
        } = esr.detail
        else {
            panic!("expected firefox detail");
        };
        assert!(
            next_release_date.is_none(),
            "the date belongs to the release channel"
        );
    }

    #[test]
    fn firefox_empty_channel_is_an_extract_error() {
        let body = br#"{"LATEST_FIREFOX_VERSION": "", "FIREFOX_ESR": "140.16.0esr"}"#;
        let err = firefox(FirefoxChannel::Release).extract(body).unwrap_err();
        assert!(matches!(
            err,
            WatchError::Extract(ExtractError::FirefoxChannelEmpty("Release"))
        ));
    }

    #[test]
    fn firefox_null_in_an_unwatched_channel_does_not_break_the_watched_one() {
        let body = br#"{"LATEST_FIREFOX_VERSION": "156.0.1", "FIREFOX_DEVEDITION": null, "NEXT_RELEASE_DATE": null}"#;
        let release = firefox(FirefoxChannel::Release).extract(body).unwrap();
        assert_eq!(release.version, "156.0.1");
        let ReleaseDetail::Firefox {
            channels,
            next_release_date,
        } = release.detail
        else {
            panic!("expected firefox detail");
        };
        assert_eq!(channels, vec![("Release", "156.0.1".to_string())]);
        assert!(next_release_date.is_none());

        let err = firefox(FirefoxChannel::Devedition)
            .extract(body)
            .unwrap_err();
        assert!(matches!(
            err,
            WatchError::Extract(ExtractError::FirefoxChannelEmpty("Developer Edition"))
        ));
    }

    #[test]
    fn firefox_link_strips_esr_and_rewrites_beta() {
        assert_eq!(
            firefox_release_notes(FirefoxChannel::Release, "156.0.1"),
            "https://www.firefox.com/firefox/156.0.1/releasenotes/"
        );
        assert_eq!(
            firefox_release_notes(FirefoxChannel::Esr, "140.16.0esr"),
            "https://www.firefox.com/firefox/140.16.0/releasenotes/"
        );
        assert_eq!(
            firefox_release_notes(FirefoxChannel::Beta, "157.0b5"),
            "https://www.firefox.com/firefox/157.0beta/releasenotes/"
        );
        assert_eq!(
            firefox_release_notes(FirefoxChannel::Devedition, "157.0b5"),
            "https://www.firefox.com/firefox/157.0beta/releasenotes/"
        );
        assert_eq!(
            firefox_release_notes(FirefoxChannel::Nightly, "159.0a1"),
            "https://www.firefox.com/firefox/nightly/notes/"
        );
    }

    #[test]
    fn chrome_payload_extracts_the_first_release_with_rollout_and_start_time() {
        let release = chrome(ChromeChannel::Stable)
            .extract(CHROME_FIXTURE.as_bytes())
            .unwrap();
        assert_eq!(release.version, "155.0.8059.12");
        assert_eq!(
            release.detail,
            ReleaseDetail::Chrome {
                fraction: Some(0.005),
                serving_since: Some(ts("2026-09-23T18:50:42.380821Z")),
            }
        );
    }

    #[test]
    fn chrome_milestone_is_the_leading_version_component() {
        assert_eq!(chrome_milestone("155.0.8059.12"), Some(155));
        assert_eq!(chrome_milestone("not-a-version"), None);
    }

    #[test]
    fn chrome_empty_releases_is_an_extract_error() {
        for body in [&br#"{"releases": [], "nextPageToken": ""}"#[..], b"{}"] {
            let err = chrome(ChromeChannel::Stable).extract(body).unwrap_err();
            assert!(matches!(
                err,
                WatchError::Extract(ExtractError::ChromeNoRelease)
            ));
        }
    }

    #[test]
    fn chrome_unparsable_start_time_degrades_to_none() {
        let body = br#"{"releases": [{"version": "1.2.3.4", "serving": {"startTime": "soon"}}]}"#;
        let release = chrome(ChromeChannel::Stable).extract(body).unwrap();
        assert_eq!(release.version, "1.2.3.4");
        assert!(matches!(
            release.detail,
            ReleaseDetail::Chrome {
                serving_since: None,
                ..
            }
        ));
    }

    #[test]
    fn chrome_link_follows_the_channel() {
        let base = "https://chromereleases.googleblog.com/";
        assert_eq!(
            chrome_channel_link(ChromeChannel::Stable),
            format!("{base}search/label/Stable%20updates")
        );
        assert_eq!(
            chrome_channel_link(ChromeChannel::Extended),
            format!("{base}search/label/Extended%20Stable%20updates")
        );
        assert_eq!(
            chrome_channel_link(ChromeChannel::Beta),
            format!("{base}search/label/Beta%20updates")
        );
        assert_eq!(
            chrome_channel_link(ChromeChannel::Dev),
            format!("{base}search/label/Dev%20updates")
        );
        assert_eq!(chrome_channel_link(ChromeChannel::Canary), base);
    }

    #[test]
    fn json_pointer_extracts_a_string_leaf() {
        let body = br#"[{"version": " v24.9.0 ", "lts": false}]"#;
        let release = pointer("/0/version").extract(body).unwrap();
        assert_eq!(release, plain("v24.9.0"));
    }

    #[test]
    fn json_pointer_renders_a_number_as_text() {
        let release = pointer("/build").extract(br#"{"build": 1234}"#).unwrap();
        assert_eq!(release.version, "1234");
    }

    #[test]
    fn json_pointer_rejects_a_missing_path() {
        let err = pointer("/releases/0/version")
            .extract(br#"{"releases": []}"#)
            .unwrap_err();
        assert!(matches!(
            err,
            WatchError::Extract(ExtractError::Missing(p)) if p == "/releases/0/version"
        ));
    }

    #[test]
    fn json_pointer_rejects_a_fractional_number() {
        let err = pointer("/v").extract(br#"{"v": 1.10}"#).unwrap_err();
        assert!(matches!(
            err,
            WatchError::Extract(ExtractError::Fractional(p)) if p == "/v"
        ));
    }

    #[test]
    fn json_pointer_rejects_a_non_scalar_leaf() {
        for body in [
            &br#"{"v": {"major": 1}}"#[..],
            br#"{"v": [1]}"#,
            br#"{"v": true}"#,
            br#"{"v": null}"#,
        ] {
            let err = pointer("/v").extract(body).unwrap_err();
            assert!(matches!(
                err,
                WatchError::Extract(ExtractError::NotScalar(_))
            ));
        }
    }

    #[test]
    fn json_pointer_rejects_an_empty_string() {
        let err = pointer("/v").extract(br#"{"v": "  "}"#).unwrap_err();
        assert!(matches!(err, WatchError::Extract(ExtractError::Empty(_))));
    }

    #[test]
    fn non_json_body_is_a_decode_error() {
        for extractor in [
            firefox(FirefoxChannel::Release),
            chrome(ChromeChannel::Stable),
            pointer("/v"),
        ] {
            let err = extractor.extract(b"<html>maintenance</html>").unwrap_err();
            assert!(matches!(err, WatchError::Decode(_)), "{extractor:?}");
        }
    }

    #[test]
    fn first_poll_records_the_version_silently() {
        let mut state = WatchState::default();
        assert_eq!(
            diff(&mut state, plain("1.0")),
            Diff::Baseline("1.0".to_string())
        );
    }

    #[test]
    fn unchanged_version_is_not_reported_twice() {
        let mut state = baselined("1.0");
        assert_eq!(diff(&mut state, plain("1.0")), Diff::Unchanged);
        assert_eq!(diff(&mut state, plain("1.0")), Diff::Unchanged);
    }

    #[test]
    fn a_new_version_is_reported_with_the_previous_one() {
        let mut state = baselined("1.0");
        assert_eq!(
            diff(&mut state, plain("1.1")),
            Diff::Changed(ReleaseEvent {
                previous: "1.0".to_string(),
                release: plain("1.1"),
            })
        );
        // Recorded before the caller sends, so the same release never repeats.
        assert_eq!(diff(&mut state, plain("1.1")), Diff::Unchanged);
    }

    #[test]
    fn a_rollback_is_reported_as_a_change() {
        let mut state = baselined("155.0.8059.12");
        let Diff::Changed(event) = diff(&mut state, plain("154.0.8037.58")) else {
            panic!("a lower version is still a change");
        };
        assert_eq!(event.previous, "155.0.8059.12");
        assert_eq!(event.release.version, "154.0.8037.58");
    }

    #[test]
    fn a_changed_rollout_fraction_alone_is_not_a_change() {
        let at = |fraction| Release {
            version: "155.0.8059.12".to_string(),
            detail: ReleaseDetail::Chrome {
                fraction: Some(fraction),
                serving_since: None,
            },
        };
        let mut state = WatchState::default();
        assert!(matches!(diff(&mut state, at(0.005)), Diff::Baseline(_)));
        assert_eq!(diff(&mut state, at(0.5)), Diff::Unchanged);
    }

    fn event_from(extractor: &Extractor, body: &str, previous: &str) -> ReleaseEvent {
        ReleaseEvent {
            previous: previous.to_string(),
            release: extractor.extract(body.as_bytes()).unwrap(),
        }
    }

    #[test]
    fn firefox_message_lists_channels_and_the_next_release() {
        let mut watch = watch_with("firefox", firefox(FirefoxChannel::Release));
        watch.label = "Firefox".to_string();
        watch.host = "product-details.mozilla.org".to_string();
        let ev = event_from(&firefox(FirefoxChannel::Release), FIREFOX_FIXTURE, "156.0");
        let detected = ts("2026-09-27T00:00:00Z");
        let msg = build_message(&watch, &ev, detected);

        assert_eq!(msg.username.as_deref(), Some("firefox Releases"));
        let embed = &msg.embeds[0];
        assert_eq!(embed.title, "Firefox 156.0.1");
        assert_eq!(embed.color, COLOR_RELEASE);
        assert_eq!(
            embed.url.as_deref(),
            Some("https://www.firefox.com/firefox/156.0.1/releasenotes/")
        );
        let fields: Vec<(&str, &str, bool)> = embed
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_str(), f.inline))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("Previous", "156.0", true),
                ("Channel", "Release", true),
                ("Next release", "2026-10-09", true),
                (
                    "Channels",
                    "Release 156.0.1 · ESR 140.16.0esr · Beta 157.0b5 · Developer Edition 157.0b5 · Nightly 159.0a1",
                    false
                ),
            ]
        );
        assert_eq!(
            embed.footer.as_ref().unwrap().text,
            "product-details.mozilla.org"
        );
        assert_eq!(
            embed.timestamp.as_deref(),
            Some("2026-09-27T00:00:00+00:00")
        );
    }

    #[test]
    fn chrome_message_reports_rollout_platform_and_serving_time_as_a_field() {
        let mut watch = watch_with("chrome", chrome(ChromeChannel::Stable));
        watch.label = "Chrome Stable".to_string();
        let ev = event_from(
            &chrome(ChromeChannel::Stable),
            CHROME_FIXTURE,
            "154.0.8037.58",
        );
        let msg = build_message(&watch, &ev, ts("2026-09-27T00:00:00Z"));

        let embed = &msg.embeds[0];
        assert_eq!(embed.title, "Chrome Stable 155.0.8059.12");
        assert_eq!(
            embed.url.as_deref(),
            Some("https://chromereleases.googleblog.com/search/label/Stable%20updates")
        );
        let fields: Vec<(&str, &str)> = embed
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_str()))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("Previous", "154.0.8037.58"),
                ("Channel", "Stable"),
                ("Platform", "win"),
                ("Rollout", "0.5%"),
                ("Milestone", "155"),
                ("Serving since", "2026-09-23 18:50 UTC"),
            ]
        );
        assert_eq!(
            embed.timestamp.as_deref(),
            Some("2026-09-27T00:00:00+00:00"),
            "detection time, so a rollback is not dated to the older release"
        );
    }

    #[test]
    fn rollout_of_a_fully_served_release_reads_100_percent() {
        assert_eq!(rollout_percent(1.0), "100%");
        assert_eq!(rollout_percent(0.25), "25.0%");
    }

    #[test]
    fn rollout_rounding_never_reads_as_complete_or_as_not_started() {
        assert_eq!(rollout_percent(0.9996), "99.9%");
        assert_eq!(rollout_percent(0.0001), "0.1%");
        assert_eq!(rollout_percent(0.0), "0%");
    }

    #[test]
    fn plain_message_carries_only_previous_and_link() {
        let mut watch = mk_run_watch("node");
        watch.link = Some("https://nodejs.org/en/blog/release".to_string());
        let ev = event_from(&watch.extractor, r#"{"version": "v24.9.0"}"#, "v24.8.0");
        let msg = build_message(&watch, &ev, ts("2026-09-27T01:02:03Z"));

        let embed = &msg.embeds[0];
        assert_eq!(embed.title, "node v24.9.0");
        assert_eq!(
            embed.url.as_deref(),
            Some("https://nodejs.org/en/blog/release")
        );
        assert_eq!(embed.fields.len(), 1);
        assert_eq!(embed.fields[0].name, "Previous");
        assert_eq!(embed.fields[0].value, "v24.8.0");
        assert_eq!(embed.footer.as_ref().unwrap().text, "watch.node.example");
        assert_eq!(
            embed.timestamp.as_deref(),
            Some("2026-09-27T01:02:03+00:00")
        );
    }

    #[test]
    fn message_without_link_omits_url() {
        let watch = mk_run_watch("node");
        let ev = ReleaseEvent {
            previous: "1".to_string(),
            release: plain("2"),
        };
        let msg = build_message(&watch, &ev, ts("2026-09-27T00:00:00Z"));
        assert!(msg.embeds[0].url.is_none());
    }
    #[test]
    fn read_feed_diffs_every_watch_from_one_body() {
        let watches = vec![
            watch_with("a", pointer("/a")),
            watch_with("b", pointer("/b")),
        ];
        let mut state = FeedState::default();
        let first = read_feed(&mut state, &watches, br#"{"a":"1","b":"1"}"#, None);
        assert!(matches!(
            first[..],
            [Ok(Diff::Baseline(_)), Ok(Diff::Baseline(_))]
        ));

        let second = read_feed(&mut state, &watches, br#"{"a":"2","b":"1"}"#, None);
        assert!(matches!(second[0], Ok(Diff::Changed(_))));
        assert!(matches!(second[1], Ok(Diff::Unchanged)));
    }

    #[test]
    fn read_feed_keeps_the_etag_only_when_every_watch_could_read() {
        let watches = vec![
            watch_with("a", pointer("/a")),
            watch_with("b", pointer("/b")),
        ];
        let mut state = FeedState::default();
        read_feed(
            &mut state,
            &watches,
            br#"{"a":"1"}"#,
            Some("v1".to_string()),
        );
        assert_eq!(state.etag(), None, "`b` could not read this body");

        read_feed(
            &mut state,
            &watches,
            br#"{"a":"1","b":"1"}"#,
            Some("v2".to_string()),
        );
        assert_eq!(state.etag(), Some("v2"));

        read_feed(&mut state, &watches, b"<html>", Some("v3".to_string()));
        assert_eq!(
            state.etag(),
            Some("v2"),
            "an unreadable body leaves the last good validator in place"
        );
    }
}
