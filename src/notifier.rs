use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::{Client, Response, StatusCode};
use serde::Serialize;
use tokio::time::Instant;
use url::Url;

const MAX_ERROR_BODY: usize = 512;
// Discord's webhook buckets reset within a couple of seconds. The ceiling only
// keeps a nonsensical header from stalling the tick.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(5);

// Discord counts message limits in characters, not bytes, so every cap here is
// applied over `chars()` — byte slicing would also split multi-byte UTF-8.
const MAX_CONTENT: usize = 2000;
const MAX_USERNAME: usize = 80;
const MAX_EMBED_TITLE: usize = 256;
// Discord allows 4096 here. The lower cap is deliberate: a Statuspage postmortem
// runs to thousands of characters, and the incident link is the authoritative
// copy — a wall of text in the channel is worse than a truncated summary.
const MAX_EMBED_DESCRIPTION: usize = 1500;
const MAX_FIELD_NAME: usize = 256;
const MAX_FIELD_VALUE: usize = 1024;
const MAX_FOOTER_TEXT: usize = 2048;

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("webhook returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
}

/// A Discord webhook execution payload. Every constructor enforces Discord's
/// length limits, so an over-long message cannot be built in the first place.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct DiscordMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub embeds: Vec<Embed>,
}

impl DiscordMessage {
    pub fn text(content: &str) -> Self {
        DiscordMessage {
            content: Some(truncate(content, MAX_CONTENT)),
            ..Default::default()
        }
    }

    pub fn embed(embed: Embed) -> Self {
        DiscordMessage {
            embeds: vec![embed],
            ..Default::default()
        }
    }

