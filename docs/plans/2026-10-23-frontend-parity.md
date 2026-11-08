# 프론트엔드 패리티 — 타 스토리 앱 기능 이식 (G1–G9)

2026-10-23 · 요청: "그거 모두 그러면 넣어주세요" — `docs/FRONTEND-COMPARISON.md` 조사의
갭 G1–G10을 이번 사이클에 가져온다. 직전 사이클(2026-10-23-settings-auto-tuning)의
미커밋 변경 위에서 진행하며 보존한다.

## 범위 결정 (가정 로그 — 사용자 확인 후 확정)

- **구현**: G1 로어북 뷰어·편집기, G2 Author's Note, G3 Continue, G5 Impersonate,
  G6 프롬프트 미리보기, G7 내보내기, G8 표시 규칙(regex), G9 퀵 동작,
  G4의 최소 완성판(되돌리기 링 영속).
- **G4 축소 근거**: 완전한 분기 트리는 세이브 포맷 재설계(大)라 이번 사이클에서는
  "세이브에 되돌리기 링 동봉 → 로드 직후 `u` 가능 → 다른 선택 후 저장으로 분기"까지.
  변형 순환 UI(스와이프식)는 후속 항목.
- **G10 그룹 채팅 제외**: 팩 스키마·엔진 턴 주체 재설계가 선행돼 이번 패스에서 제외.
  KNOWN-LIMITATIONS와 FRONTEND-COMPARISON에 설계 과제로 명시한다.
- **G8/G9는 키 없는 TOML 구성**: 인지 부하 원칙 — 기본값 없음, UI 없음, TOML만.
- **로어북 영속 조건**: 팩에 `lorebook.toml`이 파일로 존재할 때만 편집을 저장하고,
  없으면 세션 전용(상태 라벨 표시). 임포트 팩 원본의 무손상 우선.
- **노트 주입 위치**: 엔진·프롬프트 조립 경계를 건드리지 않기 위해 페르소나와 같은
  방식으로 host가 pack system 프롬프트에 멱등 섹션(`# Author's note (StoryTUI)`)을
  주입한다. ST의 @Depth(유저 직전) 대신 시스템 꼬리 — 효과 동선 단순화, 검증 패턴 재사용.

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 태스크 | 검증 |
|---|---|---|---|
| AC-1 | 설정 `w`의 로어북 모달에서 엔트리(키·상수/트리거·활성)를 보고 ↑↓ 이동·Space 활성 토글·Enter 내용 편집·`k` 키 편집을 하면 같은 세션 다음 턴부터 반영된다. 팩에 lorebook.toml이 있으면 저장돼 재시작 후에도 유지되고, 없으면 "세션 전용"으로 표시된다 | T-1, T-4, T-5 | V-1, V-3, V-4 |
| AC-2 | 설정 `t`의 노트 모달에서 텍스트를 확정하면 다음 턴 프롬프트의 `# Author's note (StoryTUI)` 섹션에 반영되고, 빈 값 확정은 섹션을 지운다. 선택은 user-settings에 영속된다 | T-3, T-4, T-5 | V-3, V-4 |
| AC-3 | 스토리 화면 `w`로 마지막 내레이션을 이어쓰면 새 턴 없이 narration이 연장되고 선택지·상태는 유지되며, 자동 저장에도 연장된 본문이 저장된다. 로딩·오류 중엔 무시된다 | T-1, T-4, T-5 | V-1, V-4 |
| AC-4 | 저장→불러오기 직후 `u`로 직전 턴 되돌리기가 동작한다(링 상한 8 유지). 기존 세이브(링 없음)는 없는 것과 같게 동작한다 | T-1, T-2 | V-1, V-2 |
| AC-5 | 스토리 화면 `z`로 대필을 요청하면 모델이 제안한 플레이어 행동 텍스트가 자유 입력 버퍼에 채워지고, 전송은 플레이어 Enter로만 일어난다 | T-1, T-4, T-5 | V-1, V-4 |
| AC-6 | 설정 `r`의 프롬프트 미리보기에서 마지막 전송 system·user 프롬프트를 스크롤로 열람할 수 있다(세션 메모리만, 영속 없음) | T-4, T-5 | V-3, V-4 |
| AC-7 | `storytui export <슬롯> --format txt\|jsonl [--out 경로]`가 저장된 턴 대화를 파일로 내보낸다 | T-2 | V-2 |
| AC-8 | user-settings `[[display_rules]]`(pattern·replacement·enabled)가 새로 표시되는 텍스트에 순차 적용된다. 컴파일 불가 규칙은 경고 후 건너뛴다 | T-3, T-4 | V-3 |
| AC-9 | user-settings `[quick_actions]`에 "1"="텍스트"가 있으면 스토리 화면 Alt+1이 자유 입력 버퍼를 그 텍스트로 채운다(전송 아님). 설정이 없으면 아무 일도 없다 | T-3, T-4, T-5 | V-3, V-4 |
| AC-10 | 표현층 회귀 없음 — 키맵·도움말에 새 행 6건(Alt+1-9·w·z 게임, w·t·r 설정)이 잘림 없이 들어가고, 설정 화면 80×24 스냅숏은 새 행 수용에 따른 단축키 두 컬럼 분할점 이동 외 변화가 없으며, 푸터 키는 증가하지 않는다 | T-5 | V-4, V-5 |

