//! `storytui import <card> [옵션]` — compile a SillyTavern/RisuAI character
//! card into a native StoryTui pack directory.

use std::path::PathBuf;

use story_import::ImportOptions;

/// Help text for the import subcommand.
pub fn usage() -> String {
    "사용법: storytui import <카드.png|.json> [옵션]

  -o, --out PATH        출력 팩 디렉터리 (기본: stories/<카드명-slug>)
  --player-name NAME    {{user}} 매크로 치환에 쓸 호칭 (기본: 플레이어)
  --language TAG        story.toml 언어 태그 (기본: ko-KR)
  --lorebook PATH       병합할 World Info(로어북) JSON
  --force               기존 출력 디렉터를 덮어쓴다
  -h, --help            도움말"
        .to_string()
}

/// Parse `import` arguments (the `import` verb itself already consumed) and
/// run the import. Errors are human-readable Korean.
pub fn run(args: &[String]) -> Result<(), String> {
    let mut card: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut options = ImportOptions::default();
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

    let output = story_import::import_card(&card, out.as_deref(), &options)?;
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
    println!("실행: storytui --pack {}", output.pack_dir.display());
    Ok(())
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} 값이 필요합니다\n\n{}", crate::import_cmd::usage()))
}
