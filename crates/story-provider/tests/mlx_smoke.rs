//! Real-MLX runtime smoke gate (ST-017 runtime gate).
//!
//! This test is `#[ignore]`-gated: it does NOT run in normal `cargo test`. Run
//! it explicitly with `cargo test -p story-provider --test mlx_smoke -- --ignored`
//! ONLY on an Apple-Silver host with a live MLX-LM / MLX-VLM OpenAI-compatible
//! server. It exercises `/health`, `/v1/models`, and one Korean chat
//! completion against the real server and asserts valid JSON.
//!
//! If `STORYTUI_MLX_URL` is unset, the test returns immediately (SKIP, not
//! FAIL) so it can never break normal CI. The focused tests below additionally
//! verify the exact choice-only and full-turn StoryTUI contracts.

use std::time::Duration;

use story_model::{ChatMessage, TurnRequest};
use story_provider::{LlmProvider, OpenAiCompatibleProvider, ProviderConfig, StructuredOutputMode};
use tokio_util::sync::CancellationToken;

fn live_url() -> Option<String> {
    match std::env::var("STORYTUI_MLX_URL") {
        Ok(url) => Some(url),
        Err(_) => {
            eprintln!("STORYTUI_MLX_URL unset — skipping real-MLX smoke (expected in CI)");
            None
        }
    }
}

async fn advertised_models(client: &reqwest::Client, url: &str) -> Vec<String> {
    let response = client
        .get(format!("{url}/v1/models"))
        .send()
        .await
        .expect("models GET");
    assert!(
        response.status().is_success(),
        "models not ok: {}",
        response.status()
    );
    response
        .json::<serde_json::Value>()
        .await
        .expect("models body is JSON")["data"]
        .as_array()
        .expect("models response has no data array")
        .iter()
        .filter_map(|entry| entry["id"].as_str().map(str::to_string))
        .collect()
}

fn live_provider(url: &str, model: &str) -> OpenAiCompatibleProvider {
    let mut config = ProviderConfig::defaults();
    config.base_url = url.to_string();
    config.model = model.to_string();
    config.structured_output = StructuredOutputMode::On;
    config.request_timeout_ms = 600_000;
    config.connect_timeout_ms = 10_000;
    config.max_retries = 0;
    config.max_tokens = 2_400;
    config.min_narration_chars = 0;
    // Mirror the checked-in StoryTUI profile. The provider owns sampling
    // policy, so request-local values are intentionally capped/overridden.
    config.temperature = 0.5;
    config.top_p = 0.92;
    config.top_k = 40;
    OpenAiCompatibleProvider::new(config).expect("live provider")
}

