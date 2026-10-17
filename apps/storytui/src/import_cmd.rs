//! `storytui import <card> [옵션]` — compile a SillyTavern/RisuAI character
//! card into a native StoryTui pack directory, optionally followed by the
//! Director normalization pass against the configured provider.

use std::path::PathBuf;

use story_import::ImportOptions;
use story_provider::OpenAiCompatibleProvider;

/// Help text for the import subcommand.
pub fn usage() -> String {
    "사용법: storytui import <카드.png|.json> [옵션]

  -o, --out PATH        출력 팩 디렉터리 (기본: stories/<카드명-slug>)
  --player-name NAME    {{user}} 매크로 치환에 쓸 호칭 (기본: 플레이어)
  --language TAG        story.toml 언어 태그 (기본: ko-KR)
  --lorebook PATH       병합할 World Info(로어북) JSON
  --chat PATH           ST 채팅 내보내기(JSONL/객체)를 이력으로 저장
  --save-dir PATH       --chat 저장 슬롯 위치 (기본: saves)
  --normalize           Director 정규화 패스 실행 (생성 1회, 기본 OFF)
  --provider-config PATH  프로바이더 설정 TOML (기본: config/storytui*.toml)
  --provider NAME       --normalize가 쓸 프로필 이름 (기본: [provider])
  --force               기존 출력 디렉터를 덮어쓴다
  -h, --help            도움말"
        .to_string()
}

/// Parse `import` arguments (the `import` verb itself already consumed) and
/// run the import. Errors are human-readable Korean.
pub async fn run(args: &[String]) -> Result<(), String> {
    let mut card: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut options = ImportOptions::default();
    let mut normalize = false;
    let mut chat: Option<PathBuf> = None;
    let mut save_dir = "saves".to_string();
    let mut provider_config: Option<PathBuf> = None;
    let mut provider_name: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--help" => {
                println!("{}", usage());
                return Ok(());
            }
            "-o" | "--out" => {
                out = Some(PathBuf::from(take_value(args, &mut index, "-o")?));
            }
            "--player-name" => {
                options.player_name = take_value(args, &mut index, "--player-name")?;
            }
            "--language" => {
                options.language = take_value(args, &mut index, "--language")?;
            }
            "--lorebook" => {
                options.lorebook_path =
                    Some(PathBuf::from(take_value(args, &mut index, "--lorebook")?));
            }
            "--normalize" => normalize = true,
            "--chat" => {
                chat = Some(PathBuf::from(take_value(args, &mut index, "--chat")?));
            }
            "--save-dir" => {
                save_dir = take_value(args, &mut index, "--save-dir")?;
            }
            "--provider-config" => {
                provider_config = Some(PathBuf::from(take_value(
                    args,
                    &mut index,
                    "--provider-config",
                )?));
            }
            "--provider" => {
                provider_name = Some(take_value(args, &mut index, "--provider")?);
            }
            "--force" => options.force = true,
            other => {
                if other.starts_with('-') {
                    return Err(format!("알 수 없는 옵션: {other}\n\n{}", usage()));
                }
                if card.is_some() {
                    return Err(format!("카드 파일은 하나만 지정하세요\n\n{}", usage()));
                }
                card = Some(PathBuf::from(other));
            }
        }
        index += 1;
    }
    let Some(card) = card else {
        return Err(format!("카드 파일이 필요합니다\n\n{}", usage()));
    };

    let mut output = story_import::import_card(&card, out.as_deref(), &options)?;
    if normalize {
        let roster = storytui::config::provider_roster(provider_config.as_deref())?;
        let selected = match provider_name.as_deref() {
            Some(name) => roster
                .into_iter()
                .find(|entry| entry.name == name)
                .ok_or_else(|| format!("프로바이더 '{name}'가 구성에 없습니다"))?,
            None => roster
                .into_iter()
                .next()
                .ok_or("구성된 프로바이더가 없습니다")?,
        };
        let provider = OpenAiCompatibleProvider::new(selected)
            .map_err(|e| format!("프로바이더를 만들지 못했습니다: {e}"))?;
        println!("Director 정규화 중… (생성 1회)");
        let summary = story_import::normalize::normalize_pack(
            &output.pack_dir,
            &provider,
            &mut output.report,
        )
        .await;
        if summary.applied {
            println!("  정규화: {}", summary.filled.join(", "));
        }
        // Re-write the report so the file carries the normalization notes too.
        output
            .report
            .write(&output.pack_dir.join("import-report.md"))?;
    }
    if let Some(chat_path) = chat {
        seed_chat_history(
            &chat_path,
            &output.pack_dir,
            std::path::Path::new(&save_dir),
            &mut output.report,
        )?;
        output
            .report
            .write(&output.pack_dir.join("import-report.md"))?;
    }

    let report = &output.report;
    println!("가져오기 완료: {}", output.pack_dir.display());
    println!(
        "  리포트: {}",
        output.pack_dir.join("import-report.md").display()
    );
    if report.lorebook_entries > 0 {
        println!("  로어북 엔트리: {}개", report.lorebook_entries);
    }
    for warning in &report.warnings {
        println!("  경고: {warning}");
    }
    println!();
    println!(
        "실행: storytui --pack {} (게임 중 l → 'imported-chat' 슬롯으로 이어하기)",
        output.pack_dir.display()
    );
    Ok(())
}

