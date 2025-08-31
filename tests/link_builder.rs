use zc_forum_etl::build_post_url;

#[test]
fn builds_post_url() {
    let url = build_post_url("https://forum.zcashcommunity.com", 1, 2);
    assert_eq!(url, "https://forum.zcashcommunity.com/t/1/2");
}
