use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zc_forum_etl::{Post, posts_to_chunk, take_prefix_chars};

#[test]
fn take_prefix_chars_handles_multibyte() {
    let s = "é😊ño"; // 4 characters
    assert_eq!(take_prefix_chars(s, 2), "é😊");
}

#[test]
fn posts_to_chunk_counts_chars() {
    let ts = OffsetDateTime::UNIX_EPOCH;
    let posts = vec![
        Post {
            id: 1,
            cooked: "<p>é</p>".to_string(),
            created_at: ts,
        },
        Post {
            id: 2,
            cooked: "<p>😀</p>".to_string(),
            created_at: ts,
        },
    ];
    let ts_str = ts.format(&Rfc3339).unwrap();
    let expected = format!("[post:1 @ {ts}] é\n[post:2 @ {ts}] 😀", ts = ts_str);
    let max = expected.chars().count();
    let chunk = posts_to_chunk(posts.iter(), max);
    assert_eq!(chunk, expected);
    assert_eq!(chunk.chars().count(), max);
}
