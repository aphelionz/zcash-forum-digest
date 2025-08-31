use zc_forum_etl::strip_post_tags;

#[test]
fn removes_post_annotations() {
    let input =
        "Headline\n- first item [post:123] more\n- second item [post:456 @ 2024-01-01T00:00:00Z]\n";
    let expected = "Headline\n- first item more\n- second item";
    let cleaned = strip_post_tags(input);
    assert_eq!(cleaned, expected);
    assert!(!cleaned.contains("[post:"));
}
