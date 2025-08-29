use anyhow::{Result, anyhow};
use serde::Deserialize;
use sqlx::{PgPool, Row};
use std::env;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[tokio::main]
async fn main() -> Result<()> {
    let pool = PgPool::connect(&env::var("DATABASE_URL")?).await?;
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("latest") => {
            let n: i64 = args.next().as_deref().unwrap_or("10").parse().unwrap_or(10);
            latest(&pool, n).await?;
        }
        Some("id") => {
            let id: i64 = args
                .next()
                .ok_or_else(|| anyhow!("missing <topic_id>"))?
                .parse()?;
            by_id(&pool, id).await?;
        }
        Some("search") => {
            let q = args.next().ok_or_else(|| anyhow!("missing <query>"))?;
            let n: i64 = args.next().as_deref().unwrap_or("20").parse().unwrap_or(20);
            search(&pool, &q, n).await?;
        }
        _ => {
            eprintln!(
                "usage:
  show latest [N]           # latest N summaries (prefer LLM)
  show id <topic_id>        # show one topic summary (LLM→heuristic)
  show search <query> [N]   # search in title/summary (LLM→heuristic)"
            );
        }
    }
    Ok(())
}

async fn latest(pool: &PgPool, n: i64) -> Result<()> {
    // Prefer LLM, else heuristic; order by most recently updated among the two.
    let rows = sqlx::query(
        r#"
        SELECT
          t.id,
          t.title,
          COALESCE(l.summary, s.summary)              AS summary,
          COALESCE(l.updated_at, s.updated_at)         AS updated_at,
          CASE WHEN l.summary IS NOT NULL THEN 'llm' ELSE 'heuristic' END AS source
        FROM topics t
        LEFT JOIN topic_summaries_llm l ON l.topic_id = t.id
        LEFT JOIN topic_summaries     s ON s.topic_id = t.id
        WHERE l.summary IS NOT NULL OR s.summary IS NOT NULL
        ORDER BY COALESCE(l.updated_at, s.updated_at) DESC
        LIMIT $1
        "#,
    )
    .bind(n)
    .fetch_all(pool)
    .await?;

    for r in rows {
        print_card(&r)?;
    }
    Ok(())
}

async fn by_id(pool: &PgPool, id: i64) -> Result<()> {
    let r = sqlx::query(
        r#"
        SELECT
          t.id,
          t.title,
          COALESCE(l.summary, s.summary)              AS summary,
          COALESCE(l.updated_at, s.updated_at)         AS updated_at,
          CASE WHEN l.summary IS NOT NULL THEN 'llm' ELSE 'heuristic' END AS source
        FROM topics t
        LEFT JOIN topic_summaries_llm l ON l.topic_id = t.id
        LEFT JOIN topic_summaries     s ON s.topic_id = t.id
        WHERE t.id = $1 AND (l.summary IS NOT NULL OR s.summary IS NOT NULL)
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = r {
        print_card(&row)?;
    } else {
        eprintln!("No summary for topic {}", id);
    }
    Ok(())
}

async fn search(pool: &PgPool, q: &str, n: i64) -> Result<()> {
    // Search title + both summary sources; prefer LLM text in results.
    let rows = sqlx::query(
        r#"
        SELECT
          t.id,
          t.title,
          COALESCE(l.summary, s.summary)              AS summary,
          COALESCE(l.updated_at, s.updated_at)         AS updated_at,
          CASE WHEN l.summary IS NOT NULL THEN 'llm' ELSE 'heuristic' END AS source
        FROM topics t
        LEFT JOIN topic_summaries_llm l ON l.topic_id = t.id
        LEFT JOIN topic_summaries     s ON s.topic_id = t.id
        WHERE
          (l.summary ILIKE '%' || $1 || '%' OR s.summary ILIKE '%' || $1 || '%' OR t.title ILIKE '%' || $1 || '%')
          AND (l.summary IS NOT NULL OR s.summary IS NOT NULL)
        ORDER BY COALESCE(l.updated_at, s.updated_at) DESC
        LIMIT $2
        "#
    )
    .bind(q)
    .bind(n)
    .fetch_all(pool)
    .await?;

    for r in rows {
        print_card(&r)?;
    }
    Ok(())
}

fn print_card(row: &sqlx::postgres::PgRow) -> Result<()> {
    let id: i64 = row.get("id");
    let title: String = row.get("title");
    let summary: String = row.get("summary");
    let source: String = row.get("source");
    let ts: Option<OffsetDateTime> = row.try_get("updated_at").ok();

    let when = ts
        .map(|t| {
            t.format(&Rfc3339)
                .unwrap_or_else(|_| t.unix_timestamp().to_string())
        })
        .unwrap_or_else(|| "unknown-time".to_string());

    println!("[{}] {}  ({source} • {when})", id, title);

    if let Ok(parsed) = serde_json::from_str::<LlmSummary>(&summary) {
        println!("{}", parsed.headline.trim());
        for (i, bullet) in parsed.bullets.iter().enumerate() {
            match parsed.citations.get(i) {
                Some(c) if !c.trim().is_empty() => {
                    println!(" - {} {}", bullet.trim(), c.trim());
                }
                _ => println!(" - {}", bullet.trim()),
            }
        }
    } else {
        println!("{}", summary.trim());
    }

    println!("---");
    Ok(())
}

#[derive(Deserialize)]
struct LlmSummary {
    headline: String,
    bullets: Vec<String>,
    #[serde(default)]
    citations: Vec<String>,
}
