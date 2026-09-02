use reqwest::Client;
use serde::Serialize;
use url::Url;

const MAX_ERROR_BODY: usize = 512;

// Discord counts message limits in characters, not bytes, so every cap here is
// applied over `chars()` — byte slicing would also split multi-byte UTF-8.
const MAX_CONTENT: usize = 2000;
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
        self.username = Some(truncate(username, 80));
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

    // Why the emptiness guards: Discord rejects the whole payload with a 400 when
    // `url` or `description` is present but empty, and both are filled from a
    // third-party feed that is free to send `""`.
    pub fn with_url(mut self, url: &str) -> Self {
        if !url.trim().is_empty() {
            self.url = Some(url.to_string());
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

#[derive(Debug, Clone)]
pub struct Discord {
    client: Client,
}

impl Discord {
    pub fn new(client: Client) -> Self {
        Discord { client }
    }
}

impl Notifier for Discord {
    async fn send(&self, webhook: &Url, message: &DiscordMessage) -> Result<(), NotifyError> {
        let resp = self
            .client
            .post(webhook.clone())
            .json(message)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let bytes = resp.bytes().await?;
        let slice = if bytes.len() > MAX_ERROR_BODY {
            &bytes[..MAX_ERROR_BODY]
        } else {
            &bytes[..]
        };
        let body = String::from_utf8_lossy(slice).into_owned();
        Err(NotifyError::Status {
            status: status.as_u16(),
            body,
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
    fn identity_sets_username_and_avatar() {
        let msg = DiscordMessage::embed(Embed::new("t", 1))
            .with_identity("Claude Status", Some("https://example.com/a.png"));
        assert_eq!(msg.username.as_deref(), Some("Claude Status"));
        assert_eq!(msg.avatar_url.as_deref(), Some("https://example.com/a.png"));
    }
}
