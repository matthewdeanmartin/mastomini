//! Status text: Mastodon-compatible length counting, entity extraction and
//! HTML rendering (spec/03-limits.md "Character counting", spec/04 "Status
//! creation").

use unicode_segmentation::UnicodeSegmentation;

pub const MAX_CHARS: usize = 140;
pub const URL_CHARS: usize = 23;
pub const MAX_TEXT_BYTES: usize = 640;
pub const MAX_SPOILER_BYTES: usize = 120;
pub const MAX_URL_BYTES: usize = 256;
pub const MAX_TAGS: usize = 8;
pub const MAX_TAG_BYTES: usize = 40;
pub const MAX_USERNAME: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    Url(&'a str),
    /// `@user` or `@user@domain`. `user` excludes the `@`.
    Mention {
        raw: &'a str,
        user: &'a str,
        domain: Option<&'a str>,
    },
    /// `#tag`, `name` excludes the `#`.
    Tag {
        raw: &'a str,
        name: &'a str,
    },
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn prev_char(text: &str, i: usize) -> Option<char> {
    text[..i].chars().next_back()
}

fn url_at(text: &str, i: usize) -> Option<usize> {
    let rest = &text[i..];
    let lower: String = rest
        .chars()
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    let scheme = if lower.starts_with("https://") {
        8
    } else if lower.starts_with("http://") {
        7
    } else {
        return None;
    };
    if prev_char(text, i).is_some_and(is_word) {
        return None;
    }
    let mut end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
        .unwrap_or(rest.len());
    while let Some(last) = rest[..end].chars().next_back() {
        let unbalanced_paren =
            last == ')' && rest[..end].matches('(').count() < rest[..end].matches(')').count();
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"') || unbalanced_paren {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    (end > scheme).then_some(i + end)
}

/// Byte offsets of a mention: end, end of the username, and the domain span.
type MentionSpan = (usize, usize, Option<(usize, usize)>);

fn mention_at(text: &str, i: usize) -> Option<MentionSpan> {
    if prev_char(text, i).is_some_and(|c| is_word(c) || c == '@') {
        return None;
    }
    let bytes = text.as_bytes();
    let user_start = i + 1;
    let mut j = user_start;
    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
        j += 1;
    }
    if j == user_start || j - user_start > 30 {
        return None;
    }
    let user_end = j;
    let mut domain = None;
    if j < bytes.len() && bytes[j] == b'@' {
        let d_start = j + 1;
        let mut k = d_start;
        while k < bytes.len()
            && (bytes[k].is_ascii_alphanumeric()
                || bytes[k] == b'.'
                || bytes[k] == b'-'
                || bytes[k] == b':')
        {
            k += 1;
        }
        while k > d_start && bytes[k - 1] == b'.' {
            k -= 1;
        }
        let host = &text[d_start..k];
        if k > d_start && (host.contains('.') || host.contains(':')) {
            domain = Some((d_start, k));
            j = k;
        }
    }
    Some((j, user_end, domain))
}

fn tag_at(text: &str, i: usize) -> Option<usize> {
    if prev_char(text, i).is_some_and(|c| is_word(c) || c == '&' || c == '#') {
        return None;
    }
    let rest = &text[i + 1..];
    let len = rest.find(|c: char| !is_word(c)).unwrap_or(rest.len());
    let name = &rest[..len];
    if name.is_empty() || name.len() > MAX_TAG_BYTES || name.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(i + 1 + len)
}

/// Split text into plain text, URLs, mentions and hashtags.
pub fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        let found = match c {
            'h' | 'H' => url_at(text, i).map(|end| (end, Segment::Url(&text[i..end]))),
            '@' => mention_at(text, i).map(|(end, user_end, domain)| {
                (
                    end,
                    Segment::Mention {
                        raw: &text[i..end],
                        user: &text[i + 1..user_end],
                        domain: domain.map(|(s, e)| &text[s..e]),
                    },
                )
            }),
            '#' => tag_at(text, i).map(|end| {
                (
                    end,
                    Segment::Tag {
                        raw: &text[i..end],
                        name: &text[i + 1..end],
                    },
                )
            }),
            _ => None,
        };
        match found {
            Some((end, segment)) => {
                if plain_start < i {
                    out.push(Segment::Text(&text[plain_start..i]));
                }
                out.push(segment);
                i = end;
                plain_start = end;
            }
            None => i += c.len_utf8(),
        }
    }
    if plain_start < text.len() {
        out.push(Segment::Text(&text[plain_start..]));
    }
    out
}

