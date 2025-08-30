use anyhow::Result;
use rss::{ChannelBuilder, ItemBuilder};
use serde::Deserialize;
use sqlx::{PgPool, Row};
use time::{OffsetDateTime, format_description::well_known::Rfc2822};
use zc_forum_etl::strip_tags_fast;

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
        "<h1>Zcash Forum Digest for {}</h1><p><a href=\"rss.xml\">RSS Feed</a></p>",
        OffsetDateTime::now_utc().date()
    ));

    let mut items = Vec::new();
    for row in rows {
        let id: i64 = row.get("id");
        let title: String = row.get("title");
        let summary_json: Option<String> = row.get("summary");
        let last_post: OffsetDateTime = row.get("last_post");
        html.push_str(&format!("<h2>{}</h2>", title));

        let mut desc = String::new();

        if let Some(js) = summary_json {
            if let Ok(s) = serde_json::from_str::<LlmSummary>(&js) {
                let mut ctx = s.headline;
                if !s.bullets.is_empty() {
                    if !ctx.is_empty() {
                        ctx.push(' ');
                    }
                    ctx.push_str(&s.bullets.join(" "));
                }
                if !ctx.is_empty() {
                    html.push_str(&format!("<p>{}</p>", ctx));
                    desc.push_str(&ctx);
                }
            }
        }

        // recent posts in last 24 hours
        let recent = sqlx::query(
            r#"SELECT username, cooked, created_at FROM posts
               WHERE topic_id = $1 AND created_at >= now() - interval '1 day'
               ORDER BY created_at ASC"#,
        )
        .bind(id)
        .fetch_all(&pool)
        .await?;

        if !recent.is_empty() {
            html.push_str("<h3>Last 24h</h3><ul>");
            for rp in recent {
                let username: String = rp.get("username");
                let cooked: String = rp.get("cooked");
                let text = strip_tags_fast(&cooked);
                html.push_str(&format!("<li><b>{}</b>: {}</li>", username, text));
                if !desc.is_empty() {
                    desc.push_str(" \u{2022} ");
                }
                desc.push_str(&format!("{}: {}", username, text));
            }
            html.push_str("</ul>");
        }

        let pub_date = last_post.format(&Rfc2822)?;
        let item = ItemBuilder::default()
            .title(title.clone())
            .link(format!("https://forum.zcashcommunity.com/t/{id}"))
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