/// Convert a chat export into history entries and write them as the
/// `imported-chat` save slot of the imported pack. The pack itself stays
/// text-only (ADR-003): the save lives in the save store, not the pack.
fn seed_chat_history(
    chat_path: &std::path::Path,
    pack_dir: &std::path::Path,
    save_root: &std::path::Path,
    report: &mut story_import::report::ImportReport,
) -> Result<(), String> {
    let text = std::fs::read_to_string(chat_path).map_err(|error| {
        format!(
            "채팅 파일을 읽지 못했습니다 ({}): {error}",
            chat_path.display()
        )
    })?;
    let turns = story_import::chat::parse_chat(&text, report)?;

    let entries: Vec<story_model::HistoryEntry> = turns
        .iter()
        .enumerate()
        .map(|(index, turn)| story_model::HistoryEntry {
            id: story_model::uuid::Uuid::new_v4(),
            turn: index as u32 + 1,
            scene_id: "imported-chat".to_string(),
            created_at: story_model::chrono::Utc::now(),
            narration: turn.narration.clone(),
            player_action: turn
                .player_action
                .as_ref()
                .map(|text| story_model::ParsedAction {
                    action_type: story_model::ActionType::GeneralAction,
                    target: None,
                    intent: None,
                    tone: None,
                    content: text.clone(),
                    original_text: text.clone(),
                    confidence: Some(0.0),
                }),
            choices: Vec::new(),
            applied_patch: story_model::StatePatch::default(),
            generation_id: story_model::uuid::Uuid::new_v4(),
        })
        .collect();
    let turn_count = entries.len();

    let pack = story_persistence::PackLoader::load(pack_dir)
        .map_err(|error| format!("임포트된 팩 로드 실패: {error}"))?;
    let mut engine = story_core::engine::EngineState::initial(&pack);
    engine.game.history = entries;
    engine.game.turn = turn_count as u32;

    let save_store = story_persistence::SaveStore::open(save_root).map_err(|error| {
        format!(
            "세이브 디렉터리를 열지 못했습니다 ({}): {error}",
            save_root.display()
        )
    })?;
    crate::host::saves::save_engine_snapshot(
        &save_store,
        &engine,
        "imported-chat".to_string(),
        Some(format!("임포트된 채팅 이력 {turn_count}턴")),
    )?;
    report.mapped.push(format!(
        "채팅 이력 {turn_count}턴 → '{}/imported-chat' 슬롯",
        save_root.display()
    ));
    println!("  채팅 이력: {turn_count}턴 → imported-chat 슬롯");
    Ok(())
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} 값이 필요합니다\n\n{}", usage()))
}
