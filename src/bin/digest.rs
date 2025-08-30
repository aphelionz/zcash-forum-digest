use anyhow::Result;
use serde::Deserialize;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

#[derive(Deserialize)]
struct LlmSummary {
    headline: String,
    bullets: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;

    // fetch topics with activity in last 24 hours
    let rows = sqlx::query(
        r#"SELECT t.id, t.title, ts.summary, MAX(p.created_at) AS last_post
            FROM topics t
            JOIN posts p ON t.id = p.topic_id
            LEFT JOIN topic_summaries_llm ts ON t.id = ts.topic_id
            WHERE p.created_at >= now() - interval '1 day'
            GROUP BY t.id, t.title, ts.summary
            ORDER BY last_post DESC"#,
    )
    .fetch_all(&pool)
    .await?;

    let mut html = String::new();
    html.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Zcash Forum Digest</title></head><body>");
    html.push_str(&format!(
        "<h1>Zcash Forum Digest for {}</h1>",
        OffsetDateTime::now_utc().date()
    ));

    for row in rows {
        let title: String = row.get("title");
        let summary_json: Option<String> = row.get("summary");
        html.push_str(&format!("<h2>{}</h2>", title));
        if let Some(js) = summary_json {
            if let Ok(s) = serde_json::from_str::<LlmSummary>(&js) {
                if !s.bullets.is_empty() {
                    html.push_str("<ul>");
                    for b in s.bullets {
                        html.push_str(&format!("<li>{}</li>", b));
                    }
                    html.push_str("</ul>");
                }
            }
        }
    }

    html.push_str("</body></html>");
    std::fs::create_dir_all("public")?;
    std::fs::write("public/index.html", html)?;
    Ok(())
}
