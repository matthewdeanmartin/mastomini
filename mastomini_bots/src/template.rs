//! `{{name}}` templates for bot messages and LLM prompts.
//!
//! Variables are plain text. A conversation is one line per post
//! (`@alice: text`), not JSON: the lightest format a person can read and a
//! model can follow, and the fewest tokens. A variable with nothing in it
//! renders as nothing, and a template line made only of such variables is
//! left out, so an empty `{{bio}}` doesn't leave a hole.

/// The `{{names}}` a template uses, in order, without repeats.
pub fn names(template: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let name = after[..end].trim().to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
        rest = &after[end + 2..];
    }
    out
}

/// One template line: the text and whether it used any variable.
fn render_line(line: &str, vars: &[(&str, &str)]) -> (String, bool) {
    let mut out = String::with_capacity(line.len());
    let mut used = false;
    let mut rest = line;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return (out, used);
        };
        used = true;
        let name = after[..end].trim();
        if let Some((_, value)) = vars.iter().find(|(k, _)| *k == name) {
            out.push_str(value);
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    (out, used)
}

/// Fill in `{{name}}`s from `vars`; unknown names render as nothing.
/// Values are inserted as they are, never read again as templates.
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let lines: Vec<String> = template
        .lines()
        .filter_map(|line| {
            let (text, used) = render_line(line, vars);
            // A line that was only variables, all empty, goes.
            (!(used && text.trim().is_empty())).then_some(text)
        })
        .collect();
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_lists() {
        let t = "Hello {{ name }}, it is {{date}}.\n{{extra}}\n\nBye {{name}}";
        assert_eq!(names(t), vec!["name", "date", "extra"]);
        let out = render(t, &[("name", "Ann"), ("date", "Monday"), ("extra", "")]);
        assert_eq!(out, "Hello Ann, it is Monday.\n\nBye Ann");
        assert_eq!(render("{{x}} and {{", &[("x", "a")]), "a and {{");
        assert_eq!(render("no vars", &[]), "no vars");
        // A value is never re-read as a template.
        assert_eq!(render("{{a}}", &[("a", "{{b}}"), ("b", "no")]), "{{b}}");
    }

    #[test]
    fn multi_line_values_keep_the_template_layout() {
        let t = "Thread:\n{{conversation}}\n\nNow reply.";
        let out = render(t, &[("conversation", "@a: one\n\n@b: two")]);
        assert_eq!(out, "Thread:\n@a: one\n\n@b: two\n\nNow reply.");
    }
}
