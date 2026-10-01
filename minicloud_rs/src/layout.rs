//! Fixed-font, dictionary-free wrapping. Only overlong tokens are split.
use crate::TextSize;
pub fn scale(size: &TextSize) -> usize {
    match size {
        TextSize::Small => 1,
        TextSize::Medium => 2,
        TextSize::Large => 3,
    }
}
pub fn pages(text: &str, size: &TextSize) -> Vec<String> {
    let scale = scale(size);
    let columns = 318 / (8 * scale);
    let rows = 128 / (16 * scale);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for token in paragraph.split_whitespace() {
            let mut word: String = token
                .chars()
                .map(|c| {
                    if c.is_ascii() && !c.is_control() {
                        c
                    } else {
                        '?'
                    }
                })
                .collect();
            if !line.is_empty() && line.len() + 1 + word.len() > columns {
                lines.push(line);
                line = String::new();
            }
            while word.len() > columns {
                let rest = word.split_off(columns - 1);
                word.push('-');
                lines.push(word);
                word = rest;
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
        .chunks(rows)
        .map(|chunk| {
            chunk
                .iter()
                .map(|line| format!("{line:columns$}"))
                .collect()
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_words_and_paragraphs() {
        let p = pages(
            "New listing: take me to the playground for 35 NC",
            &TextSize::Medium,
        );
        assert_eq!(&p[0][..19], "New listing: take  ");
        assert!(p.concat().contains("playground"));
    }
    #[test]
    fn long_tokens_and_all_sizes_preserve_content() {
        for size in [TextSize::Small, TextSize::Medium, TextSize::Large] {
            let text = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            let p = pages(text, &size);
            assert_eq!(p.concat().replace([' ', '-'], ""), text);
            assert!(p.iter().all(|s| s.len() <= 512));
        }
    }
}
