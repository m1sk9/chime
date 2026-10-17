use std::time::Duration;

use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH};
use reqwest::redirect::Policy;
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

/// Follow redirects only to https, so a polled endpoint or a webhook cannot move a
/// request onto cleartext after config validation required https.
pub(crate) fn https_redirects() -> Policy {
    Policy::custom(
        |attempt| match refuse_redirect(attempt.url(), attempt.previous().len()) {
            Some(reason) => attempt.error(reason),
            None => attempt.follow(),
        },
    )
}

/// Why a redirect to `target` after `hops` earlier requests is not followed.
fn refuse_redirect(target: &Url, hops: usize) -> Option<&'static str> {
    if target.scheme() != "https" {
        Some("redirect to a non-https URL")
    } else if hops >= MAX_REDIRECTS {
        Some("too many redirects")
    } else {
        None
    }
}

/// reqwest's own default limit.
const MAX_REDIRECTS: usize = 10;

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
/// raw bytes for `fetch_conditional`, normalized incidents for `StatusSource`.
#[derive(Debug)]
pub enum Fetched<T> {
    NotModified,
    Modified { value: T, etag: Option<String> },
}

/// The raw-body fetch behind watch and rss feeds. A trait so scheduler tests can
/// serve canned bodies.
#[allow(async_fn_in_trait)]
pub(crate) trait BodySource {
    async fn fetch(
        &self,
        url: &Url,
        etag: Option<&str>,
        accept: &'static str,
    ) -> Result<Fetched<Vec<u8>>, FetchError>;
}

#[derive(Debug, Clone)]
pub struct HttpSource {
    client: Client,
}

impl HttpSource {
    pub fn new(client: Client) -> Self {
        HttpSource { client }
    }
}

impl BodySource for HttpSource {
    async fn fetch(
        &self,
        url: &Url,
        etag: Option<&str>,
        accept: &'static str,
    ) -> Result<Fetched<Vec<u8>>, FetchError> {
        fetch_conditional(&self.client, url, etag, accept).await
    }
}

/// Conditional GET with the body capped at `MAX_BODY`. `accept` is sent verbatim.
pub(crate) async fn fetch_conditional(
    client: &Client,
    url: &Url,
    etag: Option<&str>,
    accept: &'static str,
) -> Result<Fetched<Vec<u8>>, FetchError> {
    // `Accept` is required, not merely polite. The status page CDN answers with
    // `Vary: Accept, Accept-Encoding`, and a request that omits `Accept` lands on
    // a variant that returns 200 to every `If-None-Match` — verified against
    // status.claude.com. Dropping this header silently disables conditional GETs.
    let mut request = client
        .get(url.clone())
        .timeout(FETCH_TIMEOUT)
        .header(ACCEPT, accept);
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

    /// Answers one request on a loopback port with `response` and hands back the
    /// request head it received.
    fn serve_once(response: &'static str) -> (Url, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/feed", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                head.push(byte[0]);
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(head).unwrap()
        });
        (url, server)
    }

    fn client() -> Client {
        Client::builder().no_proxy().build().unwrap()
    }

    #[test]
    fn redirects_are_followed_only_to_https_and_within_the_limit() {
        let https = Url::parse("https://status.example.com/feed").unwrap();
        let http = Url::parse("http://status.example.com/feed").unwrap();
        assert_eq!(refuse_redirect(&https, 1), None);
        assert!(refuse_redirect(&http, 1).is_some());
        assert!(refuse_redirect(&https, MAX_REDIRECTS).is_some());
    }

    #[tokio::test]
    async fn a_redirect_to_a_non_https_url_is_refused() {
        let (url, _server) = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/elsewhere\r\nContent-Length: 0\r\n\r\n",
        );
        let client = Client::builder()
            .no_proxy()
            .redirect(https_redirects())
            .build()
            .unwrap();

        let err = HttpSource::new(client)
            .fetch(&url, None, "application/json")
            .await
            .unwrap_err();

        assert!(
            matches!(&err, FetchError::Request(e) if e.is_redirect()),
            "{err}"
        );
    }

    #[tokio::test]
    async fn http_source_sends_the_callers_accept_and_etag() {
        let (url, server) = serve_once("HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\n\r\n");

        let fetched = HttpSource::new(client())
            .fetch(&url, Some("W/\"1\""), "application/feed+json")
            .await
            .unwrap();

        assert!(matches!(fetched, Fetched::NotModified));
        let head = server.join().unwrap().to_ascii_lowercase();
        assert!(
            head.contains("\r\naccept: application/feed+json\r\n"),
            "{head}"
        );
        assert!(head.contains("\r\nif-none-match: w/\"1\"\r\n"), "{head}");
    }

    #[tokio::test]
    async fn http_source_returns_the_body_with_its_etag() {
        let (url, server) = serve_once(
            "HTTP/1.1 200 OK\r\nETag: W/\"2\"\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        );

        let fetched = HttpSource::new(client())
            .fetch(&url, None, "application/json")
            .await
            .unwrap();

        let Fetched::Modified { value, etag } = fetched else {
            panic!("expected a body");
        };
        assert_eq!(value, b"ok");
        assert_eq!(etag.as_deref(), Some("W/\"2\""));
        assert!(
            !server
                .join()
                .unwrap()
                .to_ascii_lowercase()
                .contains("if-none-match")
        );
    }
}
