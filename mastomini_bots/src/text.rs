//! Plain text in and out of bots: Mastodon's HTML to text for prompts, and a
//! model's reply to something fit to post.
//!
//! Model output is untrusted text. It is posted as plain text (the server
//! escapes it), trimmed to the server's length limit, and may not mention
//! anyone the bot did not mean to: an invented `@name` is defused so nobody
//! gets notified by a model's whim.

/// Mastodon status HTML to plain text: paragraphs and line breaks kept,
/// tags dropped, entities decoded.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end].trim().to_ascii_lowercase();
        if tag.starts_with("br") {
            out.push('\n');
        } else if tag == "/p" {
            out.push_str("\n\n");
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    decode_entities(&out).trim().to_string()
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|&i| i <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
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

/// What a model says around its answer, dropped: reasoning blocks, a
/// "Sure, here's..." opener, code fences, quotes around the whole reply.
pub fn clean_completion(text: &str) -> Option<String> {
    let mut text = text.trim().to_string();
    // Reasoning some models leave in their answer.
    while let (Some(start), Some(end)) = (text.find("<think>"), text.find("</think>")) {
        if end < start {
            break;
        }
        text.replace_range(start..end + "</think>".len(), "");
    }
    let mut text = text.trim().to_string();
    if let Some((first, rest)) = text.split_once('\n') {
        let opener = first.trim().to_ascii_lowercase();
        let chatty = [
            "sure",
            "certainly",
            "of course",
            "here's",
            "here is",
            "okay",
            "ok",
        ]
        .iter()
        .any(|w| opener.starts_with(w));
        if chatty && opener.ends_with(':') && !rest.trim().is_empty() {
            text = rest.trim().to_string();
        }
    }
    if let Some(inner) = text
        .strip_prefix("```")
        .and_then(|t| t.strip_suffix("```"))
        .map(|t| {
            t.split_once('\n')
                .map_or(t, |(lang, body)| if lang.contains(' ') { t } else { body })
        })
    {
        text = inner.trim().to_string();
    }
    for (open, close) in [('"', '"'), ('“', '”'), ('\'', '\'')] {
        if text.len() > 2 && text.starts_with(open) && text.ends_with(close) {
            let inner = &text[open.len_utf8()..text.len() - close.len_utf8()];
            if !inner.contains(['"', '“', '”']) {
                text = inner.trim().to_string();
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

/// Defuse `@name` mentions not in `allowed` (lowercase `name` or
/// `name@host`), so a model cannot notify people on its own.
pub fn defuse_mentions(text: &str, allowed: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let starts_word = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
        if c == '@' && starts_word {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || "_.@-".contains(chars[j])) {
                j += 1;
            }
            let name: String = chars[i + 1..j]
                .iter()
                .collect::<String>()
                .trim_end_matches(['.', '-'])
                .to_lowercase();
            if !name.is_empty()
                && !allowed
                    .iter()
                    .any(|a| *a == name || a.split('@').next() == Some(&name))
            {
                // A zero-width space keeps it readable and inert.
                out.push_str("@\u{200B}");
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// At most `limit` characters, cut at a word boundary with an ellipsis.
pub fn fit(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    let cut = match cut.rfind(char::is_whitespace) {
        Some(i) if i > limit / 2 => cut[..i].to_string(),
        _ => cut,
    };
    format!(
        "{}…",
        cut.trim_end_matches(|c: char| c.is_whitespace() || ",;:".contains(c))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_text() {
        let html = "<p>Hi <span class=\"h-card\"><a href=\"x\">@<span>bot</span></a></span> &amp; all</p><p>Line<br>two &#x1F600; &lt;b&gt;</p>";
        assert_eq!(html_to_text(html), "Hi @bot & all\n\nLine\ntwo 😀 <b>");
        assert_eq!(html_to_text("5 & 6 &bogus; &"), "5 & 6 &bogus; &");
    }

    #[test]
    fn completions_are_cleaned() {
        assert_eq!(
            clean_completion("<think>hmm</think>\nHello!").unwrap(),
            "Hello!"
        );
        assert_eq!(
            clean_completion("Sure, here's a post:\nGood morning").unwrap(),
            "Good morning"
        );
        assert_eq!(
            clean_completion("\"Quoted whole\"").unwrap(),
            "Quoted whole"
        );
        assert_eq!(
            clean_completion("She said \"hi\" twice").unwrap(),
            "She said \"hi\" twice"
        );
        assert_eq!(
            clean_completion("```\ncode fenced\n```").unwrap(),
            "code fenced"
        );
        assert_eq!(clean_completion("  \n "), None);
    }

    #[test]
    fn only_allowed_mentions_notify() {
        let allowed = vec!["alice".to_string(), "bob@mastomini.local".to_string()];
        let out = defuse_mentions("@alice ask @bob and @mallory, mail a@b.c", &allowed);
        assert_eq!(out, "@alice ask @bob and @\u{200B}mallory, mail a@b.c");
    }

    #[test]
    fn fits_the_limit() {
        assert_eq!(fit("short", 140), "short");
        let long = "word ".repeat(40);
        let out = fit(&long, 30);
        assert!(out.chars().count() <= 30, "{out}");
        assert!(out.ends_with("word…"), "{out}");
    }
}
