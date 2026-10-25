//! RSS 2.0 and Atom, as the feeds bot needs them: the channel's title,
//! description and picture, and each post's id, time, link, HTML and images.
//!
//! Feeds are untrusted. The parser never fetches anything (no DTDs, no
//! entities beyond XML's own), keeps at most [`MAX_ITEMS`] posts and cuts
//! every field at [`MAX_FIELD`] bytes. Element names are matched with their
//! usual prefixes (`media:`, `content:`), as Mastodon and most feeds write
//! them.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

pub const MAX_ITEMS: usize = 40;
pub const MAX_FIELD: usize = 16 * 1024;
const MAX_IMAGES: usize = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feed {
    pub title: String,
    pub description: String,
    pub image: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item {
    pub guid: String,
    pub link: String,
    /// 0 when the feed gives no time it could read.
    pub published_ms: u64,
    pub title: String,
    /// The post as HTML (RSS `description`, Atom `content` or `summary`).
    pub html: String,
    pub images: Vec<Image>,
    /// `media:rating` said `adult`.
    pub adult: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Image {
    pub url: String,
    pub description: String,
}

fn attr(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref().eq_ignore_ascii_case(name))
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn push_limited(out: &mut String, text: &str) {
    for c in text.chars() {
        if out.len() + c.len_utf8() > MAX_FIELD {
            return;
        }
        out.push(c);
    }
}

pub fn parse(xml: &[u8]) -> Result<Feed, String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut p = Parser::default();
    let mut buf = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| format!("not a feed: {e}"))?
        {
            Event::Start(e) => p.open(&e, false)?,
            // An empty element (`<a/>`) has no end event.
            Event::Empty(e) => p.open(&e, true)?,
            Event::Text(t) => {
                let t = t.unescape().map_err(|e| format!("bad text: {e}"))?;
                push_limited(&mut p.text, &t);
            }
            Event::CData(c) => push_limited(&mut p.text, &String::from_utf8_lossy(&c)),
            Event::End(_) => p.close(),
            Event::DocType(_) => return Err("feeds with a DTD are refused".into()),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !p.saw_root {
        return Err("empty feed".into());
    }
    Ok(p.feed)
}

#[derive(Default)]
struct Parser {
    feed: Feed,
    stack: Vec<String>,
    item: Option<Item>,
    /// RSS `description` or Atom `summary`, used when there is no fuller
    /// `content:encoded` or Atom `content`.
    summary: String,
    text: String,
    saw_root: bool,
}

impl Parser {
    fn open(&mut self, e: &BytesStart<'_>, empty: bool) -> Result<(), String> {
        let name = String::from_utf8_lossy(e.name().as_ref()).to_ascii_lowercase();
        if !self.saw_root {
            if !matches!(name.as_str(), "rss" | "feed" | "rdf:rdf") {
                return Err("not an RSS or Atom feed".into());
            }
            self.saw_root = true;
        }
        self.text.clear();
        match name.as_str() {
            "item" | "entry" => {
                self.item = Some(Item::default());
                self.summary.clear();
            }
            "media:content" | "media:thumbnail" | "enclosure" => {
                if let (Some(it), Some(url)) = (self.item.as_mut(), attr(e, b"url")) {
                    let kind = attr(e, b"type").unwrap_or_default();
                    let medium = attr(e, b"medium").unwrap_or_default();
                    // Mastodon marks videos `medium="image"` too: a stated
                    // type wins.
                    let image = if kind.is_empty() {
                        medium == "image" || name == "media:thumbnail"
                    } else {
                        kind.starts_with("image/")
                    };
                    if image
                        && it.images.len() < MAX_IMAGES
                        && !it.images.iter().any(|i| i.url == url)
                    {
                        it.images.push(Image {
                            url,
                            description: String::new(),
                        });
                    }
                }
            }
            "link" => {
                // Atom: <link rel="alternate" href="…"/>.
                if let (Some(it), Some(href)) = (self.item.as_mut(), attr(e, b"href")) {
                    let rel = attr(e, b"rel").unwrap_or_else(|| "alternate".into());
                    let kind = attr(e, b"type").unwrap_or_default();
                    if rel == "alternate" && it.link.is_empty() {
                        it.link = href;
                    } else if rel == "enclosure"
                        && kind.starts_with("image/")
                        && it.images.len() < MAX_IMAGES
                    {
                        it.images.push(Image {
                            url: href,
                            description: String::new(),
                        });
                    }
                }
            }
            _ => {}
        }
        if !empty {
            self.stack.push(name);
        }
        Ok(())
    }

    fn close(&mut self) {
        let Some(name) = self.stack.pop() else {
            return;
        };
        let value = std::mem::take(&mut self.text);
        let parent = self.stack.last().map(String::as_str).unwrap_or("");
        let feed = &mut self.feed;
        match (self.item.as_mut(), name.as_str()) {
            (Some(_), "item" | "entry") => {
                let mut done = self.item.take().expect("in an item");
                if done.html.is_empty() {
                    done.html = std::mem::take(&mut self.summary);
                }
                if feed.items.len() < MAX_ITEMS {
                    feed.items.push(done);
                }
            }
            (Some(it), "guid" | "id") => it.guid = value,
            (Some(it), "link") if it.link.is_empty() && !value.is_empty() => it.link = value,
            (Some(it), "pubdate" | "published" | "dc:date") => {
                it.published_ms = parse_date(&value).unwrap_or(it.published_ms)
            }
            (Some(it), "updated") if it.published_ms == 0 => {
                it.published_ms = parse_date(&value).unwrap_or(0)
            }
            (Some(it), "content:encoded" | "content") => it.html = value,
            (Some(_), "description" | "summary") => {
                if self.summary.is_empty() {
                    self.summary = value;
                }
            }
            (Some(it), "title") => it.title = value,
            (Some(it), "media:description") if parent == "media:content" => {
                if let Some(last) = it.images.last_mut() {
                    last.description = value;
                }
            }
            (Some(it), "media:rating") if value.trim().eq_ignore_ascii_case("adult") => {
                it.adult = true;
            }
            (None, "title") if matches!(parent, "channel" | "feed") => feed.title = value,
            (None, "description" | "subtitle") if matches!(parent, "channel" | "feed") => {
                feed.description = value
            }
            (None, "url") if parent == "image" => feed.image = Some(value),
            (None, "icon" | "logo") if parent == "feed" && feed.image.is_none() => {
                feed.image = Some(value)
            }
            _ => {}
        }
    }
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// RFC 822/2822 (`Wed, 21 Oct 2026 10:00:00 +0000`) or RFC 3339
/// (`2026-10-21T10:00:00.000Z`), as milliseconds since 1970.
pub fn parse_date(text: &str) -> Option<u64> {
    let text = text.trim();
    let (y, mo, d, h, mi, s, offset) = if text.as_bytes().get(4) == Some(&b'-') {
        let (date, rest) = text.split_once(['T', 't', ' '])?;
        let mut p = date.split('-');
        let y: i64 = p.next()?.parse().ok()?;
        let mo: u32 = p.next()?.parse().ok()?;
        let d: u32 = p.next()?.parse().ok()?;
        let split = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
        let (time, zone) = rest.split_at(split);
        let mut t = time.split(':');
        let h: i64 = t.next()?.parse().ok()?;
        let mi: i64 = t.next()?.parse().ok()?;
        let s: i64 = t.next().unwrap_or("0").split('.').next()?.parse().ok()?;
        (y, mo, d, h, mi, s, zone_offset(zone)?)
    } else {
        let rest = text.split_once(',').map_or(text, |(_, r)| r).trim();
        let mut p = rest.split_whitespace();
        let d: u32 = p.next()?.parse().ok()?;
        let month = p.next()?.to_ascii_lowercase();
        let mo = MONTHS.iter().position(|m| month.starts_with(m))? as u32 + 1;
        let mut y: i64 = p.next()?.parse().ok()?;
        if y < 100 {
            y += 2000;
        }
        let mut t = p.next()?.split(':');
        let h: i64 = t.next()?.parse().ok()?;
        let mi: i64 = t.next()?.parse().ok()?;
        let s: i64 = t.next().unwrap_or("0").parse().ok()?;
        (y, mo, d, h, mi, s, zone_offset(p.next().unwrap_or("Z"))?)
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let days = crate::tz::days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3600 + mi * 60 + s - offset;
    u64::try_from(secs).ok().map(|s| s * 1000)
}

/// Seconds east of UTC: `Z`, `GMT`, `UT`, `+0100`, `-05:00`.
fn zone_offset(zone: &str) -> Option<i64> {
    let zone = zone.trim();
    match zone.to_ascii_uppercase().as_str() {
        "" | "Z" | "GMT" | "UT" | "UTC" => return Some(0),
        "EST" => return Some(-5 * 3600),
        "EDT" => return Some(-4 * 3600),
        "CST" => return Some(-6 * 3600),
        "CDT" => return Some(-5 * 3600),
        "MST" => return Some(-7 * 3600),
        "MDT" => return Some(-6 * 3600),
        "PST" => return Some(-8 * 3600),
        "PDT" => return Some(-7 * 3600),
        _ => {}
    }
    let sign = match zone.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: String = zone[1..].chars().filter(|c| *c != ':').collect();
    if digits.len() != 4 {
        return None;
    }
    let h: i64 = digits[..2].parse().ok()?;
    let m: i64 = digits[2..].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

/// A post's HTML as plain text, and Mastodon's content warning when the
/// feed carries one (`<p><strong>…</strong> warning</p><hr />post`).
pub fn post_text(html: &str) -> (String, String) {
    let trimmed = html.trim_start();
    if trimmed.starts_with("<p><strong>") {
        if let Some(hr) = trimmed.find("<hr") {
            let head = &trimmed[..hr];
            let body = trimmed[hr..].split_once('>').map_or("", |(_, b)| b);
            let warning = head
                .split_once("</strong>")
                .map_or(head, |(_, after)| after);
            return (html_to_text(body), html_to_text(warning));
        }
    }
    (html_to_text(html), String::new())
}

/// HTML as plain text: tags dropped, paragraphs and line breaks kept, links
/// kept as their text (Mastodon puts the whole address there), entities
/// decoded.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end].to_ascii_lowercase();
        if tag.starts_with("br") {
            out.push('\n');
        } else if tag == "/p" {
            out.push_str("\n\n");
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    let text = decode_entities(&out);
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let mut joined = lines.join("\n");
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    joined.trim().to_string()
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32)
                .filter(|c| !c.is_control() || *c == '\n'),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTODON: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:webfeeds="http://webfeeds.org/rss/1.0" xmlns:media="http://search.yahoo.com/mrss/">
  <channel>
    <title>Bob B</title>
    <description>Public posts from @bob@example.org</description>
    <link>https://example.org/@bob</link>
    <image>
      <url>https://files.example.org/accounts/avatars/1/original/a.png</url>
      <title>Bob B</title>
      <link>https://example.org/@bob</link>
    </image>
    <item>
      <guid isPermaLink="true">https://example.org/@bob/2</guid>
      <link>https://example.org/@bob/2</link>
      <pubDate>Wed, 21 Oct 2026 10:00:00 +0000</pubDate>
      <description>&lt;p&gt;Hello &amp;amp; welcome &lt;a href="https://x.org/a"&gt;&lt;span class="invisible"&gt;https://&lt;/span&gt;x.org/a&lt;/a&gt;&lt;/p&gt;&lt;p&gt;Second&lt;br /&gt;line&lt;/p&gt;</description>
      <media:content url="https://files.example.org/media_attachments/files/1/original/c.jpg" type="image/jpeg" fileSize="1000" medium="image">
        <media:rating scheme="urn:simple">nonadult</media:rating>
        <media:description type="plain">A cat</media:description>
      </media:content>
      <media:content url="https://files.example.org/media_attachments/files/1/original/v.mp4" type="video/mp4" medium="image"/>
      <category>cats</category>
    </item>
    <item>
      <guid isPermaLink="true">https://example.org/@bob/1</guid>
      <link>https://example.org/@bob/1</link>
      <pubDate>Tue, 20 Oct 2026 09:30:00 -0400</pubDate>
      <description>&lt;p&gt;&lt;strong&gt;Content warning:&lt;/strong&gt; spoilers&lt;/p&gt;&lt;hr /&gt;&lt;p&gt;The end&lt;/p&gt;</description>
      <media:content url="https://files.example.org/media_attachments/files/2/original/d.jpg" type="image/jpeg" medium="image">
        <media:rating scheme="urn:simple">adult</media:rating>
      </media:content>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn reads_a_mastodon_feed() {
        let feed = parse(MASTODON.as_bytes()).unwrap();
        assert_eq!(feed.title, "Bob B");
        assert_eq!(
            feed.image.as_deref(),
            Some("https://files.example.org/accounts/avatars/1/original/a.png")
        );
        assert_eq!(feed.items.len(), 2);
        let first = &feed.items[0];
        assert_eq!(first.guid, "https://example.org/@bob/2");
        assert_eq!(first.link, "https://example.org/@bob/2");
        assert_eq!(
            first.published_ms,
            parse_date("2026-10-21T10:00:00Z").unwrap()
        );
        assert_eq!(first.images.len(), 1);
        assert_eq!(first.images[0].description, "A cat");
        assert!(!first.adult);
        assert_eq!(
            post_text(&first.html),
            (
                "Hello & welcome https://x.org/a\n\nSecond\nline".to_string(),
                String::new()
            )
        );
        let second = &feed.items[1];
        assert!(second.adult);
        assert_eq!(
            second.published_ms,
            parse_date("2026-10-20T13:30:00Z").unwrap()
        );
        assert_eq!(
            post_text(&second.html),
            ("The end".to_string(), "spoilers".to_string())
        );
    }

    #[test]
    fn reads_atom() {
        let atom = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>A blog</title><subtitle>Notes</subtitle><icon>https://blog.example/i.png</icon>
  <entry>
    <id>tag:blog.example,2026:1</id>
    <title>Post</title>
    <link rel="alternate" href="https://blog.example/1"/>
    <link rel="enclosure" type="image/jpeg" href="https://blog.example/1.jpg"/>
    <updated>2026-10-21T10:00:00.123+02:00</updated>
    <content type="html"><![CDATA[<p>Body</p>]]></content>
  </entry>
</feed>"#;
        let feed = parse(atom.as_bytes()).unwrap();
        assert_eq!(feed.title, "A blog");
        assert_eq!(feed.description, "Notes");
        assert_eq!(feed.image.as_deref(), Some("https://blog.example/i.png"));
        let item = &feed.items[0];
        assert_eq!(item.guid, "tag:blog.example,2026:1");
        assert_eq!(item.link, "https://blog.example/1");
        assert_eq!(item.images[0].url, "https://blog.example/1.jpg");
        assert_eq!(
            item.published_ms,
            parse_date("2026-10-21T08:00:00Z").unwrap()
        );
        assert_eq!(html_to_text(&item.html), "Body");
    }

    #[test]
    fn refuses_what_is_not_a_feed() {
        assert!(parse(b"<html><body>hi</body></html>").is_err());
        assert!(parse(b"").is_err());
        assert!(parse(b"not xml at all").is_err());
        let dtd = br#"<?xml version="1.0"?><!DOCTYPE rss [<!ENTITY a "aaaa">]><rss><channel><title>&a;</title></channel></rss>"#;
        assert!(parse(dtd).is_err());
    }

    #[test]
    fn bounds_items_and_fields() {
        let mut xml = String::from("<rss><channel><title>T</title>");
        for i in 0..100 {
            xml.push_str(&format!(
                "<item><guid>{i}</guid><description>{}</description></item>",
                "x".repeat(40_000)
            ));
        }
        xml.push_str("</channel></rss>");
        let feed = parse(xml.as_bytes()).unwrap();
        assert_eq!(feed.items.len(), MAX_ITEMS);
        assert!(feed.items[0].html.len() <= MAX_FIELD);
    }

    #[test]
    fn dates_and_entities() {
        assert_eq!(parse_date("Thu, 01 Jan 1970 00:00:10 GMT"), Some(10_000));
        assert_eq!(parse_date("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(parse_date("yesterday"), None);
        assert_eq!(
            html_to_text("a &#38; b &#x263A; &bogus; &"),
            "a & b ☺ &bogus; &"
        );
    }
}
