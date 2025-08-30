use anyhow::Result;
use futures::{StreamExt, stream};
use reqwest::Client;
use serde::Deserialize;
use sqlx::{PgPool, query};
use time::OffsetDateTime;
use tokio::time::{Duration, timeout};
use tracing::{info, instrument, warn};
use zc_forum_etl::{
    load_plain_lines, make_chunk, posts_changed_since_last_llm, prompt_hash, summarize_with_ollama,
};

const CHUNK_MAX_CHARS: usize = 1_800; // keep prompt small for local models
const SUM_TIMEOUT_SECS: u64 = 240; // wrap around our own retry/HTTP timeouts
const TOPIC_CONCURRENCY: usize = 5; // limit concurrent topic processing

const OLLAMA_DEFAULT_BASE: &str = "http://127.0.0.1:11434";

#[derive(Debug, Deserialize)]
struct Latest {
    topic_list: TopicList,
}
#[derive(Debug, Deserialize)]
struct TopicList {
    topics: Vec<TopicStub>,
}
#[derive(Debug, Deserialize)]
struct TopicStub {
    id: u64,
    title: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TopicFull {
    id: u64,
    title: String,
    post_stream: PostStream,
}
#[derive(Debug, Deserialize)]
struct PostStream {
    posts: Vec<Post>,
}

#[derive(Debug, Deserialize)]
struct Post {
    id: u64,
    topic_id: u64,
    username: String,
    cooked: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let pool = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;

    // HTTP client: long overall timeout; local servers sometimes take a bit on first load.
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()?;

    // Ollama model + base URL
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "qwen2.5:latest".to_string());
    let ollama_base =
        std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| OLLAMA_DEFAULT_BASE.to_string());

    // Warm up the model once and warn if it fails.
    let warm_prompt = build_prompt("warmup", "warmup");
    let warmup_res = summarize_with_ollama(&client, &ollama_base, &model, &warm_prompt).await;
    if let Err(e) = warmup_res {
        warn!("Warm-up summarize_with_ollama failed: {e}");
    }

    // Fetch latest list of topics
    let latest: Latest = fetch_latest(&client).await?;
    info!("Fetched {} topics", latest.topic_list.topics.len());

    let topics = latest.topic_list.topics;
    stream::iter(topics.into_iter())
        .map(|topic| {
            let client = client.clone();
            let pool = pool.clone();
            let model = model.clone();
            let ollama_base = ollama_base.clone();
            async move {
                if let Err(e) = process_topic(client, pool, model, ollama_base, topic).await {
                    warn!("Topic processing failed: {e:?}");
                }
            }
        })
        .buffer_unordered(TOPIC_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

    info!("ETL + local LLM summaries complete.");
    Ok(())
}

async fn process_topic(
    client: Client,
    pool: PgPool,
    model: String,
    ollama_base: String,
    topic: TopicStub,
) -> Result<()> {
    // Upsert topic metadata
    query!(
        r#"INSERT INTO topics (id, title) VALUES ($1, $2)
           ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title"#,
        topic.id as i64,
        topic.title
    )
    .execute(&pool)
    .await?;

    // Fetch first page of posts for this topic
    let full = fetch_topic(&client, topic.id).await?;
    info!("Topic {} → {} posts", full.id, full.post_stream.posts.len());

    // Upsert posts
    for p in full.post_stream.posts {
        query!(
            r#"
            INSERT INTO posts (id, topic_id, username, cooked, created_at)
            VALUES ($1,$2,$3,$4,$5)
            ON CONFLICT (id) DO UPDATE SET
              topic_id = EXCLUDED.topic_id,
              username = EXCLUDED.username,
              cooked = EXCLUDED.cooked,
              created_at = EXCLUDED.created_at
            "#,
            p.id as i64,
            p.topic_id as i64,
            p.username,
            p.cooked,
            p.created_at
        )
        .execute(&pool)
        .await?;
    }

    // Skip if no new posts since last LLM summary
    if !posts_changed_since_last_llm(&pool, topic.id as i64).await? {
        info!("Topic {} unchanged since last LLM summary → skip", topic.id);
        return Ok(());
    }

    // Build compact chunk from first-page posts
    let lines = load_plain_lines(&pool, topic.id as i64).await?;
    if lines.is_empty() {
        return Ok(());
    }
    let chunk = make_chunk(&lines, CHUNK_MAX_CHARS);
    if chunk.is_empty() {
        return Ok(());
    }

    let prompt = build_prompt(&topic.title, &chunk);
    let phash = prompt_hash(topic.id as i64, &model, &prompt);

    // LLM call with outer timeout guard
    let started = std::time::Instant::now();
    match timeout(
        Duration::from_secs(SUM_TIMEOUT_SECS),
        summarize_with_ollama(&client, &ollama_base, &model, &prompt),
    )
    .await
    {
        Err(_) => {
            warn!("LLM summarize timed out for {}", topic.id);
        }
        Ok(Err(e)) => {
            warn!("LLM summarize failed for {}: {e}", topic.id);
        }
        Ok(Ok((summary, in_tok, out_tok))) => {
            let summary_json = serde_json::to_string(&summary)?;
            query!(
                r#"
                INSERT INTO topic_summaries_llm (topic_id, summary, model, prompt_hash, input_tokens, output_tokens, cost_usd)
                VALUES ($1, $2, $3, $4, $5, $6, NULL)
                ON CONFLICT (topic_id) DO UPDATE SET
                  summary = EXCLUDED.summary,
                  model = EXCLUDED.model,
                  prompt_hash = EXCLUDED.prompt_hash,
                  input_tokens = EXCLUDED.input_tokens,
                  output_tokens = EXCLUDED.output_tokens,
                  updated_at = now()
                "#,
                topic.id as i64, summary_json, model, phash, in_tok as i32, out_tok as i32
            ).execute(&pool).await?;

            info!(
                "LLM summarized topic {} in {:?}",
                topic.id,
                started.elapsed()
            );
        }
    }

    Ok(())
}

/* ---------------- Discourse HTTP ---------------- */

#[instrument(skip(client))]
async fn fetch_latest(client: &Client) -> Result<Latest> {
    Ok(client
        .get("https://forum.zcashcommunity.com/latest.json")
        .send()
        .await?
        .error_for_status()?
        .json::<Latest>()
        .await?)
}

#[instrument(skip(client))]
async fn fetch_topic(client: &Client, id: u64) -> Result<TopicFull> {
    let url = format!("https://forum.zcashcommunity.com/t/{}.json", id);
    Ok(client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<TopicFull>()
        .await?)
}

/* Formatting instructions are in Modelfile */
fn build_prompt(topic_title: &str, chunk: &str) -> String {
    format!(
        "Thread: {title}\n\nContent excerpt:\n---\n{body}\n---",
        title = topic_title,
        body = chunk
    )
}
