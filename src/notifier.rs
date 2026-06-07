use reqwest::Client;
use serde::Serialize;
use url::Url;

const MAX_ERROR_BODY: usize = 512;

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("webhook returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
}

#[allow(async_fn_in_trait)]
pub(crate) trait Notifier {
    async fn send(&self, webhook: &Url, message: &str) -> Result<(), NotifyError>;
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

#[derive(Serialize)]
struct DiscordPayload<'a> {
    content: &'a str,
}

impl Notifier for Discord {
    async fn send(&self, webhook: &Url, message: &str) -> Result<(), NotifyError> {
        let resp = self
            .client
            .post(webhook.clone())
            .json(&DiscordPayload { content: message })
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
