use reqwest::Client;
use zc_forum_etl::summarize_with_ollama;

#[tokio::test]
async fn summarize_ollama_local() {
    let base = match std::env::var("OLLAMA_TEST_URL") {
        Ok(url) => url,
        Err(_) => return, // skip when server not available
    };
    let model = std::env::var("OLLAMA_TEST_MODEL").unwrap_or_else(|_| "qwen2.5:latest".to_string());
    let prompt = "Thread: test\n\nContent excerpt:\n---\nHello world\n---";
    let client = Client::new();
    let (summary, in_tok, out_tok) = summarize_with_ollama(&client, &base, &model, prompt)
        .await
        .expect("ollama call");
    assert!(!summary.headline.is_empty());
    assert!(!summary.bullets.is_empty());
    assert!(in_tok > 0);
    assert!(out_tok > 0);
}

#[tokio::test]
async fn summarize_ollama_retry_error() {
    std::env::set_var("OLLAMA_MAX_ELAPSED_SECS", "1");
    let client = Client::new();
    let prompt = "Thread: test\n\nContent excerpt:\n---\nHello world\n---";
    let res = summarize_with_ollama(&client, "http://127.0.0.1:1", "test-model", prompt).await;
    assert!(res.is_err());
    std::env::remove_var("OLLAMA_MAX_ELAPSED_SECS");
}