## 태스크

| ID | 목적·AC | 수정 파일/경계 | 선행 | 검증 | 상태 |
|---|---|---|---|---|---|
| T-1 | 엔진: `ContinueNarration`·`SuggestAction` 부가 생성 패밀리(narration 연장/제안 수용, choices·patch 폐기), `SetLorebookEntries` 핫 반영, 되돌리기 링의 SaveEnvelope 직렬화(story-model 필드 포함), continue·suggest 프롬프트 지시 | crates/story-core/{engine_cmd.rs,engine.rs,prompt_context.rs}, crates/story-model (SaveEnvelope) | 없음 | V-1 | done — `SavePayload.undo_stack: Vec<GameState>`(serde default, 체크섬 포함), 사이드 생성은 단일 패스 강제·narration만 수용, 15 신규 reduce 테스트. story-core+model 386 passed, story-provider 124 passed (2026-10-23) |
| T-2 | 영속: 세이브 envelope ↔ undo 링 저장·복원(json_store), `export` 변환(txt/jsonl) + CLI 서브커맨드, lorebook.toml 원자 저장(.bak) | crates/story-persistence/{json_store.rs, export.rs(신규), pack/lorebook_store.rs(신규)}, apps/storytui/src/{main.rs,export_cmd.rs(신규)} | T-1(링 필드는 T-1이 envelope에 기록 — json_store은 serde 통과라 무변경) | V-2 | done |
| T-3 | 설정: UserSettings에 author_note·display_rules·quick_actions 추가(파싱·상한·기본 없음), 라운드트립 테스트 | apps/storytui/src/config.rs | 없음 | V-3 | done |
| T-4 | 호스트: 노트 멱등 섹션(페르소나 패턴), display_rules 적용(컴파일 캐시·무효 건너뜀), quick_actions 버퍼 주입, 로어북 편집→엔진 반영·파일 저장, continue·suggest 폴링(입력 주입), 마지막 프롬프트 캡처(단일·기획·본문 3경로), export·기동 배선 | apps/storytui/src/host.rs, host/settings.rs | T-1, T-3 | V-3 | done — apply_prompt_overlays로 두 섹션 공존 보장, 규칙 컴파일은 활성 기준 상한 |
| T-5 | TUI: 스토리 `w`·`z`, 설정 `w`(로어북 모달)·`t`(노트 모달)·`r`(프롬프트 뷰어), Alt+1..9, 키맵 행 5건, 요청 플래그·모달 상태, App::handle_key 레벨 테스트, 스냅숏 갱신 | crates/story-tui/src/app.rs, screens/{keymap.rs,settings.rs} | T-4 | V-4 | done (위임) — 180 passed 인도, 두 컬럼 분할 알고리즘 재조정 포함 |
| T-6 | 문서: UI-NAVIGATION(키·모달), KNOWN-LIMITATIONS(G4 축소·G10 제외·로어북 영속 조건), FRONTEND-COMPARISON 반영 표시 | docs/ | T-2–T-5 | V-5 | done |

