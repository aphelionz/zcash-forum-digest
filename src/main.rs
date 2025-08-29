use anyhow::{Result, anyhow};
use backoff::{ExponentialBackoff, future::retry};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, query, query_scalar};
use tiktoken_rs::cl100k_base;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::time::{Duration, timeout};
use tracing::{info, instrument, warn};

const MAX_POSTS_FOR_CHUNK: usize = 200; // first-page only (vertical slice)
const CHUNK_MAX_CHARS: usize = 1_800; // keep prompt small for local models
const SUM_TIMEOUT_SECS: u64 = 120; // wrap around our own retry/HTTP timeouts

const OLLAMA_DEFAULT_BASE: &str = "http://127.0.0.1:11434";
static TAGS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<[^>]*>").unwrap());

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

    // Warm up the model once (ignore the result).
    let warm_prompt = build_prompt("warmup", "warmup");
    let _ = summarize_with_ollama(&client, &ollama_base, &model, &warm_prompt).await;

    // Fetch latest list of topics
    let latest: Latest = fetch_latest(&client).await?;
    info!("Fetched {} topics", latest.topic_list.topics.len());

    for topic in latest.topic_list.topics {
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
            continue;
        }

        // Build compact chunk from first-page posts
        let lines = load_plain_lines(&pool, topic.id as i64).await?;
        if lines.is_empty() {
            continue;
        }
        let chunk = make_chunk(&lines, CHUNK_MAX_CHARS);
        if chunk.is_empty() {
            continue;
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
    }

    info!("ETL + local LLM summaries complete.");
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

/* ---------------- Incremental guard ---------------- */

async fn posts_changed_since_last_llm(pool: &PgPool, topic_id: i64) -> Result<bool> {
    // latest post time we have
    let max_created: Option<OffsetDateTime> =
        query_scalar(r#"SELECT MAX(created_at) FROM posts WHERE topic_id = $1"#)
            .bind(topic_id)
            .fetch_one(pool)
            .await?;

    // last time we summarized with LLM
    let last_llm: Option<OffsetDateTime> =
        query_scalar(r#"SELECT updated_at FROM topic_summaries_llm WHERE topic_id = $1"#)
            .bind(topic_id)
            .fetch_optional(pool)
            .await?
            .flatten();

    Ok(match (max_created, last_llm) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(mc), Some(ts)) => mc > ts,
    })
}

/* ---------------- Text prep ---------------- */

async fn load_plain_lines(pool: &PgPool, topic_id: i64) -> Result<Vec<String>> {
    let rows = query(
        r#"SELECT id, created_at, cooked FROM posts WHERE topic_id = $1 ORDER BY created_at ASC LIMIT $2"#,
    )
    .bind(topic_id)
    .bind(MAX_POSTS_FOR_CHUNK as i64)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let cooked: String = r.get("cooked");
        let id: i64 = r.get("id");
        let created_at: OffsetDateTime = r.get("created_at");
        let t = strip_tags_fast(&cooked);
        if !t.is_empty() {
            let ts = created_at.format(&Rfc3339)?;
            out.push(format!("[post:{id} @ {ts}] {t}"));
        }
    }
    Ok(out)
}

fn strip_tags_fast(html: &str) -> String {
    let no_tags = TAGS_RE.replace_all(html, " ");
    squeeze_ws(no_tags.trim())
}

