use std::fs;
use zc_forum_etl::{compose_digest_item, Post};

#[test]
fn merges_metadata_with_summary() {
    let data = fs::read_to_string("tests/fixtures/post.json").unwrap();
    let post: Post = serde_json::from_str(&data).unwrap();
    let topic_id = 42;
    let title = "Example Topic";
    let summary = "Real summary".to_string();
    let item = compose_digest_item(
        "https://forum.zcashcommunity.com",
        topic_id,
        title,
        &post,
        summary.clone(),
    );
    assert_eq!(item.post_id, post.id);
    assert_eq!(item.created_at, post.created_at);
    assert_eq!(item.author, post.username);
    assert_eq!(item.title, title);
    assert_eq!(item.topic_id, topic_id);
    assert_eq!(item.url, format!("https://forum.zcashcommunity.com/t/{}/{}", topic_id, post.id));
    assert_eq!(item.summary, summary);
    assert!(!item.summary.is_empty());

    let fake = "fake 9999".to_string();
    let item2 = compose_digest_item(
        "https://forum.zcashcommunity.com",
        topic_id,
        title,
        &post,
        fake.clone(),
    );
    assert_eq!(item2.post_id, post.id);
    assert_eq!(item2.created_at, post.created_at);
    assert_eq!(item2.summary, fake);
}