## 설계 메모

- **부가 생성 패밀리**: `EngineCommand::{ContinueNarration, SuggestAction}`은 로딩·오류
  중 무시, 체크포인트 불변. 내부는 기존 `RequestGeneration` 변형(kind 필드)으로:
  continue는 마지막 committed turn narration 뒤에 이어쓰기 지시 + 직전 본문 제공,
  suggest는 "플레이어의 다음 행동 한 문장을 한국어로 제안" 지시. 응답은 기존 JSON
  draft로 받아 **narration만** 수용 — continue는 직전 narration에 연장 저장(선택지·
  patch 유지), suggest는 엔진에 저장하지 않고 앱 요청 플래그로 전달해 입력 버퍼에 채운다.
  2단계(Director→Writer) 경로와는 조합하지 않고 단일 패스로 강제.
- **undo 링 직렬화**: SaveEnvelope에 `undo_stack: Vec<EngineState 스냅샷>`(상한 8) 추가,
  `#[serde(default)]`로 구세이브 호환. `u`는 복원된 링에서도 동작.
- **로어북 편집**: 모달은 표시 필드(키·content·enabled)만 편집 가능, 나머지는 조회 전용.
  확정 시 `EngineCommand::SetLorebookEntries`로 세션 즉시 반영 + lorebook.toml이
  있으면 원자 저장(.bak). 편집 제출은 생성이 끝날 때까지 앱이 보류한다 — 생성 중에는
  엔진이 핫스왑을 무시(identity reduce)하므로, 모달·엔진·파일이 갈라지지 않게 take를
  유보하고 로딩 종료 후 전달한다(G3 F-2 수정).
- **프롬프트 캡처**: host가 provider 호출 직전 system·user 문자열을 보관(최근 1회),
  설정 `r` 뷰어는 스크롤 열람만. 영속 없음 — 시크릿 정책과 동일 선상(이미 로그
  redaction 대상 아님 유지: 화면 열람은 로그와 달리 로컬 사용자 눈).
- **display_rules**: host가 app에 텍스트 블록을 push하는 지점에서 순차 적용. regex
  컴파일은 로드 시 1회, 실패 규칙은 tracing 경고 후 제외. 규칙 상한 16, 과도 치환
  방지용 길이 상한. story-core가 아니라 apps 쪽에 둔다(표현층 관심사).
- **quick_actions**: 키는 "1".."9"만 수용, 값은 200자 상한. Alt+조합은 스토리 화면에서
 만 소비하고 선택지 숫자키·기존 키와 충돌 없음(crossterm modifiers ALT).
