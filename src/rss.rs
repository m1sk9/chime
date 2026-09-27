use std::collections::HashSet;

use chrono::{DateTime, Utc};
use feed_rs::parser::ParseFeedError;
use reqwest::Client;
use url::Url;

use crate::fetch::{FetchError, Fetched, fetch_conditional};
use crate::notifier::{DiscordMessage, Embed};
use crate::runtime::RunRss;

/// Discord fuchsia. The other brand colours are taken: blurple is a release, and
/// green/yellow/orange/red/grey encode incident severity.
const COLOR_ENTRY: u32 = 0xEB459E;

/// Ordered by preference; `*/*` last so a server that only knows `text/html`
/// still answers rather than 406-ing, and the parser reports the real problem.
const FEED_ACCEPT: &str = "application/rss+xml, application/atom+xml, application/feed+json, application/xml;q=0.9, text/xml;q=0.8, */*;q=0.1";

/// Only the decode error lives here: fetch errors are handled by the scheduler
/// before parsing, exactly as `poll_feed` does. A `Fetch` variant would be dead
/// code and fail `clippy -D warnings` (binary crate: `pub` does not exempt it).
#[derive(Debug, thiserror::Error)]
pub enum RssError {
    #[error("response is not an RSS, Atom or JSON feed: {0}")]
    Decode(#[source] ParseFeedError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RssEntry {
    /// The dedup key: the feed's own id, or feed-rs's hash of link + title.
    pub id: String,
    pub title: Option<String>,
    pub link: Option<String>,
    /// Already rendered to plain text.
    pub body: Option<String>,
    /// `published`, else `updated`. Display only; never the embed timestamp.
    pub published: Option<DateTime<Utc>>,
}

/// Pure. `url` is the base for relative links and for the id of an entry that has
/// a title but no link (feed-rs would otherwise mint a random UUID per parse).
pub fn parse_feed(body: &[u8], url: &Url) -> Result<Vec<RssEntry>, RssError> {
    let feed = feed_rs::parser::Builder::new()
        .base_uri(Some(url.as_str()))
        .build()
        .parse(body)
        .map_err(RssError::Decode)?;
    Ok(feed.entries.into_iter().filter_map(normalize).collect())
}

fn normalize(e: feed_rs::model::Entry) -> Option<RssEntry> {
    let title = e.title.map(|t| t.content).filter(|t| !t.trim().is_empty());
    let link = e
        .links
        .iter()
        .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
        .or(e.links.first())
        .map(|l| l.href.clone());
    // Neither a link nor a title: no stable id (UUID per parse) and nothing to show.
    if title.is_none() && link.is_none() {
        return None;
    }
    let body = [e.summary.map(|t| t.content), e.content.and_then(|c| c.body)]
        .into_iter()
        .flatten()
        .map(|s| html_to_text(&s))
        .find(|s| !s.is_empty());
    Some(RssEntry {
        id: e.id,
        title,
        link,
        body,
        published: e.published.or(e.updated),
    })
}

/// Renders an entry body to the plain text an embed description can show.
///
/// Why not an HTML parser: the output is a short preview under a link to the real
/// page, and the only structure worth keeping is line breaks and list bullets.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    // Starts `true` so leading whitespace is dropped like the rest of a line's indent.
    let mut after_space = true;
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        push_text(&mut out, &rest[..open], &mut after_space);
        let tail = &rest[open + 1..];
        let Some(close) = tail.find('>') else {
            push_text(&mut out, &rest[open..], &mut after_space);
            rest = "";
            break;
        };
        push_tag(&mut out, &tail[..close], &mut after_space);
        rest = &tail[close + 1..];
    }
    push_text(&mut out, rest, &mut after_space);

    let decoded = decode_entities(&out);
    let mut lines: Vec<&str> = Vec::new();
    for line in decoded.lines().map(str::trim) {
        if line.is_empty() && lines.last().is_some_and(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}

/// Whitespace runs collapse to one space, as HTML renders them, so the source
/// indentation between `<li>`s does not turn into blank lines.
fn push_text(out: &mut String, text: &str, after_space: &mut bool) {
    for c in text.chars() {
        if c.is_whitespace() {
            if !*after_space {
                out.push(' ');
                *after_space = true;
            }
        } else {
            out.push(c);
            *after_space = false;
        }
    }
}

fn push_tag(out: &mut String, inner: &str, after_space: &mut bool) {
    let (closing, name) = match inner.strip_prefix('/') {
        Some(n) => (true, n),
        None => (false, inner),
    };
    let name: String = name
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match name.as_str() {
        "br" => push_break(out),
        "li" if !closing => {
            // Why not always a newline: after a nested list's `</ul>` the line is
            // already open, and a second break would leave a blank line in the list.
            if !(out.is_empty() || out.ends_with('\n')) {
                out.push('\n');
            }
            out.push_str("- ");
        }
        // Why only the closing tag: `<li>` already starts a line, so an opening
        // `<ul>` would put a blank line above every nested list.
        "ul" | "ol" if closing => push_break(out),
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "pre" | "tr"
        | "table" => push_break(out),
        _ => return,
    }
    *after_space = true;
}

/// A block inside a list item (`<li><p>…`) must not strand the bullet on a line
/// of its own.
fn push_break(out: &mut String) {
    let bullet_open = out
        .strip_suffix("- ")
        .is_some_and(|line_start| line_start.is_empty() || line_start.ends_with('\n'));
    if !bullet_open {
        out.push('\n');
    }
}

/// One left-to-right pass, so `&amp;lt;` decodes exactly once to `&lt;`. Runs
/// after tags are stripped, so an escaped `&lt;b&gt;` stays literal text.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp + 1..];
        let decoded = tail
            .find(';')
            .filter(|&end| end <= 10)
            .and_then(|end| decode_entity(&tail[..end]).map(|c| (c, end)));
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Per-feed polling state. In-memory only, like `PageState`: a restart
/// re-baselines rather than replaying the feed.
#[derive(Debug, Default)]
pub struct RssState {
    seen: HashSet<String>,
    initialized: bool,
    pub etag: Option<String>,
}

impl RssState {
    /// `false` until the first `diff` — the scheduler logs the baseline once.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

/// Every entry whose id has not been seen, oldest first. The first call is the
/// cold-start baseline: it records every id and reports nothing. Ids are inserted
/// *before* returning, so a Discord failure never re-sends an entry.
pub fn diff(state: &mut RssState, entries: &[RssEntry]) -> Vec<RssEntry> {
    // Prune only against a non-empty feed (same reasoning as status: an empty
    // body must not drop the baseline and replay the whole feed when it returns).
    if !entries.is_empty() {
        let current: HashSet<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        state.seen.retain(|id| current.contains(id.as_str()));
    }
    let first_poll = !state.initialized;
    state.initialized = true;
    let mut events: Vec<RssEntry> = entries
        .iter()
        .filter(|e| state.seen.insert(e.id.clone()) && !first_poll)
        .cloned()
        .collect();
    // Dated entries ascending; undated ones after them in feed order. `sort_by_key`
    // is stable, and `(true, None)` compares equal, so feed order is what survives.
    events.sort_by_key(|e| (e.published.is_none(), e.published));
    events
}

pub fn build_message(
    feed: &RunRss,
    entry: &RssEntry,
    detected_at: DateTime<Utc>,
) -> DiscordMessage {
    let mut embed = Embed::new(entry.title.as_deref().unwrap_or(&feed.name), COLOR_ENTRY);
    if let Some(link) = &entry.link {
        embed = embed.with_url(link);
    }
    if let Some(body) = &entry.body {
        embed = embed.with_description(body);
    }
    if let Some(at) = entry.published {
        embed = embed.with_field(
            "Published",
            &at.format("%Y-%m-%d %H:%M UTC").to_string(),
            true,
        );
    }
    // Why detection time and not the entry's date: a feed may publish an entry
    // backdated, or re-date it on edit, and the timestamp says when chime saw it.
    embed = embed
        .with_footer(&feed.host)
        .with_timestamp(&detected_at.to_rfc3339());
    DiscordMessage::embed(embed).with_identity(&feed.display_name, feed.avatar_url.as_deref())
}

#[allow(async_fn_in_trait)]
pub(crate) trait RssSource {
    async fn fetch(&self, url: &Url, etag: Option<&str>) -> Result<Fetched<Vec<u8>>, FetchError>;
}

#[derive(Debug, Clone)]
pub struct FeedEndpoint {
    client: Client,
}

impl FeedEndpoint {
    pub fn new(client: Client) -> Self {
        FeedEndpoint { client }
    }
}

impl RssSource for FeedEndpoint {
    async fn fetch(&self, url: &Url, etag: Option<&str>) -> Result<Fetched<Vec<u8>>, FetchError> {
        fetch_conditional(&self.client, url, etag, FEED_ACCEPT).await
    }
}

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use test_support::*;

#[cfg(test)]
mod test_support {
    use super::*;

    /// Shaped after the Claude Code changelog as measured on 2026-09-27; the last
    /// item has no `<guid>`.
    pub(crate) const RSS2_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title>Claude Code Changelog</title>
    <link>https://code.claude.com/docs/en/changelog</link>
    <description>Release notes for Claude Code</description>
    <item>
      <title><![CDATA[2.3.1]]></title>
      <link>https://code.claude.com/docs/en/changelog#2-3-1</link>
      <guid isPermaLink="false">a1b2c3d4e5f60718</guid>
      <pubDate>Fri, 26 Sep 2026 18:00:00 GMT</pubDate>
      <content:encoded><![CDATA[<ul>
  <li>Fixed <code>/resume</code> on Windows</li>
  <li>Faster startup</li>
</ul>]]></content:encoded>
    </item>
    <item>
      <title><![CDATA[2.3.0]]></title>
      <link>https://code.claude.com/docs/en/changelog#2-3-0</link>
      <guid isPermaLink="false">0f1e2d3c4b5a6978</guid>
      <pubDate>Wed, 24 Sep 2026 17:30:00 GMT</pubDate>
      <content:encoded><![CDATA[<ul><li>Added RSS feed</li></ul>]]></content:encoded>
    </item>
    <item>
      <title>2.2.9</title>
      <link>https://code.claude.com/docs/en/changelog#2-2-9</link>
      <pubDate>Mon, 22 Sep 2026 09:00:00 GMT</pubDate>
    </item>
  </channel>
</rss>"#;

    pub(crate) const ATOM_FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Example Blog</title>
  <id>urn:uuid:60a76c80-d399-11d9-b93C-0003939e0af6</id>
  <updated>2026-09-25T12:00:00Z</updated>
  <entry>
    <title>Hello Atom</title>
    <id>tag:blog.example,2026:1</id>
    <link rel="alternate" href="https://blog.example/hello"/>
    <link rel="edit" href="https://blog.example/edit/1"/>
    <updated>2026-09-25T12:00:00Z</updated>
    <content type="html">&lt;p&gt;First &amp;amp; best&lt;/p&gt;</content>
  </entry>
</feed>"#;

    pub(crate) fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    pub(crate) fn mk_entry(id: &str, published: Option<&str>) -> RssEntry {
        RssEntry {
            id: id.to_string(),
            title: Some(format!("entry {id}")),
            link: Some(format!("https://blog.example/{id}")),
            body: Some(format!("body of {id}")),
            published: published.map(ts),
        }
    }

    /// Drive a state past its cold-start baseline so a test can assert on real diffs.
    pub(crate) fn baselined(entries: &[RssEntry]) -> RssState {
        let mut state = RssState::default();
        assert!(diff(&mut state, entries).is_empty());
        state
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::runtime::mk_run_rss;

    fn feed_url() -> Url {
        Url::parse("https://code.claude.com/docs/en/changelog/rss.xml").unwrap()
    }

    fn parse(body: &str) -> Vec<RssEntry> {
        parse_feed(body.as_bytes(), &feed_url()).unwrap()
    }

    fn rss_item(inner: &str) -> String {
        format!(
            "<rss version=\"2.0\" xmlns:content=\"http://purl.org/rss/1.0/modules/content/\"><channel><title>t</title><item>{inner}</item></channel></rss>"
        )
    }

    // --- parse_feed ---

    #[test]
    fn rss2_items_yield_guid_link_title_body_and_pubdate() {
        let entries = parse(RSS2_FIXTURE);
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0],
            RssEntry {
                id: "a1b2c3d4e5f60718".to_string(),
                title: Some("2.3.1".to_string()),
                link: Some("https://code.claude.com/docs/en/changelog#2-3-1".to_string()),
                body: Some("- Fixed /resume on Windows\n- Faster startup".to_string()),
                published: Some(ts("2026-09-26T18:00:00Z")),
            }
        );
        assert_eq!(entries[1].id, "0f1e2d3c4b5a6978");
        assert!(entries[2].body.is_none());
    }

    #[test]
    fn content_encoded_is_used_when_description_is_absent() {
        let entries = parse(&rss_item(
            "<title>x</title><guid>g</guid><content:encoded><![CDATA[<p>from content</p>]]></content:encoded>",
        ));
        assert_eq!(entries[0].body.as_deref(), Some("from content"));
    }

    #[test]
    fn description_is_preferred_over_content_encoded() {
        let entries = parse(&rss_item(
            "<title>x</title><guid>g</guid><description>short summary</description><content:encoded><![CDATA[<p>full body</p>]]></content:encoded>",
        ));
        assert_eq!(entries[0].body.as_deref(), Some("short summary"));
    }

    #[test]
    fn an_item_without_guid_gets_the_same_id_on_every_parse() {
        let first = parse(RSS2_FIXTURE);
        let second = parse(RSS2_FIXTURE);
        assert_eq!(first[2].id, second[2].id);

        let no_link = rss_item("<title>only a title</title>");
        assert_eq!(parse(&no_link)[0].id, parse(&no_link)[0].id);
    }

    #[test]
    fn an_item_with_neither_link_nor_title_is_dropped() {
        let entries = parse(&rss_item("<description>orphan</description>"));
        assert!(entries.is_empty());
    }

    #[test]
    fn atom_entries_yield_id_alternate_link_updated_and_html_content() {
        let entries = parse(ATOM_FIXTURE);
        assert_eq!(
            entries,
            vec![RssEntry {
                id: "tag:blog.example,2026:1".to_string(),
                title: Some("Hello Atom".to_string()),
                link: Some("https://blog.example/hello".to_string()),
                body: Some("First & best".to_string()),
                published: Some(ts("2026-09-25T12:00:00Z")),
            }]
        );
    }

    #[test]
    fn a_relative_entry_link_is_resolved_against_the_feed_url() {
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>t</title><id>f</id><updated>2026-09-25T12:00:00Z</updated>
<entry><title>rel</title><id>e</id><updated>2026-09-25T12:00:00Z</updated><link href="posts/1"/></entry></feed>"#;
        let entries = parse(atom);
        assert_eq!(
            entries[0].link.as_deref(),
            Some("https://code.claude.com/docs/en/changelog/posts/1")
        );
    }

    #[test]
    fn a_non_feed_body_is_a_decode_error() {
        for body in [&b"<html>maintenance</html>"[..], b"{}", b""] {
            let r = parse_feed(body, &feed_url());
            assert!(
                matches!(r, Err(RssError::Decode(_))),
                "{:?}",
                String::from_utf8_lossy(body)
            );
        }
    }

    // --- html_to_text ---

    #[test]
    fn list_items_become_dash_lines() {
        assert_eq!(html_to_text("<ul><li>a</li><li>b</li></ul>"), "- a\n- b");
    }

    #[test]
    fn paragraphs_and_breaks_become_newlines() {
        assert_eq!(
            html_to_text("<p>one<br>two<br/>three</p>"),
            "one\ntwo\nthree"
        );
        assert_eq!(html_to_text("<p>a</p><p>b</p>"), "a\n\nb");
    }

    #[test]
    fn unknown_tags_are_stripped_and_their_text_kept() {
        assert_eq!(
            html_to_text(
                r#"Use <a href="https://x"><code>/help</code></a> <strong>now</strong><!-- c -->"#
            ),
            "Use /help now"
        );
    }

    #[test]
    fn entities_are_decoded_after_tags_are_stripped() {
        assert_eq!(
            html_to_text(
                "&lt;b&gt;literal&lt;/b&gt; &quot;q&quot; it&#39;s a&nbsp;b &#x2014; &#8217;"
            ),
            "<b>literal</b> \"q\" it's a b \u{2014} \u{2019}"
        );
    }

    #[test]
    fn an_encoded_ampersand_decodes_exactly_once() {
        assert_eq!(
            html_to_text("&amp;lt; &amp;amp; &unknown; & x"),
            "&lt; &amp; &unknown; & x"
        );
    }

    #[test]
    fn source_indentation_does_not_produce_blank_lines() {
        let html = "<ul>\n    <li>\n      first\n    </li>\n    <li>second</li>\n</ul>";
        assert_eq!(html_to_text(html), "- first\n- second");
    }

    #[test]
    fn runs_of_blank_lines_collapse_to_one() {
        assert_eq!(html_to_text("a<br><br><br><br>b"), "a\n\nb");
    }

    #[test]
    fn an_unterminated_tag_keeps_the_rest_as_text() {
        assert_eq!(html_to_text("a < b and <b"), "a < b and <b");
    }

    #[test]
    fn nested_lists_are_flattened_to_one_level() {
        assert_eq!(
            html_to_text("<ul><li>a<ul><li>b</li></ul></li><li>c</li></ul>"),
            "- a\n- b\n- c"
        );
        assert_eq!(html_to_text("<ul><li><p>loose</p></li></ul>"), "- loose");
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        assert_eq!(html_to_text("Just a sentence."), "Just a sentence.");
    }

    // --- diff ---

    #[test]
    fn first_poll_records_every_entry_silently() {
        let mut state = RssState::default();
        assert!(!state.is_initialized());
        let entries = vec![mk_entry("a", None), mk_entry("b", None)];
        assert!(diff(&mut state, &entries).is_empty());
        assert!(state.is_initialized());
        assert_eq!(state.seen.len(), 2);
    }

    #[test]
    fn a_new_entry_after_baseline_is_reported() {
        let a = mk_entry("a", Some("2026-09-01T00:00:00Z"));
        let b = mk_entry("b", Some("2026-09-02T00:00:00Z"));
        let mut state = baselined(std::slice::from_ref(&a));
        assert_eq!(diff(&mut state, &[b.clone(), a]), vec![b]);
    }

    #[test]
    fn a_reported_entry_is_recorded_before_the_caller_sends() {
        let a = mk_entry("a", None);
        let b = mk_entry("b", None);
        let mut state = baselined(std::slice::from_ref(&a));
        let next = [b, a];
        assert_eq!(diff(&mut state, &next).len(), 1);
        assert!(diff(&mut state, &next).is_empty());
    }

    #[test]
    fn new_entries_are_ordered_oldest_first() {
        let mut state = baselined(&[mk_entry("old", Some("2026-08-01T00:00:00Z"))]);
        let next = [
            mk_entry("late", Some("2026-09-03T00:00:00Z")),
            mk_entry("early", Some("2026-09-01T00:00:00Z")),
            mk_entry("old", Some("2026-08-01T00:00:00Z")),
        ];
        let ids: Vec<String> = diff(&mut state, &next).into_iter().map(|e| e.id).collect();
        assert_eq!(ids, ["early", "late"]);
    }

    #[test]
    fn undated_entries_come_after_dated_ones_in_feed_order() {
        let mut state = baselined(&[mk_entry("old", None)]);
        let next = [
            mk_entry("u1", None),
            mk_entry("dated", Some("2026-09-01T00:00:00Z")),
            mk_entry("u2", None),
        ];
        let ids: Vec<String> = diff(&mut state, &next).into_iter().map(|e| e.id).collect();
        assert_eq!(ids, ["dated", "u1", "u2"]);
    }

    #[test]
    fn entries_that_roll_out_of_the_feed_are_pruned_from_state() {
        let a = mk_entry("a", None);
        let b = mk_entry("b", None);
        let mut state = baselined(&[a.clone(), b.clone()]);

        // `a` aged out of the feed; `b` is unchanged.
        assert!(diff(&mut state, std::slice::from_ref(&b)).is_empty());
        assert_eq!(state.seen.len(), 1);

        // `a` reappearing is treated as new, not as already-seen.
        let events = diff(&mut state, &[a, b]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "a");
    }

    #[test]
    fn an_empty_feed_keeps_the_baseline() {
        let entries = vec![mk_entry("a", None)];
        let mut state = baselined(&entries);
        assert!(diff(&mut state, &[]).is_empty());
        assert!(diff(&mut state, &entries).is_empty());
    }

    // --- build_message ---

    fn detected() -> DateTime<Utc> {
        ts("2026-09-27T00:00:00Z")
    }

    #[test]
    fn message_carries_the_feed_identity_entry_link_and_body() {
        let mut feed = mk_run_rss("news", Duration::from_secs(900));
        feed.avatar_url = Some("https://example.com/a.png".to_string());
        let msg = build_message(&feed, &mk_entry("a", None), detected());

        assert_eq!(msg.username.as_deref(), Some("news Feed"));
        assert_eq!(msg.avatar_url.as_deref(), Some("https://example.com/a.png"));
        let embed = &msg.embeds[0];
        assert_eq!(embed.title, "entry a");
        assert_eq!(embed.url.as_deref(), Some("https://blog.example/a"));
        assert_eq!(embed.description.as_deref(), Some("body of a"));
        assert_eq!(embed.footer.as_ref().unwrap().text, "rss.news.example");
    }

    #[test]
    fn message_title_falls_back_to_the_feed_name() {
        let feed = mk_run_rss("news", Duration::from_secs(900));
        let mut entry = mk_entry("a", None);
        entry.title = None;
        assert_eq!(
            build_message(&feed, &entry, detected()).embeds[0].title,
            "news"
        );
    }

    #[test]
    fn published_is_a_field_and_the_timestamp_is_detection_time() {
        let feed = mk_run_rss("news", Duration::from_secs(900));
        let msg = build_message(
            &feed,
            &mk_entry("a", Some("2026-09-26T18:05:00Z")),
            detected(),
        );
        let embed = &msg.embeds[0];
        let fields: Vec<(&str, &str, bool)> = embed
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.value.as_str(), f.inline))
            .collect();
        assert_eq!(fields, vec![("Published", "2026-09-26 18:05 UTC", true)]);
        assert_eq!(
            embed.timestamp.as_deref(),
            Some("2026-09-27T00:00:00+00:00")
        );
    }

    #[test]
    fn message_without_link_body_or_date_omits_them() {
        let feed = mk_run_rss("news", Duration::from_secs(900));
        let entry = RssEntry {
            id: "a".to_string(),
            title: Some("t".to_string()),
            link: None,
            body: None,
            published: None,
        };
        let embed = &build_message(&feed, &entry, detected()).embeds[0];
        assert!(embed.url.is_none());
        assert!(embed.description.is_none());
        assert!(embed.fields.is_empty());
    }

    #[test]
    fn embed_color_is_the_entry_colour() {
        let feed = mk_run_rss("news", Duration::from_secs(900));
        let msg = build_message(&feed, &mk_entry("a", None), detected());
        assert_eq!(msg.embeds[0].color, COLOR_ENTRY);
    }
}
