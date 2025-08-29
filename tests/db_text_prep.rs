use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use sqlx::{query, PgPool};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use zc_forum_etl::{load_plain_lines, make_chunk};

static LINE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[post:\d+ @ [^\]]+\] .+").unwrap());

#[sqlx::test(migrations = "./migrations")]
async fn load_plain_lines_orders_and_strips(pool: PgPool) -> Result<()> {
    // seed topic
    query("INSERT INTO topics (id, title) VALUES ($1, $2)")
        .bind(1_i64)
        .bind("Test topic")
        .execute(&pool)
        .await?;

    // two posts with HTML and differing timestamps
    let t_old = OffsetDateTime::from_unix_timestamp(1_000_000).unwrap();
    let t_new = OffsetDateTime::from_unix_timestamp(1_000_100).unwrap();
    query("INSERT INTO posts (id, topic_id, username, cooked, created_at) VALUES ($1,$2,$3,$4,$5)")
        .bind(10_i64)
        .bind(1_i64)
        .bind("alice")
        .bind("<p>First <b>post</b></p>")
        .bind(t_old)
        .execute(&pool)
        .await?;
    query("INSERT INTO posts (id, topic_id, username, cooked, created_at) VALUES ($1,$2,$3,$4,$5)")
        .bind(11_i64)
        .bind(1_i64)
        .bind("bob")
        .bind("<div>Second <i>post</i></div>")
        .bind(t_new)
        .execute(&pool)
        .await?;

    let lines = load_plain_lines(&pool, 1).await?;

    // ensure ordering by created_at
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("First post"));
    assert!(lines[1].contains("Second post"));

    // HTML tags should be gone
    for l in &lines {
        assert!(!l.contains('<') && !l.contains('>'));
    }

    // each line should match pattern
    for l in &lines {
        assert!(LINE_RE.is_match(l), "line did not match pattern: {l}");
    }

    // optional: ensure chunking respects length limit
    let chunk = make_chunk(&lines, 80);
    assert!(chunk.len() <= 80);

    // ensure actual timestamps in formatted string
    let ts_old = t_old.format(&Rfc3339)?;
    let ts_new = t_new.format(&Rfc3339)?;
    assert_eq!(lines[0], format!("[post:10 @ {ts_old}] First post"));
    assert_eq!(lines[1], format!("[post:11 @ {ts_new}] Second post"));

    Ok(())
}