fn graphemes(text: &str) -> usize {
    text.graphemes(true).count()
}

/// Length as Mastodon counts it: grapheme clusters, every URL is 23, and a
/// mention counts only its local part.
pub fn count(text: &str) -> usize {
    segments(text)
        .iter()
        .map(|segment| match segment {
            Segment::Url(_) => URL_CHARS,
            Segment::Mention { user, .. } => 1 + graphemes(user),
            Segment::Text(t) => graphemes(t),
            Segment::Tag { raw, .. } => graphemes(raw),
        })
        .sum()
}

/// Validate status text + content warning against the limits.
pub fn validate_status(text: &str, spoiler: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("Validation failed: Text can't be blank".into());
    }
    if text.len() > MAX_TEXT_BYTES || spoiler.len() > MAX_SPOILER_BYTES {
        return Err(format!(
            "Validation failed: Text character limit of {MAX_CHARS} exceeded"
        ));
    }
    if segments(text)
        .iter()
        .any(|s| matches!(s, Segment::Url(u) if u.len() > MAX_URL_BYTES))
    {
        return Err("Validation failed: Text contains a link that is too long".into());
    }
    if count(text) + graphemes(spoiler) > MAX_CHARS {
        return Err(format!(
            "Validation failed: Text character limit of {MAX_CHARS} exceeded"
        ));
    }
    Ok(())
}

/// Lowercased, de-duplicated hashtags in order of appearance (at most 8).
pub fn tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for segment in segments(text) {
        if let Segment::Tag { name, .. } = segment {
            let lower = name.to_lowercase();
            if !out.contains(&lower) && out.len() < MAX_TAGS {
                out.push(lower);
            }
        }
    }
    out
}