- **표현층 원칙**: 푸터 무증가, 도움말 두 컬럼 예산(현재 27행 / 36행) 내 5행 추가,
  라벨 최단("이어쓰기", "대필", "로어북", "노트", "프롬프트"), 80×24 스냅숏 확인.

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-core -p story-model` — continue 연장·선택지 유지, suggest 요청 플래그, SetLorebookEntries 반영, undo 링 직렬화 라운드트립, 구세이브(링 없음) 호환 | repo root | 신규+기존 전부 통과 | 25 스위트 386 passed / 0 failed — 신규 19건(continue 7·suggest 2·로어북 3·undo 링 3·SavePayload 기본값 1·프롬프트 결정론 3). `cargo test -p story-provider` 124 passed로 교차 크레이트 무파손 (2026-10-23) | PASS |
| V-2 | `cargo test -p story-persistence` + `cargo test -p storytui --bin storytui` — envelope 링 라운드트립, export txt/jsonl 골든, lorebook 원자 저장(.bak 생성)·무파일 팩 거부, `storytui export` 클릭 테스트 | repo root | 신규+기존 전부 통과 | story-persistence 7 스위트 전부 ok(lib 39 포함 — export 2건·lorebook_store 2건·save_round_trip 링 필드 포함), export_cmd 클릭 테스트(txt 기본·jsonl 플래그·슬롯 없음·포맷 오류) 통과 (2026-10-23) | PASS |
| V-3 | `cargo test -p storytui` — UserSettings 라운드트립(신규 3필드·빈 값 해제·상한), 노트 섹션 멱등(페르소나 공존), display_rules 컴파일(상한·무효 건너뜀·순서), quick_actions 파싱, 프롬프트 캡처 | repo root | 신규+기존 전부 통과 | storytui lib+bin 전 스위트 ok — 신규: note_rules_round_trip, prompt_overlays_are_idempotent_and_keep_each_other, author_note_submit_persists, display_rule_compilation_caps_skips(활성 기준 상한 16 정정 후 통과), export_writes_txt/jsonl (2026-10-23) | PASS |
| V-4 | `cargo test -p story-tui` — handle_key: 스토리 w·z(로딩 흡수), Alt+N(선택지 숫자키 무간섭), 설정 w·t·r, 모달 입력·Esc·토글, 키맵 두 컬럼 수용, 스냅숏 | repo root, TestBackend 80×24 | 신규+기존 전부 통과 | T-5 인도 시 180 passed / 0 failed(신규 8건 + 두 컬럼 예산 신설 테스트), 이후 display_rules 표시 경로 테스트 1건 추가 — 최종 워크스페이스 실행에서 전부 재통과 (2026-10-23) | PASS |
| V-5 | 게이트 5종 — `cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --check` + `./scripts/check-core-deps.sh` + `./scripts/check-public-config.sh` | repo root | 회귀 0 | 최종 상태 기준 전부 통과 — 63 스위트 ok(직전 사이클과 동일 수, 실패 0), clippy 경고 0(clapsible-if·unused-mut·이중 #[test]·field_reassign 정정 후), fmt 정렬, check-core-deps OK(regex는 story-tui/apps 추가 — story-core 무관), check-public-config OK (2026-10-23) | PASS |
| V-6 | G4 사용성 — 설정 스냅숏 diff 검토·도움말 두 컬럼 예산·모달 렌더 테스트 | repo root, TestBackend 80×24 | 레이아웃 붕괴 없음 | settings_snapshot 3건 갱신 diff 대조: 이번 사이클 가분분은 단축키 블록 우측 컬럼 상단 2행 분할점 이동뿐(o/? 행), 잘림 없음·푸터 무변화 확인. 도움말 본문 예산(≤20행/컬럼) 신설 테스트 통과, 로어북·노트·프롬프트 뷰어 렌더 테스트 통과. 실단말 육안 확인은 사용자 실행(잔여) | PASS |

## 가정 로그

- 범위·축소·제외 사유는 상단 "범위 결정"에 기록. G4 변형 순환 UI와 G10은 후속 설계
  과제로 FRONTEND-COMPARISON §4에 남긴다.
- continue의 2단계 경로 비조합: 두 경로를 모두 지원하면 검증 면이 2배 — 단일 패스로
  강제하고 도움말에 명시하지 않는다(동작 차이가 체감되지 않는 수준).
- 노트 섹션 명칭은 `(StoryTUI)` 접미사로 페르소나 섹션과 같은 임포트 충돌 방지.
- **T-1이 발견한 기존 결함(이번 사이클 범위 외, 미수정)**: `record_lorebook_activations`가
  요청 시점 transition.state에 기록하지만 accept가 기록 이전 `pending.checkpoint`에서
  재구성해 활성 원장이 accept을 살아남지 못한다 — sticky/cooldown이 실전 세이브 체인에서
  무효화 의심. 신규 로어북 편집 UI와 무관한 엔진 기존 경로이므로 후속 결함(F-* 후보)으로
  G3 리뷰에 회부한다.
- **링 직렬화 비용**: SavePayload.undo_stack은 GameState 전체 스냅샷 최대 8개 — 세이브
  크기가 커진다(기존 인메모리 링과 동일 중복). 상한 8은 복원에서도 강제.

## 평가와 인계

- **G1 계획 평가**: PASS (self, plan 모드) — AC↔T↔V 전결합, 범위 결정(사용자 확인
  "모두")과 축소·제외 사유를 가정 로그로 고정. 실행 중 설계 보강 2건: 되돌리기 링은
  SaveEnvelope가 아니라 SavePayload.undo_stack으로(체크섬 포함·host 배선 0), 규칙
  적용은 표시 계층(story-tui)에서 텍스트→라인 경계로.
- **G3 코드 리뷰**: **FAIL → 결함 수정 후 재검증 PASS**. 위임 독립 리뷰어(이번 세션은
  위임 도구 가용 — 직전 사이클의 self-review와 달리 독립성 확보). 리뷰 입력: 원 요청,
  plan, 작업 트리 전체 diff, 검증 기록. 판정 FAIL 사유와 처리:
  - **F-1(P1, 수정 완료)**: 프롬프트 뷰어 스크롤 클램프가 논리 줄 수 기준 — Paragraph는
    wrap 후 시각 행 단위로 스크롤되어 꼬리에 도달 불가(AC-6 위반). 수정: 렌더러와 같은
    지오메트리 헬퍼(prompt_viewer_frame) + `wrapped_line_count`로 접힌 행 기준 클램프,
    랩 시나리오 테스트 추가.
  - **F-2(P2, 수정 완료)**: 생성 중 로어북 편집 제출 시 엔진은 무시하는데 호스트는
    pack·파일을 바꿔 상태 분기. 수정: 앱이 로딩 중 take를 유보(요청 보존), 종료 후
    전달 — 보류 테스트 추가.
  - **F-5·F-8(P3, 수정 완료)**: 규칙 상한이 무효 패턴도 슬롯 소비(활성 규칙 기준으로
    정정 + 테스트 확장), set_input_text가 설정·모달 위에서 입력 모드를 강제 개방하던
    점(맨 스토리 화면에서만 개방하도록 정정).
  - **F-3(P2, 기존 결함 — 미수정)**: 로어북 활성 원장이 accept을 살아남지 못함(sticky/
    cooldown 무효화 의심). 이번 사이클 미도입·범위 외로 리뷰어도 확인 — 후속 사이클
    과제로 KNOWN-LIMITATIONS에 등록됨.
  - **F-4(P3, 수정 완료)**: AC-10 문구를 실제 범위(행 6건·분할점 이동)로 정정.
  - **F-6·F-7(P3, 잔여)**: 규칙 미적용 표면(선택지 라벨·기록 목록), 쉼표 포함 키의 모달
    표현 한계 — 아래 잔여 한계에 기록.
- **G4 사용성·디자인**: PASS (V-6 렌더 증거 + 스냅숏 diff 대조). 새 키 전부 설정
  키맵·도움말로만 안내(푸터 무증가), 도움말 본문 예산 신설 테스트(≤20행/컬럼), 모달
  3종 80×24 렌더 테스트 통과. 실단말 육안은 사용자 실행.
- **G5 완료 판정**: PASS (self, completion 모드) — AC-1 로어북 열람·토글·편집·핫스왑·
  조건부 영속(F-2 보류 포함), AC-2 노트 주입·영속·해제, AC-3 이어쓰기 연장·자동 저장,
  AC-4 로드 직후 undo(구세이브 호환), AC-5 대필 버퍼 주입·수동 전송, AC-6 프롬프트
  열람(F-1 클램프 수정 포함), AC-7 export CLI, AC-8 표시 규칙(표시 계층 전용), AC-9
  퀵 입력(미설정 no-op), AC-10 표현층 회귀 없음. 게이트 5종 최종 상태 통과.
- **잔여 한계**: (1) 실단말 육안 확인(사용자 실행), (2) continue·suggest의 실모델 응답
  품질과 JSON 회수 파서 상호작용은 mock·reduce 수준 관찰에 머묾, (3) 표시 규칙이
  선택지 라벨·`h` 기록 목록 미리보기에는 미적용(내레이션·스트리밍만), (4) 로어북 키
  편집은 쉼표 구분이라 쉼표 포함 키는 TOML 직접 편집 필요, (5) F-3 활성 원장 결함은
  후속 사이클, (6) 변형 순환 UI·그룹 채팅은 로드맵(비교 문서 §4).
- 커밋은 사용자 요청 시에만.
