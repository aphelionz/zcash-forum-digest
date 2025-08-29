use once_cell::sync::Lazy;
use regex::Regex;
use sha2::{Digest, Sha256};
use tiktoken_rs::{CoreBPE, cl100k_base};

static TAGS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<[^>]*>").unwrap());
pub static BPE: Lazy<CoreBPE> = Lazy::new(|| cl100k_base().expect("tokenizer"));

pub fn strip_tags_fast(html: &str) -> String {
    let no_tags = TAGS_RE.replace_all(html, " ");
    squeeze_ws(no_tags.trim())
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

pub fn take_prefix_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = 0usize;
    for (idx, _) in s.char_indices() {
        if idx <= max {
            cut = idx;
        } else {
            break;
        }
    }
    s[..cut].to_string()
}

pub fn make_chunk(lines: &[String], max_chars: usize) -> String {
    let mut cur = String::new();
    for l in lines {
        if cur.len() + l.len() + 1 > max_chars {
            let remain = max_chars.saturating_sub(cur.len());
            if remain > 0 {
                cur.push_str(&take_prefix_chars(l, remain));
            }
            break;
        }
        if !l.is_empty() {
            cur.push_str(l);
            cur.push('\n');
        }
    }
    cur
}

pub fn prompt_hash(topic_id: i64, model: &str, prompt: &str) -> String {
    let mut h = Sha256::new();
    h.update(model.as_bytes());
    h.update(b"\n");
    h.update(topic_id.to_be_bytes());
    h.update(b"\n");
    h.update(prompt.as_bytes());
    format!("{:x}", h.finalize())
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
    fn take_prefix_chars_handles_utf8_boundaries() {
        assert_eq!(take_prefix_chars("a🐱b", 5), "a🐱");
        assert_eq!(take_prefix_chars("a🐱b", 3), "a");
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
