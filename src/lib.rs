use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde::Deserialize;
use std::sync::LazyLock;
use tiktoken_rs::{CoreBPE, cl100k_base};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub mod ollama;
pub use ollama::summarize_with_ollama;

pub static BPE: LazyLock<CoreBPE> =
    LazyLock::new(|| cl100k_base().expect("Failed to initialize cl100k_base tokenizer"));

// Sorted list of HTML block-level tags that should be separated by whitespace
// when converting to plain text.
const BLOCK_TAGS: [&str; 39] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "canvas",
    "dd",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "noscript",
    "ol",
    "output",
    "p",
    "pre",
    "section",
    "table",
    "td",
    "tfoot",
    "th",
    "tr",
    "ul",
    "video",
];

/// Strip HTML tags, decode entities, and drop script/style blocks.
pub fn strip_tags_fast(html: &str) -> String {
    // Fast path: skip DOM parse if there are no tags or entities.
    if !html.as_bytes().iter().any(|b| *b == b'<' || *b == b'&') {
        return squeeze_ws(html.trim());
    }
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html);

    fn walk(handle: &Handle, out: &mut String) {
        match &handle.data {
            NodeData::Text { contents } => {
                out.push_str(&contents.borrow());
                // Do not unconditionally add a space here.
            }
            NodeData::Element { name, .. } => {
                let local = name.local.as_ref();
                if local.eq_ignore_ascii_case("script") || local.eq_ignore_ascii_case("style") {
                    return;
                }
                // After processing children, add a space if this is a block-level element.
                let is_block = BLOCK_TAGS.binary_search(&local).is_ok();
                for child in handle.children.borrow().iter() {
                    walk(child, out);
                }
                if is_block {
                    out.push(' ');
                }
                return;
            }
            _ => {}
        }
        // For non-element nodes, walk children as before.
        for child in handle.children.borrow().iter() {
            walk(child, out);
        }
    }

    let mut text = String::new();
    for child in dom.document.children.borrow().iter() {
        walk(child, &mut text);
    }
    squeeze_ws(text.trim())
}

pub fn squeeze_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

pub fn take_prefix_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

#[derive(Deserialize, Clone)]
pub struct Post {
    pub id: u64,
    pub cooked: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub username: String,
}

pub fn posts_to_chunk<'a>(posts: impl Iterator<Item = &'a Post>, max_chars: usize) -> String {
    let mut out = String::new();
    let mut cur_chars = 0usize;
    for p in posts {
        let t = strip_tags_fast(&p.cooked);
        if t.is_empty() {
            continue;
        }
        if let Ok(ts) = p.created_at.format(&Rfc3339) {
            let line = format!("[post:{} @ {}] {}", p.id, ts, t);
            let l_chars = line.chars().count();
            if cur_chars + l_chars + 1 > max_chars {
                let remain = max_chars.saturating_sub(cur_chars);
                if remain > 0 {
                    out.push_str(&take_prefix_chars(&line, remain));
                }
                break;
            }
            out.push_str(&line);
            out.push('\n');
            cur_chars += l_chars + 1;
        }
    }
    out
}

#[derive(Clone)]
pub struct DigestItem {
    pub post_id: u64,
    pub topic_id: u64,
    pub created_at: OffsetDateTime,
    pub author: String,
    pub title: String,
    pub url: String,
    pub summary: String,
}

pub fn build_post_url(base: &str, topic_id: u64, post_id: u64) -> String {
    format!("{}/t/{}/{}", base.trim_end_matches('/'), topic_id, post_id)
}

pub fn compose_digest_item(
    base: &str,
    topic_id: u64,
    title: &str,
    post: &Post,
    summary: String,
) -> DigestItem {
    DigestItem {
        post_id: post.id,
        topic_id,
        created_at: post.created_at,
        author: post.username.clone(),
        title: title.to_string(),
        url: build_post_url(base, topic_id, post.id),
        summary,
    }
}

/// Remove any `[post:ID]` annotations the model might echo from the prompt.
///
/// The model is instructed not to emit these tags, but this function provides a
/// final safeguard by stripping them from the summary while preserving
/// newlines.
pub fn strip_post_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            let mut temp = chars.clone();
            if temp.next() == Some('p')
                && temp.next() == Some('o')
                && temp.next() == Some('s')
                && temp.next() == Some('t')
                && temp.next() == Some(':')
            {
                // Skip until closing bracket
                for c2 in chars.by_ref() {
                    if c2 == ']' {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(ch);
    }
    out.lines()
        .map(|l| squeeze_ws(l).trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squeeze_ws_collapses_whitespace() {
        let s = "Hello   world \n\n test";
        assert_eq!(squeeze_ws(s), "Hello world test");
    }

    #[test]
    fn strip_tags_fast_removes_html_and_normalizes_space() {
        let html = "<p>Hello <b>world</b></p>\n<div>Rust lang</div>";
        assert_eq!(strip_tags_fast(html), "Hello world Rust lang");
    }

    #[test]
    fn strip_tags_fast_decodes_entities_and_drops_script_style() {
        let html =
            "<p>Tom &amp; Jerry</p><script>var x = 1;</script><style>body{color:red}</style>";
        assert_eq!(strip_tags_fast(html), "Tom & Jerry");
    }

    #[test]
    fn take_prefix_chars_handles_utf8_boundaries() {
        assert_eq!(take_prefix_chars("a🐱b", 2), "a🐱");
        assert_eq!(take_prefix_chars("a🐱b", 1), "a");
    }

    #[test]
    fn bpe_static_encodes() {
        let tokens = BPE.encode_with_special_tokens("hello");
        assert!(!tokens.is_empty());
    }
}
