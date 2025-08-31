use std::time::Duration as StdDuration;

use anyhow::Result;
use reqwest::{Client, StatusCode};
use rss::{ChannelBuilder, ItemBuilder};
use serde::Deserialize;
use time::{
    Duration, OffsetDateTime,
    format_description::well_known::{Rfc2822, Rfc3339},
};
use tokio::time::{sleep, timeout};
use tracing::{info, warn};
use zc_forum_etl::{Summary, make_chunk, strip_tags_fast, summarize_with_ollama};

const CHUNK_MAX_CHARS: usize = 1_800;
const SUM_TIMEOUT_SECS: u64 = 240;
const PAGE_SIZE: usize = 20;
const MAX_POSTS_FOR_CHUNK: usize = 200;
const CUTOFF_HOURS: i64 = 24;

#[derive(Deserialize)]
struct Latest {
    topic_list: TopicList,
}

#[derive(Deserialize)]
struct TopicList {
    topics: Vec<TopicStub>,
}

#[derive(Deserialize)]
struct TopicStub {
    id: u64,
    title: String,
}

#[derive(Deserialize)]
struct TopicFull {
    post_stream: PostStream,
}

#[derive(Deserialize)]
struct PostStream {
    posts: Vec<Post>,
}

#[derive(Deserialize, Clone)]
struct Post {
    id: u64,
    cooked: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    // HTTP client: generous timeouts for local servers.
    let client = Client::builder()
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(StdDuration::from_secs(120))
        .build()?;

    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "qwen2.5:latest".to_string());
    let ollama_base =
        std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());

    // Warmup
    let warm_prompt = build_prompt("warmup", "warmup");
    if let Err(e) = summarize_with_ollama(&client, &ollama_base, &model, &warm_prompt).await {
        warn!("Warm-up summarize_with_ollama failed: {e}");
    }

    let latest: Latest = fetch_latest(&client).await?;
    info!("Fetched {} topics", latest.topic_list.topics.len());

    let mut html = String::with_capacity(1024);
    html.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Zcash Forum Digest</title></head><body>");
    html.push_str(&format!(
        "<h1>Zcash Forum Digest for {}</h1><p><a href=\"rss.xml\">RSS Feed</a></p>",
        OffsetDateTime::now_utc().date()
    ));

    let mut items = Vec::with_capacity(latest.topic_list.topics.len());
    let cutoff = OffsetDateTime::now_utc() - Duration::hours(CUTOFF_HOURS);

    for stub in latest.topic_list.topics {
        let posts = fetch_posts(&client, stub.id, cutoff).await?;
        if posts.is_empty() {
            continue;
        }
        let last_post = posts
            .iter()
            .map(|p| p.created_at)
            .max()
            .unwrap_or_else(OffsetDateTime::now_utc);

        let lines = posts_to_lines(posts.iter());

        let mut recent_html = String::new();
        let mut desc = String::new();
        if !lines.is_empty() {
            let chunk = make_chunk(&lines, CHUNK_MAX_CHARS);
            if !chunk.is_empty() {
                let prompt = build_prompt(&stub.title, &chunk);
                match timeout(
                    StdDuration::from_secs(SUM_TIMEOUT_SECS),
                    summarize_with_ollama(&client, &ollama_base, &model, &prompt),
                )
                .await
                {
                    Ok(Ok((summary, _, _))) => {
                        recent_html = summary_to_html(&summary);
                        desc = summary_to_text(&summary);
                    }
                    Ok(Err(e)) => warn!("LLM summarize failed for {}: {e}", stub.id),
                    Err(_) => warn!("LLM summarize timed out for {}", stub.id),
                }
            }
        }

        html.push_str(&format!("<h2>{}</h2>", stub.title));
        if !recent_html.is_empty() {
            html.push_str(&recent_html);
        }

        let pub_date = last_post.format(&Rfc2822)?;
        let item = ItemBuilder::default()
            .title(stub.title.clone())
            .link(format!("https://forum.zcashcommunity.com/t/{}", stub.id))
            .description((!desc.is_empty()).then_some(desc))
            .pub_date(pub_date)
            .build();
        items.push(item);
    }

    html.push_str("</body></html>");
    std::fs::create_dir_all("public")?;
    std::fs::write("public/index.html", html)?;

    let channel = ChannelBuilder::default()
        .title(format!(
            "Zcash Forum Digest for {}",
            OffsetDateTime::now_utc().date()
        ))
        .link("https://forum.zcashcommunity.com")
        .description("Topics updated in the last 24 hours")
        .items(items)
        .build();
    std::fs::write("public/rss.xml", channel.to_string())?;
    Ok(())
}

fn summary_to_html(s: &Summary) -> String {
    let mut out = String::new();
    if !s.headline.is_empty() {
        out.push_str(&format!("<p>{}</p>", s.headline));
    }
    if !s.bullets.is_empty() {
        out.push_str("<ul>");
        for b in &s.bullets {
            out.push_str(&format!("<li>{}</li>", b));
        }
        out.push_str("</ul>");
    }
    out
}

fn summary_to_text(s: &Summary) -> String {
    let mut out = String::new();
    if !s.headline.is_empty() {
        out.push_str(&s.headline);
    }
    for b in &s.bullets {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("- ");
        out.push_str(b);
    }
    out
}

async fn fetch_latest(client: &Client) -> Result<Latest> {
    Ok(client
        .get("https://forum.zcashcommunity.com/latest.json")
        .send()
        .await?
        .error_for_status()?
        .json::<Latest>()
        .await?)
}

async fn fetch_topic_page(client: &Client, id: u64, page: u32) -> Result<TopicFull> {
    let url = if page == 0 {
        format!("https://forum.zcashcommunity.com/t/{}.json", id)
    } else {
        format!(
            "https://forum.zcashcommunity.com/t/{}.json?page={}",
            id, page
        )
    };
    Ok(client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<TopicFull>()
        .await?)
}

async fn fetch_posts(client: &Client, id: u64, cutoff: OffsetDateTime) -> Result<Vec<Post>> {
    let mut all = Vec::new();
    let mut page = 0;
    loop {
        match fetch_topic_page(client, id, page).await {
            Ok(tf) => {
                let count = tf.post_stream.posts.len();
                if count == 0 {
                    break;
                }
                all.extend(
                    tf.post_stream
                        .posts
                        .into_iter()
                        .filter(|p| p.created_at >= cutoff),
                );
                if count < PAGE_SIZE {
                    break;
                }
                page += 1;
                sleep(StdDuration::from_secs(1)).await;
            }
            Err(e) => {
                if let Some(req_err) = e.downcast_ref::<reqwest::Error>() {
                    if req_err.status() == Some(StatusCode::NOT_FOUND) {
                        break;
                    }
                }
                return Err(e);
            }
        }
    }
    Ok(all)
}

fn posts_to_lines<'a>(posts: impl Iterator<Item = &'a Post>) -> Vec<String> {
    let mut out = Vec::new();
    for p in posts.take(MAX_POSTS_FOR_CHUNK) {
        let t = strip_tags_fast(&p.cooked);
        if t.is_empty() {
            continue;
        }
        if let Ok(ts) = p.created_at.format(&Rfc3339) {
            out.push(format!("[post:{} @ {}] {}", p.id, ts, t));
        }
    }
    out
}

fn build_prompt(topic_title: &str, chunk: &str) -> String {
    format!(
        "Thread: {title}\n\nContent excerpt:\n---\n{body}\n---",
        title = topic_title,
        body = chunk
    )
}
