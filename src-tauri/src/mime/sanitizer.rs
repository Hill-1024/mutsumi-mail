#![allow(dead_code)] // Renderer is kept separate so the IPC layer never receives raw HTML.

/// Conservative text extraction for the first reader slice. Raw HTML never crosses the IPC boundary.
pub fn html_to_safe_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut cursor = 0;
    let mut blocked_element: Option<String> = None;
    while cursor < html.len() {
        let rest = &html[cursor..];
        if let Some(relative_end) = rest.find('>') {
            if rest.starts_with('<') {
                let raw_tag = rest[1..relative_end]
                    .trim()
                    .trim_start_matches('/')
                    .trim_end_matches('/');
                let tag_name = raw_tag
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let is_closing = rest.as_bytes().get(1).copied() == Some(b'/');
                if is_closing {
                    if blocked_element.as_deref() == Some(tag_name.as_str()) {
                        blocked_element = None;
                    }
                } else if matches!(
                    tag_name.as_str(),
                    "script" | "style" | "iframe" | "object" | "embed" | "form"
                ) {
                    blocked_element = Some(tag_name);
                }
                cursor += relative_end + 1;
                continue;
            }
        }
        if blocked_element.is_none() {
            if let Some(character) = rest.chars().next() {
                text.push(character);
                cursor += character.len_utf8();
            } else {
                break;
            }
        } else {
            cursor += rest.chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::html_to_safe_text;

    #[test]
    fn strips_script_markup_and_content() {
        assert_eq!(
            html_to_safe_text("<p>Hello</p><script>alert(1)</script>"),
            "Hello"
        );
    }
}