fn generated_turn_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["narration", "choices", "state_patch", "summary"],
        "properties": {
            "narration": {"type": "string", "minLength": 600, "maxLength": 900},
            "choices": {
                "type": "array", "minItems": 3, "maxItems": 3,
                "items": {
                    "type": "object", "additionalProperties": false,
                    "required": ["id", "label", "intent", "tone", "target", "risk", "hidden_requirements", "preview"],
                    "properties": {
                        "id": {"type": "string", "minLength": 1, "maxLength": 40},
                        "label": {"type": "string", "minLength": 1, "maxLength": 36},
                        "intent": {"type": "string", "minLength": 1, "maxLength": 72},
                        "tone": {"type": ["string", "null"], "maxLength": 20},
                        "target": {"type": ["string", "null"], "maxLength": 40},
                        "risk": {"type": "string", "enum": ["safe", "low", "medium", "high", "severe"]},
                        "hidden_requirements": {"type": "array", "maxItems": 0, "items": {"type": "object"}},
                        "preview": {"type": "string", "minLength": 1, "maxLength": 72}
                    }
                }
            },
            "state_patch": {
                "type": "object", "additionalProperties": false,
                "required": ["affection_delta", "trust_delta", "tension_delta", "relationship_transitions", "set_flags", "number_delta", "set_strings", "inventory_delta", "add_events", "next_location", "next_time_slot", "proposed_route"],
                "properties": {
                    "affection_delta": {"type": "object", "maxProperties": 0, "additionalProperties": {"type": "integer"}},
                    "trust_delta": {"type": "object", "maxProperties": 0, "additionalProperties": {"type": "integer"}},
                    "tension_delta": {"type": "object", "maxProperties": 0, "additionalProperties": {"type": "integer"}},
                    "relationship_transitions": {"type": "object", "maxProperties": 0, "additionalProperties": {"type": "string"}},
                    "set_flags": {"type": "object", "maxProperties": 0, "additionalProperties": {"type": "boolean"}},
                    "number_delta": {"type": "object", "maxProperties": 0, "additionalProperties": {"type": "integer"}},
                    "set_strings": {"type": "object", "maxProperties": 0, "additionalProperties": {"type": "string"}},
                    "inventory_delta": {"type": "object", "maxProperties": 0, "additionalProperties": {"type": "integer"}},
                    "add_events": {"type": "array", "maxItems": 0, "items": {"type": "string"}},
                    "next_location": {"type": ["string", "null"]},
                    "next_time_slot": {"type": ["string", "null"]},
                    "proposed_route": {"type": ["string", "null"]}
                }
            },
            "summary": {
                "type": "object", "additionalProperties": false,
                "required": ["facts", "unresolved", "relationship_changes", "knowledge_changes", "interpretation", "address_form_changes", "important_player_actions", "source_history_ids"],
                "properties": {
                    "facts": {"type": "array", "maxItems": 0, "items": {"type": "string"}},
                    "unresolved": {"type": "array", "maxItems": 0, "items": {"type": "string"}},
                    "relationship_changes": {"type": "array", "maxItems": 0, "items": {"type": "string"}},
                    "knowledge_changes": {"type": "array", "maxItems": 0, "items": {"type": "string"}},
                    "interpretation": {"type": ["string", "null"]},
                    "address_form_changes": {"type": "array", "maxItems": 0, "items": {"type": "string"}},
                    "important_player_actions": {"type": "array", "maxItems": 0, "items": {"type": "string"}},
                    "source_history_ids": {"type": "array", "maxItems": 0, "items": {"type": "string"}}
                }
            }
        }
    })
}

fn generated_choices_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["choices"],
        "properties": {
            "choices": {
                "type": "array", "minItems": 3, "maxItems": 3,
                "items": {
                    "type": "object", "additionalProperties": false,
                    "required": ["label", "intent", "risk", "preview"],
                    "properties": {
                        "label": {"type": "string", "minLength": 1, "maxLength": 36},
                        "intent": {"type": "string", "minLength": 1, "maxLength": 72},
                        "risk": {"type": "string", "enum": ["safe", "low", "medium", "high", "severe"]},
                        "preview": {"type": "string", "minLength": 1, "maxLength": 72}
                    }
                }
            }
        }
    })
}

fn has_korean(text: &str) -> bool {
    text.chars().any(|ch| ('가'..='힣').contains(&ch))
}

