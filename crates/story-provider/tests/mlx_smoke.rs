//! Real-MLX runtime smoke gate (ST-017 runtime gate).
//!
//! This test is `#[ignore]`-gated: it does NOT run in normal `cargo test`. Run
//! it explicitly with `cargo test -p story-provider --test mlx_smoke -- --ignored`
//! ONLY on an Apple-Silver host with a live MLX-LM / MLX-VLM OpenAI-compatible
//! server. It exercises `/health`, `/v1/models`, and one Korean chat
//! completion against the real server and asserts valid JSON.
//!
//! If `STORYTUI_MLX_URL` is unset, the test returns immediately (SKIP, not
//! FAIL) so it can never break normal CI. Closing the runtime gate requires a
//! real server + a 10-turn play; that work is OUT OF SCOPE for W6.

use std::time::Duration;

/// Run the real-MLX smoke against `STORYTUI_MLX_URL`.
#[tokio::test]
#[ignore = "runtime gate: requires a live MLX server; set STORYTUI_MLX_URL to enable"]
async fn mlx_smoke_health_models_korean_chat() {
    let url = match std::env::var("STORYTUI_MLX_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("STORYTUI_MLX_URL unset — skipping real-MLX smoke (expected in CI)");
            return;
        }
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("client");

    // /health
    let health = client
        .get(format!("{url}/health"))
        .send()
        .await
        .expect("health GET");
    assert!(
        health.status().is_success(),
        "health not ok: {}",
        health.status()
    );

    // /v1/models — assert valid JSON object.
    let models = client
        .get(format!("{url}/v1/models"))
        .send()
        .await
        .expect("models GET");
    assert!(
        models.status().is_success(),
        "models not ok: {}",
        models.status()
    );
    let models_json: serde_json::Value = models.json().await.expect("models body is JSON");
    assert!(models_json.is_object(), "models response is not an object");
    let advertised_models = models_json
        .get("data")
        .and_then(serde_json::Value::as_array)
        .expect("models response has no data array")
        .iter()
        .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert!(
        !advertised_models.is_empty(),
        "models response advertised no usable model ids"
    );
    let model =
        std::env::var("STORYTUI_MLX_MODEL").unwrap_or_else(|_| advertised_models[0].to_string());
    assert!(
        advertised_models.contains(&model.as_str()),
        "requested model {model} is not advertised by /v1/models: {advertised_models:?}"
    );

    // One Korean chat completion — assert a valid envelope with choices.
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "한 줄로 답해줘: 안녕?"}],
        "temperature": 0.85,
        "max_tokens": 64
    });
    let chat = client
        .post(format!("{url}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .expect("chat POST");
    assert!(chat.status().is_success(), "chat not ok: {}", chat.status());
    let chat_json: serde_json::Value = chat.json().await.expect("chat body is JSON");
    assert!(
        chat_json
            .get("choices")
            .and_then(|c| c.as_array())
            .is_some(),
        "chat response has no choices array"
    );
    eprintln!("mlx smoke ok against {url}");
}
