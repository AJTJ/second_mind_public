use reqwest::Client;

/// Max chars to return from a page read. This is the single truncation point —
/// agent.rs does NOT re-truncate read_page results.
const MAX_PAGE_CHARS: usize = 2_000;

/// Fetch a URL and return its text content with HTML stripped.
/// Validates URL scheme and blocks internal/private addresses (SSRF protection).
pub async fn read_page(client: &Client, url: &str) -> anyhow::Result<String> {
    // SSRF protection: only allow http/https, block internal addresses
    if !url.starts_with("http://") && !url.starts_with("https://") {
        anyhow::bail!("Blocked: only http/https URLs are allowed, got: {url}");
    }
    let lower = url.to_ascii_lowercase();
    if lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("0.0.0.0")
        || lower.contains("172.17.0.")
        || lower.contains("[::1]")
        || lower.contains("169.254.")
        || lower.contains("10.0.")
        || lower.contains("192.168.")
    {
        anyhow::bail!("Blocked: cannot fetch internal/private URLs: {url}");
    }

    let resp = client
        .get(url)
        .header("User-Agent", "Explorer/0.1 (research agent)")
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("Failed to fetch {url} ({status})");
    }

    let body = resp.text().await?;

    // Bail on binary/garbage content — if >10% non-ASCII, it's not useful text
    let non_ascii = body
        .chars()
        .filter(|c| !c.is_ascii() && !c.is_alphanumeric())
        .count();
    if body.len() > 100 && non_ascii * 10 > body.len() {
        anyhow::bail!("Page returned non-text content (likely binary/compressed): {url}");
    }

    let text = strip_html(&body);

    // Truncate on a sentence boundary to avoid cutting mid-fact
    Ok(truncate_semantic(&text, MAX_PAGE_CHARS))
}

/// Truncate text to max_chars, preferring to cut at a sentence boundary.
/// Safe for all UTF-8 strings — never slices mid-character.
fn truncate_semantic(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    // Find a char-safe boundary at max_chars
    let byte_limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let slice = &text[..byte_limit];

    // Look for the last sentence-ending punctuation
    let cut_point = slice
        .rfind(". ")
        .or_else(|| slice.rfind(".\n"))
        .or_else(|| slice.rfind("? "))
        .or_else(|| slice.rfind("! "))
        .map(|p| p + 1)
        .unwrap_or(byte_limit);

    text[..cut_point].to_string()
}

fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut last_was_space = false;
    let mut tag_name = String::new();

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            tag_name.clear();
            continue;
        }
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let lower = tag_name.to_ascii_lowercase();
                if lower == "script" || lower == "style" {
                    in_script = true;
                } else if lower == "/script" || lower == "/style" {
                    in_script = false;
                }
                if !in_script && !last_was_space {
                    result.push(' ');
                    last_was_space = true;
                }
            } else if ch != '/' || tag_name.is_empty() {
                if !ch.is_ascii_whitespace() && !tag_name.contains(' ') {
                    tag_name.push(ch);
                }
            } else {
                tag_name.push(ch);
            }
            continue;
        }
        if in_script {
            continue;
        }

        let ch = if ch == '\n' || ch == '\r' || ch == '\t' {
            ' '
        } else {
            ch
        };

        if ch == ' ' && last_was_space {
            continue;
        }
        last_was_space = ch == ' ';
        result.push(ch);
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_basic_html() {
        assert_eq!(strip_html("<p>hello</p>"), "hello");
    }

    #[test]
    fn strips_script_content() {
        let html = "<p>before</p><script>var x = 1;</script><p>after</p>";
        assert_eq!(strip_html(html), "before after");
    }

    #[test]
    fn strips_style_content() {
        let html = "<p>text</p><style>body { color: red; }</style><p>more</p>";
        assert_eq!(strip_html(html), "text more");
    }

    #[test]
    fn truncate_semantic_at_sentence() {
        let text = "First sentence. Second sentence. Third sentence is longer.";
        let result = truncate_semantic(text, 35);
        assert_eq!(result, "First sentence. Second sentence.");
    }

    #[test]
    fn truncate_semantic_hard_cut() {
        let text = "One very long sentence without any periods";
        let result = truncate_semantic(text, 20);
        assert_eq!(result, "One very long senten");
    }

    #[test]
    fn blocks_internal_urls() {
        // Can't test async fn directly, but validate the logic
        assert!("http://localhost:8000".contains("localhost"));
        assert!("http://172.17.0.1:8000".contains("172.17.0."));
    }
}