fn has_refusal(text: &str) -> bool {
    let lowered = text.to_lowercase();
    [
        "도와드릴 수 없",
        "제공할 수 없",
        "응답할 수 없",
        "can't assist",
        "cannot assist",
        "i'm sorry",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
}

/// Run the real-MLX smoke against `STORYTUI_MLX_URL`.
#[tokio::test]
#[ignore = "runtime gate: requires a live MLX server; set STORYTUI_MLX_URL to enable"]
async fn mlx_smoke_health_models_korean_chat() {
    let Some(url) = live_url() else { return };
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

/// Choice-model runtime gate: the exact StoryTUI turn parser must accept a
/// compact, all-ages Korean choice rewrite. This catches model-id routing,
/// output-cap regressions, schema failures, unnecessary refusals, duplicate
/// labels, passive filler, and missing previews in one live request.
#[tokio::test]
#[ignore = "runtime gate: requires live MLX + STORYTUI_MLX_CHOICE_MODEL"]
async fn mlx_choice_model_structured_safe_gate() {
    let Some(url) = live_url() else { return };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .expect("client");
    let models = advertised_models(&client, &url).await;
    let choice_model = std::env::var("STORYTUI_MLX_CHOICE_MODEL")
        .or_else(|_| std::env::var("STORYTUI_MLX_MODEL"))
        .unwrap_or_else(|_| models[0].clone());
    assert!(
        models.contains(&choice_model),
        "choice model is not advertised: {choice_model}"
    );

    let provider = live_provider(&url, &choice_model);
    let request = TurnRequest {
        generation_id: story_model::uuid::Uuid::new_v4(),
        model: choice_model,
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "너는 전체 이용가 한국어 미스터리 선택지 디자이너다. 플레이어의 행동은 선택지로만 제안하고, 생각 과정이나 설명 없이 주어진 선택지 전용 JSON 스키마만 즉시 출력한다.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "폐관 십 분 전 박물관에서 사라진 열쇠를 찾는 장면의 선택지 3개를 만든다. 단서를 확인하는 낮은 위험 행동, 경비원에게 사실을 밝히는 관계 행동, 잠기기 전 전시실로 이동하는 높은 위험 행동을 하나씩 쓴다. 각 항목은 label, intent, risk, preview 네 필드만 사용한다. label은 짧고 구체적인 행동, preview는 즉시 달라질 결과다. 기다리기·바라보기·분위기 살피기와 같은 수동적 변형은 금지하고 risk도 서로 다르게 한다.".to_string(),
            },
        ],
        temperature: 0.6,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 900,
        json_schema: Some(generated_choices_schema()),
    };
    let draft = provider
        .generate_turn(request, CancellationToken::new())
        .await
        .expect("choice model must satisfy the StoryTUI contract");

    assert_eq!(draft.choices.len(), 3);
    let combined = draft
        .choices
        .iter()
        .flat_map(|choice| [&choice.label, &choice.intent])
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(has_korean(&combined), "choice output must contain Korean");
    assert!(!has_refusal(&combined), "choice model refused safe fiction");
    let labels = draft
        .choices
        .iter()
        .map(|choice| choice.label.trim().to_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(labels.len(), 3, "choice labels must be distinct");
    assert!(draft.choices.iter().all(|choice| {
        choice
            .preview
            .as_deref()
            .is_some_and(|preview| !preview.trim().is_empty())
    }));
    let passive = ["기다", "바라", "살핀", "지켜본"];
    assert!(
        draft
            .choices
            .iter()
            .all(|choice| !passive.iter().any(|word| choice.label.contains(word))),
        "choice labels must not collapse to passive filler"
    );
    let risks = draft
        .choices
        .iter()
        .map(|choice| format!("{:?}", choice.risk))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(risks.len() >= 2, "choice risk levels must diverge");
}

/// Writer-model runtime gate for Korean prose, the real full-turn schema, and
/// an all-ages mystery scene without an unnecessary refusal.
#[tokio::test]
#[ignore = "runtime gate: requires live MLX + STORYTUI_MLX_MODEL"]
async fn mlx_writer_model_korean_safe_contract_gate() {
    let Some(url) = live_url() else { return };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .expect("client");
    let models = advertised_models(&client, &url).await;
    let writer_model = std::env::var("STORYTUI_MLX_MODEL").unwrap_or_else(|_| models[0].clone());
    assert!(
        models.contains(&writer_model),
        "writer model is not advertised: {writer_model}"
    );

    let provider = live_provider(&url, &writer_model);
    let request = TurnRequest {
        generation_id: story_model::uuid::Uuid::new_v4(),
        model: writer_model,
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "너는 전체 이용가 한국어 미스터리 작가다. 플레이어가 입력하지 않은 감정이나 행동을 확정하지 않고 JSON 스키마만 출력한다.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "폐관 십 분 전 박물관에서 유나가 사라진 보관함 열쇠의 단서를 발견하는 장면을 반복 없이 600~900자 한국어로 쓴다. 경비 방송과 멀어지는 발소리, 젖은 우산을 단서로 활용한다. 이어질 행동 선택지 3개는 서로 다른 결과와 위험도를 가져야 한다. state_patch와 summary의 배열과 객체는 모두 비우고 nullable 필드는 null로 쓴다.".to_string(),
            },
        ],
        temperature: 0.55,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1_800,
        json_schema: Some(generated_turn_schema()),
    };
    let draft = provider
        .generate_turn(request, CancellationToken::new())
        .await
        .expect("writer model must satisfy the StoryTUI contract");

    assert!((600..=900).contains(&draft.narration.chars().count()));
    assert!(has_korean(&draft.narration));
    assert!(!has_refusal(&draft.narration));
    assert_eq!(draft.choices.len(), 3);
    let labels = draft
        .choices
        .iter()
        .map(|choice| choice.label.trim().to_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(labels.len(), 3, "writer choices must be distinct");
    assert!(draft.choices.iter().all(|choice| {
        choice
            .preview
            .as_deref()
            .is_some_and(|preview| !preview.trim().is_empty())
    }));
    let risks = draft
        .choices
        .iter()
        .map(|choice| format!("{:?}", choice.risk))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(risks.len() >= 2, "writer choice risk levels must diverge");
}