    pub fn with_identity(mut self, username: &str, avatar_url: Option<&str>) -> Self {
        self.username = Some(truncate(username, MAX_USERNAME));
        self.avatar_url = avatar_url.map(str::to_string);
        self
    }
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Embed {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub color: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<EmbedField>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<EmbedFooter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl Embed {
    pub fn new(title: &str, color: u32) -> Self {
        Embed {
            title: truncate(title, MAX_EMBED_TITLE),
            color,
            ..Default::default()
        }
    }

    // Why the guards: Discord rejects the whole payload with a 400 when `url` is
    // present but empty or not an absolute http(s) URL, and when `description` is
    // present but empty. Both are filled from a third-party feed that is free to
    // send either. Dropping the link is a better failure than dropping the message.
    pub fn with_url(mut self, url: &str) -> Self {
        let trimmed = url.trim();
        let usable = Url::parse(trimmed).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        if usable {
            self.url = Some(trimmed.to_string());
        }
        self
    }

    pub fn with_description(mut self, description: &str) -> Self {
        if !description.trim().is_empty() {
            self.description = Some(truncate(description, MAX_EMBED_DESCRIPTION));
        }
        self
    }

    pub fn with_field(mut self, name: &str, value: &str, inline: bool) -> Self {
        self.fields.push(EmbedField {
            name: truncate(name, MAX_FIELD_NAME),
            value: truncate(value, MAX_FIELD_VALUE),
            inline,
        });
        self
    }

    pub fn with_footer(mut self, text: &str) -> Self {
        self.footer = Some(EmbedFooter {
            text: truncate(text, MAX_FOOTER_TEXT),
        });
        self
    }

    pub fn with_timestamp(mut self, timestamp: &str) -> Self {
        self.timestamp = Some(timestamp.to_string());
        self
    }
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct EmbedFooter {
    pub text: String,
}

/// How long the next send to the same webhook has to wait, from the rate-limit
/// headers Discord puts on every webhook response: the bucket's reset time once
/// it has no requests left, and `Retry-After` on a 429.
fn rate_limit_wait(status: StatusCode, headers: &HeaderMap) -> Option<Duration> {
    let secs = |name: &str| -> Option<Duration> {
        let value: f64 = headers.get(name)?.to_str().ok()?.trim().parse().ok()?;
        Duration::try_from_secs_f64(value).ok()
    };
    let exhausted = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == "0");
    let reset = exhausted.then(|| secs("x-ratelimit-reset-after")).flatten();
    let retry = (status == StatusCode::TOO_MANY_REQUESTS)
        .then(|| secs("retry-after"))
        .flatten();
    reset
        .max(retry)
        .filter(|wait| !wait.is_zero())
        .map(|wait| wait.min(MAX_RATE_LIMIT_WAIT))
}

/// Cap an error response body before it is attached to an error. Discord and
/// Statuspage both answer failures with pages that dwarf the useful part.
fn error_body(bytes: &[u8]) -> String {
    let end = bytes.len().min(MAX_ERROR_BODY);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Read only as much of a failure body as [`error_body`] would keep, then leave
/// the rest on the socket. Discord and Statuspage both answer failures with a
/// full HTML page, and `Response::bytes` would buffer all of it to quote the
/// first 512 bytes — with `Accept-Encoding: gzip` on the wire the transferred
/// size no longer bounds what that costs. A read error here is swallowed on
/// purpose: the HTTP status is the finding, and losing the quote is not worth
/// masking it with a transport error.
pub(crate) async fn read_error_body(resp: &mut Response) -> String {
    let mut buf = Vec::new();
    // Ends on a read error as well as on the end of the body — see above.
    while let Ok(Some(chunk)) = resp.chunk().await {
        if fill_capped(&mut buf, &chunk) {
            break;
        }
    }
    error_body(&buf)
}

/// Append only the part of `chunk` that fits under [`MAX_ERROR_BODY`], and report
/// whether the buffer is now full.
///
/// Why not test `buf.len()` and then append whole chunks: that admits one entire
/// chunk past the cap, and a chunk is as large as the transport hands over — 4KB
/// from the gzip decoder, more from an uncompressed read.
fn fill_capped(buf: &mut Vec<u8>, chunk: &[u8]) -> bool {
    let take = (MAX_ERROR_BODY - buf.len()).min(chunk.len());
    buf.extend_from_slice(&chunk[..take]);
    buf.len() >= MAX_ERROR_BODY
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[allow(async_fn_in_trait)]
pub(crate) trait Notifier {
    async fn send(&self, webhook: &Url, message: &DiscordMessage) -> Result<(), NotifyError>;
}

#[derive(Debug)]
pub struct Discord {
    client: Client,
    /// When each webhook may be sent to again, keyed by URL path: a `thread_id`
    /// query still posts through the same webhook and its rate-limit bucket.
    not_before: RefCell<HashMap<String, Instant>>,
}

impl Discord {
    pub fn new(client: Client) -> Self {
        Discord {
            client,
            not_before: RefCell::new(HashMap::new()),
        }
    }
}

impl Notifier for Discord {
    // Waits out the bucket before the send instead of retrying after a 429: a
    // burst of new entries would otherwise lose everything past the fifth post,
    // and chime never re-sends a message that was already attempted.
    async fn send(&self, webhook: &Url, message: &DiscordMessage) -> Result<(), NotifyError> {
        let key = webhook.path().to_string();
        let until = self.not_before.borrow().get(&key).copied();
        if let Some(until) = until {
            tokio::time::sleep_until(until).await;
        }
        let mut resp = self
            .client
            .post(webhook.clone())
            .json(message)
            .send()
            .await?;
        let status = resp.status();
        match rate_limit_wait(status, resp.headers()) {
            Some(wait) => self
                .not_before
                .borrow_mut()
                .insert(key, Instant::now() + wait),
            None => self.not_before.borrow_mut().remove(&key),
        };
        if status.is_success() {
            return Ok(());
        }
        Err(NotifyError::Status {
            status: status.as_u16(),
            body: read_error_body(&mut resp).await,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_message_serializes_to_content_only() {
        let msg = DiscordMessage::text("hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"content":"hello"}"#);
    }

    #[test]
    fn truncate_keeps_char_boundaries() {
        let s = "あいうえお";
        assert_eq!(truncate(s, 10), "あいうえお");
        assert_eq!(truncate(s, 3), "あい…");
    }

    #[test]
    fn embed_enforces_discord_limits() {
        let long = "x".repeat(5000);
        let embed = Embed::new(&long, 0x00FF00)
            .with_description(&long)
            .with_field("f", &long, false)
            .with_footer(&long);
        assert_eq!(embed.title.chars().count(), MAX_EMBED_TITLE);
        assert_eq!(
            embed.description.as_ref().unwrap().chars().count(),
            MAX_EMBED_DESCRIPTION
        );
        assert_eq!(embed.fields[0].value.chars().count(), MAX_FIELD_VALUE);
        assert!(embed.title.ends_with('…'));
    }

    #[test]
    fn empty_optional_fields_are_omitted() {
        let msg = DiscordMessage::embed(Embed::new("t", 1));
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"embeds":[{"title":"t","color":1}]}"#);
    }

    #[test]
    fn empty_description_and_url_are_omitted() {
        let embed = Embed::new("t", 1).with_description("  ").with_url("");
        assert!(embed.description.is_none());
        assert!(embed.url.is_none());
    }

    #[test]
    fn a_url_discord_would_reject_is_dropped_rather_than_the_message() {
        assert!(Embed::new("t", 1).with_url("/incidents/abc").url.is_none());
        assert!(Embed::new("t", 1).with_url("stspg.io/abc").url.is_none());
        assert!(
            Embed::new("t", 1)
                .with_url("javascript:alert(1)")
                .url
                .is_none()
        );
        assert_eq!(
            Embed::new("t", 1).with_url(" https://stspg.io/abc ").url,
            Some("https://stspg.io/abc".to_string())
        );
    }

    #[test]
    fn error_body_caps_the_response_without_panicking_mid_codepoint() {
        // A cut inside a multi-byte codepoint becomes U+FFFD, so the result can be a
        // few bytes over the cap; what matters is that the body stays bounded.
        let long = "あ".repeat(1000);
        let capped = error_body(long.as_bytes());
        assert!(capped.len() < MAX_ERROR_BODY + 4);
        assert_eq!(error_body(b"boom"), "boom");
    }

    #[test]
    fn an_oversized_chunk_is_truncated_rather_than_buffered() {
        let mut buf = Vec::new();
        let chunk = vec![b'x'; MAX_ERROR_BODY * 4];

        assert!(
            fill_capped(&mut buf, &chunk),
            "the buffer reports itself full"
        );
        assert_eq!(
            buf.len(),
            MAX_ERROR_BODY,
            "a chunk larger than the cap never lands in memory whole"
        );
    }

    #[test]
    fn chunks_below_the_cap_accumulate_until_it_is_reached() {
        let mut buf = Vec::new();
        let chunk = vec![b'x'; MAX_ERROR_BODY / 2];

        assert!(!fill_capped(&mut buf, &chunk), "half a cap is not full");
        assert!(fill_capped(&mut buf, &chunk), "two halves fill it exactly");
        assert_eq!(buf.len(), MAX_ERROR_BODY);
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn an_exhausted_bucket_waits_for_its_reset() {
        let h = headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset-after", "1.5"),
        ]);
        assert_eq!(
            rate_limit_wait(StatusCode::NO_CONTENT, &h),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn a_bucket_with_requests_left_does_not_wait() {
        let h = headers(&[
            ("x-ratelimit-remaining", "3"),
            ("x-ratelimit-reset-after", "1.5"),
        ]);
        assert_eq!(rate_limit_wait(StatusCode::NO_CONTENT, &h), None);
        assert_eq!(
            rate_limit_wait(StatusCode::NO_CONTENT, &HeaderMap::new()),
            None
        );
    }

    #[test]
    fn a_429_waits_for_the_longer_of_retry_after_and_the_reset() {
        let h = headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset-after", "0.5"),
            ("retry-after", "2"),
        ]);
        assert_eq!(
            rate_limit_wait(StatusCode::TOO_MANY_REQUESTS, &h),
            Some(Duration::from_secs(2))
        );
    }

    #[test]
    fn retry_after_counts_only_on_a_429() {
        let h = headers(&[("retry-after", "2")]);
        assert_eq!(rate_limit_wait(StatusCode::NO_CONTENT, &h), None);
    }

    #[test]
    fn a_wait_is_capped_and_a_malformed_one_ignored() {
        let long = headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset-after", "3600"),
        ]);
        assert_eq!(
            rate_limit_wait(StatusCode::NO_CONTENT, &long),
            Some(MAX_RATE_LIMIT_WAIT)
        );
        for bad in ["-1", "soon", "NaN", "0"] {
            let h = headers(&[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset-after", bad),
            ]);
            assert_eq!(rate_limit_wait(StatusCode::NO_CONTENT, &h), None, "{bad}");
        }
    }

    /// Answers each accepted connection with the next response, draining the
    /// request body first so the client never sees a reset mid-write.
    fn serve(responses: Vec<&'static str>) -> std::net::SocketAddr {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                    head.push(byte[0]);
                }
                let length = String::from_utf8_lossy(&head)
                    .to_ascii_lowercase()
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:")?.trim().parse().ok())
                    .unwrap_or(0);
                stream.read_exact(&mut vec![0u8; length]).unwrap();
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        addr
    }

    const EXHAUSTED: &str = "HTTP/1.1 204 No Content\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 0.4\r\nConnection: close\r\n\r\n";
    const OK: &str = "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";

    fn discord() -> Discord {
        Discord::new(Client::builder().no_proxy().build().unwrap())
    }

    #[tokio::test]
    async fn the_next_send_to_an_exhausted_webhook_waits_for_the_reset() {
        let addr = serve(vec![EXHAUSTED, OK]);
        let hook = Url::parse(&format!("http://{addr}/api/webhooks/1/a")).unwrap();
        let thread = Url::parse(&format!("http://{addr}/api/webhooks/1/a?thread_id=9")).unwrap();
        let discord = discord();
        let msg = DiscordMessage::text("hi");

        discord.send(&hook, &msg).await.unwrap();
        let start = Instant::now();
        discord.send(&thread, &msg).await.unwrap();

        assert!(start.elapsed() >= Duration::from_millis(400));
    }

    #[tokio::test]
    async fn another_webhook_is_not_held_by_an_exhausted_one() {
        let addr = serve(vec![EXHAUSTED, OK]);
        let first = Url::parse(&format!("http://{addr}/api/webhooks/1/a")).unwrap();
        let second = Url::parse(&format!("http://{addr}/api/webhooks/2/b")).unwrap();
        let discord = discord();
        let msg = DiscordMessage::text("hi");

        discord.send(&first, &msg).await.unwrap();
        let start = Instant::now();
        discord.send(&second, &msg).await.unwrap();

        assert!(start.elapsed() < Duration::from_millis(400));
    }

    #[test]
    fn identity_sets_username_and_avatar() {
        let msg = DiscordMessage::embed(Embed::new("t", 1))
            .with_identity("Claude Status", Some("https://example.com/a.png"));
        assert_eq!(msg.username.as_deref(), Some("Claude Status"));
        assert_eq!(msg.avatar_url.as_deref(), Some("https://example.com/a.png"));
    }
}
