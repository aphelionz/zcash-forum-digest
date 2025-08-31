use std::time::Duration;

use anyhow::{Result, anyhow};
use backoff::{ExponentialBackoff, future::retry};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::BPE;

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
    keep_alive: Option<&'a str>,
}

#[derive(Deserialize)]
struct ChatResp {
    message: ChatMsg,
}

#[derive(Deserialize)]
struct ChatMsg {
    content: String,
}

pub async fn summarize_with_ollama(
    client: &Client,
    base: &str,
    model: &str,
    prompt: &str,
) -> Result<(String, usize, usize)> {
    let url = format!("{}/api/chat", base.trim_end_matches('/'));

    const SYSTEM: &str = "You are summarizing ONE forum thread excerpt.\nReturn a concise summary in plain text:\n- First line: a brief headline.\n- Subsequent lines: '- ' bullet points with key facts.\nDo NOT include post IDs, timestamps, author names, or URLs.";

    let body = ChatReq {
        model,
        stream: false,
        keep_alive: Some("5m"),
        messages: vec![
            Msg {
                role: "system",
                content: SYSTEM,
            },
            Msg {
                role: "user",
                content: prompt,
            },
        ],
    };

    let in_tok: usize = body
        .messages
        .iter()
        .map(|m| BPE.encode_with_special_tokens(m.content).len())
        .sum();

    let url_clone = url.clone();
    let body_clone = body.clone();

    let max_elapsed = std::env::var("OLLAMA_MAX_ELAPSED_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(120));

    let backoff = ExponentialBackoff {
        max_elapsed_time: Some(max_elapsed),
        ..Default::default()
    };
    let op = move || {
        let url = url_clone.clone();
        let body = body_clone.clone();
        async move {
            let resp = client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| backoff::Error::transient(anyhow!("transport: {e:?}")))?;

            let status = resp.status();
            if status.is_client_error() {
                let text = resp.text().await.unwrap_or_default();
                return Err(backoff::Error::permanent(anyhow!("http {status}: {text}")));
            } else if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(backoff::Error::transient(anyhow!("http {status}: {text}")));
            }

            let r: ChatResp = resp
                .json()
                .await
                .map_err(|e| backoff::Error::transient(anyhow!("decode: {e:?}")))?;

            let raw = r.message.content;
            let out_tok = BPE.encode_with_special_tokens(&raw).len();

            Ok::<(String, usize), backoff::Error<anyhow::Error>>((raw, out_tok))
        }
    };

    let (summary, out_tok) = retry(backoff, op).await?;
    Ok((summary, in_tok, out_tok))
}
