use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use std::sync::LazyLock;
use tiktoken_rs::{CoreBPE, cl100k_base};

pub mod ollama;
pub use ollama::{Summary, summarize_with_ollama};

pub static BPE: LazyLock<CoreBPE> =
    LazyLock::new(|| cl100k_base().expect("Failed to initialize cl100k_base tokenizer"));

/// Strip HTML tags, decode entities, and drop script/style blocks.
pub fn strip_tags_fast(html: &str) -> String {
    // Fast path: skip DOM parse if there are no tags or entities.
    if !html.contains('<') && !html.contains('&') {
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
                let is_block = matches!(
                    local,
                    "address"
                        | "article"
                        | "aside"
                        | "blockquote"
                        | "canvas"
                        | "dd"
                        | "div"
                        | "dl"
                        | "dt"
                        | "fieldset"
                        | "figcaption"
                        | "figure"
                        | "footer"
                        | "form"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "header"
                        | "hr"
                        | "li"
                        | "main"
                        | "nav"
                        | "noscript"
                        | "ol"
                        | "output"
                        | "p"
                        | "pre"
                        | "section"
                        | "table"
                        | "tfoot"
                        | "ul"
                        | "video"
                        | "tr"
                        | "td"
                        | "th"
                        | "br"
                );
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

pub fn make_chunk(lines: &[String], max_chars: usize) -> String {
    let mut cur = String::new();
    let mut cur_chars = 0usize;
    for l in lines {
        let l_chars = l.chars().count();
        if cur_chars + l_chars + 1 > max_chars {
            let remain = max_chars.saturating_sub(cur_chars);
            if remain > 0 {
                cur.push_str(&take_prefix_chars(l, remain));
            }
            break;
        }
        if !l.is_empty() {
            cur.push_str(l);
            cur.push('\n');
            cur_chars += l_chars + 1; // account for newline
        }
    }
    cur
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
    fn make_chunk_truncates_without_splitting_chars() {
        let lines = vec![
            "12345".to_string(),
            "67890".to_string(),
            "abcde".to_string(),
        ];
        let chunk = make_chunk(&lines, 11);
        assert_eq!(chunk, "12345\n67890");
    }

    #[test]
    fn bpe_static_encodes() {
        let tokens = BPE.encode_with_special_tokens("hello");
        assert!(!tokens.is_empty());
    }
}