/// Lowercased usernames mentioned. Mentions with a domain are included only
/// when the domain is `local_host`.
pub fn mentions(text: &str, local_host: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for segment in segments(text) {
        if let Segment::Mention { user, domain, .. } = segment {
            if domain.is_some_and(|d| !d.eq_ignore_ascii_case(local_host)) {
                continue;
            }
            let lower = user.to_ascii_lowercase();
            if !out.contains(&lower) {
                out.push(lower);
            }
        }
    }
    out
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Plain text to HTML with newlines: blank lines become paragraphs, single
/// newlines become `<br />`.
fn push_text(out: &mut String, text: &str) {
    let mut newlines = 0;
    let flush = |out: &mut String, n: &mut usize| {
        match *n {
            0 => {}
            1 => out.push_str("<br />"),
            _ => out.push_str("</p><p>"),
        }
        *n = 0;
    };
    let mut run = String::new();
    for c in text.chars() {
        if c == '\n' {
            if !run.is_empty() {
                out.push_str(&escape(&run));
                run.clear();
            }
            newlines += 1;
        } else {
            flush(out, &mut newlines);
            run.push(c);
        }
    }
    if !run.is_empty() {
        out.push_str(&escape(&run));
    }
    flush(out, &mut newlines);
}

/// The target of a resolved mention.
pub struct MentionTarget {
    pub url: String,
    pub username: String,
}

/// Render status text as Mastodon HTML. `resolve` maps a local username to
/// its profile; unresolved mentions stay plain text.
pub fn render(
    text: &str,
    resolve: &dyn Fn(&str) -> Option<MentionTarget>,
    tag_base: &str,
) -> String {
    let text = text.replace("\r\n", "\n");
    let text = text.trim_matches('\n');
    let mut out = String::from("<p>");
    for segment in segments(text) {
        match segment {
            Segment::Text(t) => push_text(&mut out, t),
            Segment::Url(url) => {
                let href = escape(url);
                let scheme_len = if url.to_ascii_lowercase().starts_with("https://") {
                    8
                } else {
                    7
                };
                let (scheme, rest) = url.split_at(scheme_len);
                let rest = rest.strip_prefix("www.").map_or(rest, |r| r);
                let prefix = &url[scheme_len..url.len() - rest.len()];
                out.push_str(&format!(
                    "<a href=\"{href}\" target=\"_blank\" rel=\"nofollow noopener noreferrer\" translate=\"no\"><span class=\"invisible\">{}{}</span>",
                    escape(scheme),
                    escape(prefix)
                ));
                let visible: String = rest.chars().take(30).collect();
                if visible.len() < rest.len() {
                    out.push_str(&format!(
                        "<span class=\"ellipsis\">{}</span><span class=\"invisible\">{}</span></a>",
                        escape(&visible),
                        escape(&rest[visible.len()..])
                    ));
                } else {
                    out.push_str(&format!("<span class=\"\">{}</span></a>", escape(rest)));
                }
            }
            Segment::Mention { raw, user, .. } => match resolve(&user.to_ascii_lowercase()) {
                Some(target) => out.push_str(&format!(
                    "<span class=\"h-card\" translate=\"no\"><a href=\"{}\" class=\"u-url mention\">@<span>{}</span></a></span>",
                    escape(&target.url),
                    escape(&target.username)
                )),
                None => push_text(&mut out, raw),
            },
            Segment::Tag { name, .. } => out.push_str(&format!(
                "<a href=\"{}/{}\" class=\"mention hashtag\" rel=\"tag\">#<span>{}</span></a>",
                tag_base,
                escape(&name.to_lowercase()),
                escape(name)
            )),
        }
    }
    out.push_str("</p>");
    out
}

/// Plain text to a single escaped HTML paragraph (profile notes).
pub fn render_plain(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::from("<p>");
    push_text(&mut out, text.replace("\r\n", "\n").trim_matches('\n'));
    out.push_str("</p>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_table() {
        let cases: &[(&str, usize)] = &[
            ("hello", 5),
            (
                "https://example.com/a/very/long/path/that/goes/on/and/on",
                23,
            ),
            ("see https://x.io.", 4 + 23 + 1),
            ("@bob@elsewhere.example hi", 4 + 3),
            ("@bob hi", 7),
            ("👨‍👩‍👧‍👦", 1),
            ("e\u{301}", 1),
            ("#rust", 5),
            ("日本語", 3),
        ];
        for (text, expected) in cases {
            assert_eq!(count(text), *expected, "{text:?}");
        }
    }

    #[test]
    fn validation_applies_140_including_spoiler() {
        let text = "a".repeat(140);
        assert!(validate_status(&text, "").is_ok());
        assert!(validate_status(&text, "cw").is_err());
        assert!(validate_status(&"a".repeat(141), "").is_err());
        assert!(validate_status("   ", "").is_err());
        let links =
            "https://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ".repeat(5);
        assert!(
            validate_status(&links, "").is_ok(),
            "five long links count as 5 x 24"
        );
    }

    #[test]
    fn extracts_tags_and_mentions() {
        let text = "Hi @Alice and @bob@home.local and @carol@else.where #Dinner #dinner #123 #tea";
        assert_eq!(tags(text), vec!["dinner", "tea"]);
        assert_eq!(mentions(text, "home.local"), vec!["alice", "bob"]);
        assert!(tags("email a@b.c and x#y").is_empty());
        assert!(mentions("email a@b.c", "h").is_empty());
    }

    #[test]
    fn renders_mastodon_html() {
        let resolve = |u: &str| {
            (u == "alice").then(|| MentionTarget {
                url: "https://h/@alice".into(),
                username: "alice".into(),
            })
        };
        let html = render(
            "Hi @alice & @nobody <b>\nline\n\npara #Tea https://example.com/x",
            &resolve,
            "https://h/tags",
        );
        assert_eq!(
            html,
            "<p>Hi <span class=\"h-card\" translate=\"no\"><a href=\"https://h/@alice\" class=\"u-url mention\">@<span>alice</span></a></span> &amp; @nobody &lt;b&gt;<br />line</p><p>para <a href=\"https://h/tags/tea\" class=\"mention hashtag\" rel=\"tag\">#<span>Tea</span></a> <a href=\"https://example.com/x\" target=\"_blank\" rel=\"nofollow noopener noreferrer\" translate=\"no\"><span class=\"invisible\">https://</span><span class=\"\">example.com/x</span></a></p>"
        );
    }

    #[test]
    fn long_urls_get_an_ellipsis() {
        let html = render(
            "https://www.example.com/0123456789012345678901234567890123456789",
            &|_| None,
            "t",
        );
        assert!(html.contains("<span class=\"invisible\">https://www.</span>"));
        assert!(html.contains("<span class=\"ellipsis\">example.com/012345678901234567</span>"));
    }
}