fn squeeze_ws(s: &str) -> String {
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

// char-safe chunker
fn make_chunk(lines: &[String], max_chars: usize) -> String {
    let mut cur = String::new();
    for l in lines {
        if cur.len() + l.len() + 1 > max_chars {
            // add as many chars as fit, on a UTF-8 boundary
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

fn take_prefix_chars(s: &str, max: usize) -> String {
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

fn build_prompt(topic_title: &str, chunk: &str) -> String {
    format!(
        "Thread: {title}\n\nContent excerpt:\n---\n{body}\n---\n\n\
         Summarize for a technical audience. Respond ONLY with strict JSON:\n\
         {{\"headline\": string, \"bullets\": [strings], \"citations\": [strings]}}\n\
         Rules:\n\
         - Headline ≤15 words\n\
         - 3–6 concise bullets with concrete facts, dates, statuses, decisions\n\
         - Citations reference [post:<id>] lines from the excerpt\n\
         - If off-topic/banter, set headline to 'Meta: off-topic' and bullets/citations to []\n\
         - No speculation. No marketing fluff.",
        title = topic_title,
        body = chunk
    )
}

/* ---------------- Ollama client (/api/chat) ---------------- */

#[derive(Serialize, Clone)]
struct Msg<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize, Clone)]
struct ChatReq<'a> {
    model: &'a str,
    stream: bool,
    messages: Vec<Msg<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<&'a str>, // keep model in memory for a bit
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOpts>,
}

#[derive(Serialize, Clone)]
struct OllamaOpts {
    temperature: f32,
    num_ctx: usize,
    top_p: f32,
    repeat_penalty: f32,
}

#[derive(Deserialize)]
struct ChatResp {
    message: ChatMsg,
}
#[derive(Deserialize)]
struct ChatMsg {
    content: String, /* role: String */
}

#[derive(Debug, Deserialize, Serialize)]
struct Summary {
    headline: String,
    bullets: Vec<String>,
    citations: Vec<String>,
}

async fn summarize_with_ollama(
    client: &Client,
    base: &str,
    model: &str,
    prompt: &str,
) -> Result<(Summary, usize, usize)> {
    let url = format!("{}/api/chat", base.trim_end_matches('/'));

    let body = ChatReq {
        model,
        stream: false,
        keep_alive: Some("5m"),
        messages: vec![
            Msg {
                role: "system",
                content: "You are a technical note-taker. Output a one-line headline and 3–6 factual bullets. \
                 Include dates/numbers from the text. No speculation. If off-topic/banter, say: 'Meta: off-topic'.",
            },
            Msg {
                role: "user",
                content: prompt,
            },
        ],
        options: Some(OllamaOpts {
            temperature: 0.2,
            num_ctx: 8192,
            top_p: 0.9,
            repeat_penalty: 1.05,
        }),
    };

    let bpe = cl100k_base().map_err(|e| anyhow!("tokenizer: {e:?}"))?;
    let in_tok: usize = body
        .messages
        .iter()
        .map(|m| bpe.clone().encode_with_special_tokens(m.content).len())
        .sum();

    let bpe_out = bpe.clone();
    let url_clone = url.clone();
    let body_clone = body.clone();

    let backoff = ExponentialBackoff {
        max_elapsed_time: Some(Duration::from_secs(120)),
        ..Default::default()
    };
    let op = move || {
        let bpe = bpe_out.clone();
        let url = url_clone.clone();
        let body = body_clone.clone();
        async move {
            // send; treat connection/send/parse problems as transient
            let resp = client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| backoff::Error::transient(anyhow!("transport: {e:?}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                return Err(backoff::Error::transient(anyhow!("http {status}: {text}")));
            }

            let r: ChatResp = resp
                .json()
                .await
                .map_err(|e| backoff::Error::transient(anyhow!("decode: {e:?}")))?;

            let raw = r.message.content;
            let out_tok = bpe.encode_with_special_tokens(&raw).len();
            let summary: Summary = serde_json::from_str(&raw)
                .map_err(|e| backoff::Error::transient(anyhow!("json: {e:?} raw: {raw}")))?;

            Ok::<(Summary, usize), backoff::Error<anyhow::Error>>((summary, out_tok))
        }
    };

    let (summary, out_tok) = retry(backoff, op).await?;
    Ok((summary, in_tok, out_tok))
}

/* ---------------- Utils ---------------- */

fn prompt_hash(topic_id: i64, model: &str, prompt: &str) -> String {
    let mut h = Sha256::new();
    h.update(model.as_bytes());
    h.update(b"\n");
    h.update(topic_id.to_be_bytes());
    h.update(b"\n");
    h.update(prompt.as_bytes());
    format!("{:x}", h.finalize())
}
