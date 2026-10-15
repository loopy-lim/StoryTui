//! Director normalization pass integration: the model's structured response
//! fills `characters.toml`, and a failed call leaves the deterministic pack
//! untouched with a report warning.

use std::fs;

use story_import::{ImportOptions, import_card, normalize};
use story_provider::OpenAiCompatibleProvider;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn card_json() -> String {
    r#"{"name":"유나","description":"차분하고 관찰력이 좋다. 오래된 재즈를 좋아한다.","first_mes":"안녕."}"#
        .to_string()
}

fn provider_for(uri: String, structured_off: bool) -> OpenAiCompatibleProvider {
    let mut config = story_provider::ProviderConfig::defaults();
    config.base_url = uri;
    config.model = "test-model".to_string();
    if structured_off {
        config.structured_output = story_provider::StructuredOutputMode::Off;
    }
    OpenAiCompatibleProvider::new(config).expect("provider")
}

#[tokio::test]
async fn normalization_fills_structured_fields_and_keeps_description() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": r#"{"personality":["차분함","관찰력이 좋음"],"likes":["오래된 재즈"],"dislikes":[],"speech_style":"짧고 정확한 문장.","honorific_rule":"해요체","current_goal":"재회의 이유 확인","long_term_desire":"사라지지 않는 관계","known_facts":["플레이어가 서울에 산다"],"unknown_facts":["플레이어가 편지를 보냈다"]}"#
                }
            }]
        })))
        .mount(&server)
        .await;

    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("card.json");
    fs::write(&card_path, card_json()).expect("card");
    let out_dir = temp.path().join("pack");
    let mut output = import_card(&card_path, Some(&out_dir), &ImportOptions::default())
        .expect("deterministic import");

    let provider = provider_for(server.uri(), true);
    let summary = normalize::normalize_pack(&out_dir, &provider, &mut output.report).await;

    assert!(
        summary.applied,
        "normalization must rewrite characters.toml"
    );
    let characters = fs::read_to_string(out_dir.join("characters.toml")).expect("characters");
    assert!(characters.contains("차분함"), "{characters}");
    assert!(characters.contains("짧고 정확한 문장."), "{characters}");
    // description_md survives verbatim.
    assert!(
        characters.contains("차분하고 관찰력이 좋다"),
        "{characters}"
    );
    // The enriched report was already folded in by normalize_pack.
    assert!(
        output
            .report
            .mapped
            .iter()
            .any(|line| line.contains("Director 정규화")),
        "{:?}",
        output.report.mapped
    );
}

#[tokio::test]
async fn failed_normalization_keeps_the_deterministic_pack() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("card.json");
    fs::write(&card_path, card_json()).expect("card");
    let out_dir = temp.path().join("pack");
    let mut output = import_card(&card_path, Some(&out_dir), &ImportOptions::default())
        .expect("deterministic import");

    let provider = provider_for(server.uri(), true);
    let summary = normalize::normalize_pack(&out_dir, &provider, &mut output.report).await;

    assert!(!summary.applied, "a 500 keeps the scaffold");
    assert!(
        output
            .report
            .warnings
            .iter()
            .any(|warning| warning.contains("정규화 실패")),
        "{:?}",
        output.report.warnings
    );
    // The pack itself still verifies.
    story_persistence::PackLoader::load(&out_dir).expect("pack still loads");
}

#[tokio::test]
async fn prose_wrapped_responses_are_salvaged() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("personality"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "구조화 결과입니다:\n{\"personality\":[\"다정함\"],\"likes\":[],\"dislikes\":[],\"speech_style\":\"\",\"honorific_rule\":\"\",\"current_goal\":\"\",\"long_term_desire\":\"\",\"known_facts\":[],\"unknown_facts\":[]}\n위와 같습니다."
                }
            }]
        })))
        .mount(&server)
        .await;

    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("card.json");
    fs::write(&card_path, card_json()).expect("card");
    let out_dir = temp.path().join("pack");
    let mut output = import_card(&card_path, Some(&out_dir), &ImportOptions::default())
        .expect("deterministic import");

    let provider = provider_for(server.uri(), true);
    let summary = normalize::normalize_pack(&out_dir, &provider, &mut output.report).await;
    assert!(summary.applied);
    let characters = fs::read_to_string(out_dir.join("characters.toml")).expect("characters");
    assert!(characters.contains("다정함"), "{characters}");
}
