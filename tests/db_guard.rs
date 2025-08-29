use anyhow::Result;
use sqlx::{PgPool, query};
use time::{Duration, OffsetDateTime};
use zc_forum_etl::posts_changed_since_last_llm;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[sqlx::test(migrator = "MIGRATOR")]
async fn db_guard(pool: PgPool) -> Result<()> {
    let topic_id = 1_i64;
    let post_time = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

    query("INSERT INTO topics (id, title) VALUES ($1, $2)")
        .bind(topic_id)
        .bind("test")
        .execute(&pool)
        .await?;

    query("INSERT INTO posts (id, topic_id, username, cooked, created_at) VALUES ($1,$2,$3,$4,$5)")
        .bind(1_i64)
        .bind(topic_id)
        .bind("user")
        .bind("<p>hi</p>")
        .bind(post_time)
        .execute(&pool)
        .await?;

    assert!(posts_changed_since_last_llm(&pool, topic_id).await?);

    let early = post_time - Duration::seconds(60);
    query("INSERT INTO topic_summaries_llm (topic_id, summary, model, prompt_hash, input_tokens, output_tokens, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(topic_id)
        .bind("{}")
        .bind("m")
        .bind("h")
        .bind(0_i64)
        .bind(0_i64)
        .bind(early)
        .execute(&pool)
        .await?;

    assert!(posts_changed_since_last_llm(&pool, topic_id).await?);

    let late = post_time + Duration::seconds(60);
    query("UPDATE topic_summaries_llm SET updated_at = $1 WHERE topic_id = $2")
        .bind(late)
        .bind(topic_id)
        .execute(&pool)
        .await?;

    assert!(!posts_changed_since_last_llm(&pool, topic_id).await?);

    Ok(())
}
