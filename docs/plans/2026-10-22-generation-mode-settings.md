# 생성 방식 설정 토글 — 단일 패스 ↔ 2단계 스트리밍

2026-10-22 · 요청: "`/sdd` 를 활용해서 만들어줘. 다른 이런 앱들은 어떻게 이루어져 있는지 연구하고
이에 대해서 잘 개발해주면 좋을꺼 같아."

## 연구 (SDD discovery)

경쟁/유사 앱 조사 — AI Dungeon(help.aidungeon.com, Wikipedia), SillyTavern 문서,
Character Card Spec V2, Rust TUI 계열(tenere, aichat):

1. **모든 경쟁 앱은 모델 출력을 기본으로 스트리밍한다** — AI Dungeon(실시간 서사),
   SillyTavern(스트리밍 + World Info), tenere(스트리밍 + Ctrl+T 중단), aichat(REPL 스트리밍).
   설정 파일을 손으로 고쳐야 스트리밍이 켜지는 앱은 없다.
2. **모델/스테이지 선택은 설정 화면의 일급 시민** — tenere·aichat 모두 UI에서 백엔드/모델 전환.
3. StoryTui의 메모리 티어·로어북(sticky/cooldown/delay, vectorized 폴백)은 경쟁 앱 대비 이미 우위 —
   이번 사이클 범위 밖.

저장소 갭: W10에서 2단계 Director→Writer 스트리밍이 프로바이더(SSE)→호스트(청크 전달)→TUI
(`streaming_narration` 렌더)까지 전부 구현돼 있지만 `use_director_writer`가 maintainer-only
TOML 플래그(ADR-005 기본 `false`)라 플레이어가 스트리밍을 경험할 길이 없었다.
`KNOWN-LIMITATIONS.md`도 "the settings UI does not yet expose it"을 명시.

## 목표와 범위

- **달라지는 결과**:
  1. 설정 화면에서 `g`(한글 ㅎ)로 생성 방식을 전환할 수 있다 — 단일 패스 ↔ 2단계(기획→본문 스트리밍).
  2. 선택은 `saves/user-settings.toml`에 영속되고 다음 턴부터 즉시 적용된다.
  3. 설정 화면 상태 블록과 키맵 표에 생성 방식이 표시된다.
- **제외**: 단일 패스 경로의 스트리밍(구조화 JSON 한 번의 호출이라 서사만 스트리밍 불가 — 2단계가
  정답 경로), ADR-005 기본값 변경(실모델 20턴 A/B 게이트가 전제).
- **호환 근거**: 엔진은 Submit 커맨드 시점에만 플래그를 읽고(`engine.rs:281,288,414,604`)
  Accept 커맨드는 플래그를 참조하지 않으므로 전환 중 진행 생성은 원래 경로로 완료된다.
  설정 진입 시 호스트가 진행 생성을 취소한다(`host.rs`). 복원·스왑·새 게임은 모두
  `config.use_director_writer`를 읽으므로 세션 값 갱신만으로 전부 일관된다.

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | 설정에서 `g`/`ㅎ`를 누르면 생성 방식 토글이 요청되고 다음 턴부터 반영된다 | T-2, T-4 | V-1, V-2 |
| AC-2 | 토글 값이 user-settings.toml에 영속되고 재시작 시 기본 config보다 우선한다 | T-1, T-4 | V-2 |
| AC-3 | 설정 상태 블록에 `gen (g) : 단일 패스 / 2단계(...)`가 표시되고 키맵·푸터에 `g`가 안내된다 | T-3 | V-1 |
| AC-4 | 기존 흐름(단일 패스 기본, 복원·스왑·새 게임·모델 전환)에 회귀가 없다 | T-4 | V-3 |

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 태스크 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | UserSettings 영속 필드 (AC-2) | apps/storytui/src/config.rs | 없음 | V-2 | done |
| T-2 | App 상태·`g` 키·게터/패치 메서드 (AC-1) | crates/story-tui/src/app.rs | 없음 | V-1 | done |
| T-3 | 설정 렌더·키맵·푸터 (AC-3) | crates/story-tui/src/screens/{settings.rs,keymap.rs} | T-2 | V-1 | done |
| T-4 | 호스트 연결 — 설정 로드 선행, 세션+엔진 동시 전환, 영속 (AC-1,2,4) | apps/storytui/src/host.rs, host/settings.rs | T-1, T-2 | V-2, V-3 | done |
| T-5 | 문서 — KNOWN-LIMITATIONS·UI-NAVIGATION·본 계획 문서 | docs/ | T-4 | — | done |

## 설계 메모

- **값의 흐름**: `user-settings.toml` → 호스트가 `config.use_director_writer`에 유효값 스탬프
  (엔진/복원/스왑/세이브가 전부 이 한 곳을 읽음) → 설정 표시는 App의 `SettingsState` 미러.
  `app.use_director_writer()` 게터로 refresh 경로가 config를 순회하지 않게 한다(images 패턴과 동일).
- **토글 처리**: `config`와 `engine` 플래그를 함께 뒤집고 user-settings에 영속 — 진행 생성은
  Accept 커맨드가 플래그를 읽지 않아 영향 없음. 설정 진입 시 취소되므로 mid-flight 위험 없음.
- **ADR-005 유지**: 엔진이 스스로 플래그를 뒤집지 않고, 기본값은 계속 `false`. 플레이어 명시 선택만
  세션/영속 값을 바꾼다.

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-tui` — `g` 키 바인딩(라틴/한글)·요청 소비·게터 패치·settings 스냅샷 2종 | repo root | 신규+기존 전부 통과 | lib 125/125 통과(신규 `settings_g_key_requests_the_generation_toggle_once`), 전체 타깃 ok, 스냅샷 2종 갱신 후 통과 — `gen (g) : 2단계(...)`/`단일 패스` 양분기 렌더 확인 (2026-10-22) | PASS |
| V-2 | `cargo test -p storytui` — UserSettings 라운드트립(부재→None 상속, Some(true) 보존) | repo root | 전부 통과 | bin 23 + lib 18 포함 전 스위트 ok (2026-10-22) | PASS |
| V-3 | `cargo test --workspace` + clippy(-D warnings) + fmt --check + check-core-deps + check-public-config | repo root | 회귀 0 | 63 스위트 전부 ok, clippy 경고 0, fmt 정렬, ADR-003·public config 양호 (2026-10-22) | PASS |

## 부수 수정 (기존 게이트 fail — 본 워크스트림과 무관하게 발화)

stable 전환 커밋(e748044) 이후 새 clippy(rust 1.99) lint가 기존 코드에서 발화해 워크스페이스
게이트가 깨져 있었다 (클린 HEAD에서 재현 확인). 게이트 회복을 위해 최소 수정:

- `story-provider/src/provider.rs` — async_trait 확장의 `double_must_use`를 트레이트 경계에서 allow.
- `story-import/src/card.rs` — 테스트의 `.err().expect()` → `expect_err` (`err_expect`).

## 평가와 인계

- 대화형 실단말 확인(실모델로 `g` 토글 → 본문 스트리밍 체감)은 터미널 앱 특성상 사용자 실행으로
  남긴다 — Mock은 `write_stream`이 UnsupportedCapability라 자동 검증 불가 (기존 컨벤션과 동일).
- 커밋은 사용자 요청 시에만.
