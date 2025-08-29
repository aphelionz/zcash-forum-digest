use anyhow::{Context, Result, anyhow};
use backoff::{ExponentialBackoff, future::retry};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub struct LlmConfig {
    pub provider: LlmProvider,
    pub model: String,
    pub max_input_tokens: usize,
    pub openai_base: Option<String>, // vLLM/OpenAI
    pub ollama_base: Option<String>, // Ollama
}

#[derive(Clone, Copy)]
pub enum LlmProvider {
    Off,
    OpenAi,
    Vllm,
    Ollama,
}

impl LlmProvider {
    pub fn from_env() -> Self {
        match std::env::var("LLM_SUMMARIZER")
            .unwrap_or_else(|_| "off".into())
            .to_lowercase()
            .as_str()
        {
            "openai" => Self::OpenAi,
            "vllm" => Self::Vllm,
            "ollama" => Self::Ollama,
            _ => Self::Off,
        }
    }
}

pub fn cfg_from_env() -> LlmConfig {
    LlmConfig {
        provider: LlmProvider::from_env(),
        model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
        max_input_tokens: std::env::var("LLM_MAX_INPUT_TOKENS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8000),
        openai_base: std::env::var("OPENAI_BASE_URL").ok(), // vLLM uses this
        ollama_base: std::env::var("OLLAMA_BASE_URL").ok(), // http://127.0.0.1:11434
    }
}

pub fn prompt_hash(topic_id: i64, model: &str, prompt: &str) -> String {
    let mut h = Sha256::new();
    h.update(model.as_bytes());
    h.update(b"\n");
    h.update(topic_id.to_be_bytes());
    h.update(b"\n");
    h.update(prompt.as_bytes());
    format!("{:x}", h.finalize())
}

pub async fn summarize(
    client: &Client,
    cfg: &LlmConfig,
    prompt: &str,
) -> Result<(String, usize, usize)> {
    match cfg.provider {
        LlmProvider::Off => Err(anyhow!("LLM provider is Off")),
        LlmProvider::OpenAi => summarize_with_openai_like(client, cfg, prompt, true).await,
        LlmProvider::Vllm => summarize_with_openai_like(client, cfg, prompt, false).await,
        LlmProvider::Ollama => summarize_with_ollama(client, cfg, prompt).await,
    }
}

/* ---------- OpenAI / vLLM (OpenAI-compatible) ---------- */
#[derive(Serialize)]
struct ChatReq<'a> {
    model: &'a str,
    messages: Vec<Msg<'a>>,
}
#[derive(Serialize)]
struct Msg<'a> {
    role: &'a str,
    content: &'a str,
}
#[derive(Deserialize)]
struct ChatResp {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}
#[derive(Deserialize)]
struct Choice {
    message: MsgOwned,
}
#[derive(Deserialize)]
struct MsgOwned {
    role: String,
    content: String,
}
#[derive(Deserialize)]
struct Usage {
    prompt_tokens: usize,
    completion_tokens: usize,
}

async fn summarize_with_openai_like(
    client: &Client,
    cfg: &LlmConfig,
    prompt: &str,
    use_openai_key: bool,
) -> Result<(String, usize, usize)> {
    let base = cfg
        .openai_base
        .clone()
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let url = format!("{}/chat/completions", base);
    let api_key = if use_openai_key {
        Some(std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not set")?)
    } else {
        None
    };

    let req_body = ChatReq {
        model: &cfg.model,
        messages: vec![
            Msg {
                role: "system",
                content: "You are a technical note-taker. Summarize concisely with bullet points and dates. \
                 Do not invent facts. Include a short headline and 3–6 bullets.",
            },
            Msg {
                role: "user",
                content: prompt,
            },
        ],
    };

    let op = || async {
        let mut b = client.post(&url).json(&req_body);
        if let Some(k) = &api_key {
            b = b.bearer_auth(k);
        }
        let resp = b.send().await?;

        if resp.status() == StatusCode::TOO_MANY_REQUESTS || resp.status().is_server_error() {
            Err(backoff::Error::transient(anyhow!("HTTP {}", resp.status())))
        } else {
            let r: ChatResp = resp.error_for_status()?.json().await?;
            let text = r
                .choices
                .get(0)
                .map(|c| c.message.content.clone())
                .unwrap_or_default();
            let in_tok = r
                .usage
                .as_ref()
                .map(|u| u.prompt_tokens)
                .unwrap_or_default();
            let out_tok = r
                .usage
                .as_ref()
                .map(|u| u.completion_tokens)
                .unwrap_or_default();
            Ok((text, in_tok, out_tok))
        }
    };

    retry(
        ExponentialBackoff {
            max_elapsed_time: Some(std::time::Duration::from_secs(20)),
            ..Default::default()
        },
        op,
    )
    .await
}

/* ---------- Ollama ---------- */
#[derive(Serialize)]
struct OllamaReq<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
}
#[derive(Deserialize)]
struct OllamaResp {
    response: String,
}

async fn summarize_with_ollama(
    client: &Client,
    cfg: &LlmConfig,
    prompt: &str,
) -> Result<(String, usize, usize)> {
    let base = cfg
        .ollama_base
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
    let url = format!("{}/api/generate", base);

    let op = || async {
        let resp = client
            .post(&url)
            .json(&OllamaReq {
                model: &cfg.model,
                prompt,
                stream: false,
            })
            .send()
            .await?;

        if resp.status().is_server_error() {
            Err(backoff::Error::transient(anyhow!("HTTP {}", resp.status())))
        } else {
            let r: OllamaResp = resp.error_for_status()?.json().await?;
            Ok((r.response, 0usize, 0usize)) // Ollama doesn't return token counts
        }
    };

    retry(
        ExponentialBackoff {
            max_elapsed_time: Some(std::time::Duration::from_secs(20)),
            ..Default::default()
        },
        op,
    )
    .await
}
