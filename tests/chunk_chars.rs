use zc_forum_etl::{make_chunk, take_prefix_chars};

#[test]
fn take_prefix_chars_handles_multibyte() {
    let s = "é😊ño"; // 4 characters
    assert_eq!(take_prefix_chars(s, 2), "é😊");
}

#[test]
fn make_chunk_counts_chars() {
    let lines = vec!["é".to_string(), "😀".to_string()];
    let chunk = make_chunk(&lines, 3);
    assert_eq!(chunk, "é\n😀");
    assert_eq!(chunk.chars().count(), 3);
}
