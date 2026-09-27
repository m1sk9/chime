use std::time::Duration;

use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH};
use reqwest::{Client, Response};
use url::Url;

use crate::notifier::read_error_body;

/// Deliberately shorter than the Discord timeout: this request runs inside the
/// scheduler tick, so a slow endpoint must not delay a reminder past its minute.
pub(crate) const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Ceiling on the decompressed size of one polled body.
///
/// Why this is needed at all: `Response::json`/`bytes` buffer the whole body with
/// no limit, and once `Accept-Encoding: gzip` is on the wire the transferred size
/// stops bounding what that costs — gzip reaches roughly 1000:1, so a few hundred
/// kilobytes of response can become a gigabyte of `Vec<u8>` inside a tick. The
/// real feeds measure 40-300KB, so this sits far above anything legitimate and
/// only ever fires on an endpoint that has gone wrong.
pub(crate) const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("body exceeded {limit} bytes")]
    TooLarge { limit: usize },
}

/// Outcome of a conditional GET. `T` is whatever the caller decodes the body into:
/// raw bytes for `fetch_json`, normalized incidents for `StatusSource`.
#[derive(Debug)]
pub enum Fetched<T> {
    NotModified,
    Modified { value: T, etag: Option<String> },
}

/// Conditional GET of a JSON endpoint with the body capped at `MAX_BODY`.
pub(crate) async fn fetch_json(
    client: &Client,
    url: &Url,
    etag: Option<&str>,
) -> Result<Fetched<Vec<u8>>, FetchError> {
    // `Accept` is required, not merely polite. The status page CDN answers with
    // `Vary: Accept, Accept-Encoding`, and a request that omits `Accept` lands on
    // a variant that returns 200 to every `If-None-Match` — verified against
    // status.claude.com. Dropping this header silently disables conditional GETs.
    let mut request = client
        .get(url.clone())
        .timeout(FETCH_TIMEOUT)
        .header(ACCEPT, "application/json");
    if let Some(tag) = etag {
        request = request.header(IF_NONE_MATCH, tag);
    }
    let mut resp = request.send().await?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !status.is_success() {
        return Err(FetchError::Status {
            status: status.as_u16(),
            body: read_error_body(&mut resp).await,
        });
    }
    let new_etag = resp
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = read_capped(&mut resp, MAX_BODY).await?;
    Ok(Fetched::Modified {
        value: bytes,
        etag: new_etag,
    })
}

/// Append `chunk`, or refuse if it would push the buffer past `limit`.
///
/// The check happens *before* the copy: a body that blows the ceiling must never
/// be materialized in order to discover that it was too big.
fn push_capped(buf: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<(), FetchError> {
    if buf.len() + chunk.len() > limit {
        return Err(FetchError::TooLarge { limit });
    }
    buf.extend_from_slice(chunk);
    Ok(())
}

/// Buffer the response body, failing once it passes `limit`.
async fn read_capped(resp: &mut Response, limit: usize) -> Result<Vec<u8>, FetchError> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        push_capped(&mut buf, &chunk, limit)?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_within_the_cap_is_buffered_whole() {
        let mut buf = Vec::new();
        assert!(push_capped(&mut buf, b"abc", 6).is_ok());
        assert!(push_capped(&mut buf, b"def", 6).is_ok());
        assert_eq!(
            buf, b"abcdef",
            "a body exactly at the cap is still accepted"
        );
    }

    #[test]
    fn a_body_over_the_cap_is_rejected_without_being_buffered() {
        let mut buf = Vec::new();
        push_capped(&mut buf, b"abcde", 6).unwrap();

        let err = push_capped(&mut buf, b"fg", 6).unwrap_err();
        assert!(matches!(err, FetchError::TooLarge { limit: 6 }));
        // The chunk that would have crossed the cap is never copied in, so a
        // decompression bomb cannot be materialized in order to be detected.
        assert_eq!(buf, b"abcde");
    }
}
