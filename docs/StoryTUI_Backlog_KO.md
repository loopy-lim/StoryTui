# StoryTUI 개발 백로그

문서 버전: 1.0-draft  
티켓 수: 36  
정렬 원칙: 선행 작업을 만족하는 실행 순서  
우선순위: P0 필수 기반, P1 릴리스 필수, P2 품질 향상, P3 이후 검토

## Epic E0 - 기술 검증과 작업 기반

### ST-001 Rust workspace와 품질 기준선

- Feature: Build foundation
- Task: 2024 edition workspace, `rust-toolchain.toml`, 공통 lint, formatter와 test 명령을 구성한다.
- 우선순위: P0
- 선행 작업: 없음
- Acceptance criteria:
  - `apps/storytui`와 5개 library crate가 workspace member다.
  - `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`가 실행된다.
  - `story-core` manifest에 UI·HTTP 의존성이 없다.
- 테스트: 빈 workspace CI smoke, dependency tree 검사.

### ST-002 TerminalGuard와 비동기 TUI spike

- Feature: TUI runtime
- Task: Ratatui/Crossterm mock 화면, key·resize·tick channel, panic 복원을 구현한다.
- 우선순위: P0
- 선행 작업: ST-001
- Acceptance criteria:
  - 80x24에서 제목·본문·선택지·상태줄이 겹치지 않는다.
  - 3초짜리 background task 중 키와 resize가 처리된다.
  - 정상 종료와 의도적 panic 후 raw mode·cursor·alternate screen 복원을 시도한다.
- 테스트: TUI snapshot, 수동 터미널 복원 체크.

### ST-003 초기 MLX 서버 호환성 spike

- Feature: Local inference
- Task: 후보 모델을 `mlx_vlm.server`에서 우선 로드하고 health·chat completions·JSON을 검증한다.
- 우선순위: P0
- 선행 작업: 없음
- Acceptance criteria:
  - 실제 사용한 Python/MLX-VLM/모델 revision과 실행 명령이 기록된다.
  - 한국어 text-only 요청 10회의 성공·JSON 유효률이 기록된다.
  - 실패 시 대체 MLX 모델과 실패 원인이 구분된다.
- 테스트: `/health`, `/v1/models`, non-streaming 10회, 서버 중단.

### ST-004 한글 IME·폭 검증 harness

- Feature: Korean terminal UX
- Task: grapheme/width fixture와 Terminal.app·iTerm2 수동 체크리스트를 만든다.
- 우선순위: P0
- 선행 작업: ST-002
- Acceptance criteria:
  - 한글 조합 확정, 중간 삽입, 삭제, 붙여넣기, emoji 혼합을 검사한다.
  - 계산된 줄 폭이 Ratatui 영역을 넘지 않는다.
  - 두 터미널의 차이와 우회 방법이 문서화된다.
- 테스트: unicode fixture unit, 두 터미널 수동 테스트.

### ST-005 M1 Max 성능 기준 측정

- Feature: Performance baseline
- Task: 4K·8K·16K context에서 cold/warm 지연과 메모리를 측정한다.
- 우선순위: P0
- 선행 작업: ST-003
- Acceptance criteria:
  - load time, TTFT, total latency, tokens/s, peak memory가 JSON/CSV로 남는다.
  - 측정 도구와 3회 반복의 중앙값을 명시한다.
  - MVP context budget과 timeout의 잠정값이 결정된다.
- 테스트: 결과 schema validation, 동일 preset 재실행.

## Epic E1 - Story Model과 Core

### ST-006 공통 ID·enum·GameState 타입

- Feature: Domain model
- Task: ID newtype, `GameState`, `CharacterState`, `Choice`, `HistoryEntry`, `MemoryEntry`를 구현한다.
- 우선순위: P0
- 선행 작업: ST-001
- Acceptance criteria:
  - Serde round-trip이 정보 손실 없이 통과한다.
  - unknown 필드와 기본값 정책이 타입별로 명시된다.
  - 문자열 ID를 서로 다른 종류로 실수 전달할 수 없다.
- 테스트: round-trip, golden JSON, compile-fail 또는 type-level 확인.

### ST-007 StatePatch 스키마와 구조 검증

- Feature: State mutation
- Task: `StatePatch`와 구조·참조·범위 검증 단계를 구현한다.
- 우선순위: P0
- 선행 작업: ST-006
- Acceptance criteria:
  - affection/trust/tension 일반 delta -3~+3을 검사한다.
  - 없는 character/location/flag/item을 code와 함께 거부한다.
  - 5개 초과 선택지, 빈 narration과 invalid enum을 거부한다.
- 테스트: 경계값 table test, snapshot error.

### ST-008 원자 reducer와 StateChangeLog

- Feature: State mutation
- Task: dry-run 후 전체 commit하는 reducer와 audit log를 구현한다.
- 우선순위: P0
- 선행 작업: ST-007
- Acceptance criteria:
  - patch의 마지막 연산이 실패해도 앞 변경이 적용되지 않는다.
  - 성공 시 revision이 정확히 1 증가한다.
  - before/proposed/applied/decision이 change log에 남는다.
- 테스트: rollback fault cases, 동일 입력 결정성 100회.

### ST-009 조건식 DSL과 event evaluator

- Feature: Rules engine
- Task: 부작용 없는 `ConditionExpr`와 event condition/effect 평가기를 구현한다.
- 우선순위: P0
- 선행 작업: ST-006, ST-008
- Acceptance criteria:
  - All/Any/Not, flag, number, 관계 수치, event, location, turn을 지원한다.
  - 같은 priority의 event는 ID 정렬로 결정적이다.
  - 존재하지 않는 참조는 실행 시점이 아니라 Pack validation에서 발견 가능하다.
- 테스트: truth table, nested condition, deterministic order.

### ST-010 route·relationship·ending 상태 머신

- Feature: Narrative governance
- Task: 핵심 route와 ending을 모델 patch에서 분리해 규칙 엔진이 판정하게 한다.
- 우선순위: P0
- 선행 작업: ST-009
- Acceptance criteria:
  - 모델이 ending을 직접 설정할 필드가 없다.
  - 조건 미충족 route 제안은 상태를 바꾸지 않는다.
  - 관계 단계 건너뛰기는 명시 event만 허용한다.
- 테스트: route/ending truth table, forbidden transition.

## Epic E2 - Story Pack

### ST-011 Story Pack manifest와 로더

- Feature: Pack loading
- Task: `story.toml`, `characters.toml`, `rules.toml`, `events.toml`, Markdown을 로드한다.
- 우선순위: P0
- 선행 작업: ST-006, ST-009
- Acceptance criteria:
  - UTF-8 파일을 canonical `StoryPack`으로 변환한다.
  - 누락 파일·TOML 문법 오류에 파일·필드·가능한 위치가 포함된다.
  - Pack root 밖 경로 참조를 거부한다.
- 테스트: good fixture, missing file, malformed TOML, path traversal.

### ST-012 Pack schema version·migration 정책

- Feature: Pack compatibility
- Task: semver 호환 검사와 same-major in-memory default migration을 구현한다.
- 우선순위: P0
- 선행 작업: ST-011
- Acceptance criteria:
  - 지원 major와 다른 Pack은 시작 전에 거부한다.
  - 같은 major의 누락 optional 필드는 명시된 default로 변환된다.
  - 원본 Pack을 자동 수정하지 않는다.
- 테스트: old minor, current, future major fixtures.

### ST-013 Pack 참조 무결성·도달 가능성 검증

- Feature: Pack validation
- Task: ID registry, 조건 참조, 장소 전이, event/route 도달 가능성을 검사한다.
- 우선순위: P0
- 선행 작업: ST-011
- Acceptance criteria:
  - 중복 ID와 없는 참조를 모두 수집해 한 번에 보여준다.
  - 시작점에서 도달 불가한 필수 event를 경고한다.
  - 장소 adjacency 없는 이동을 검출한다.
- 테스트: invalid reference matrix, unreachable fixture.

### ST-014 Story Pack 지원 등급 검증

- Feature: Content metadata
- Task: Pack 등급이 `general` 또는 `teen`인지 검증한다.
- 우선순위: P1
- 선행 작업: ST-011
- Acceptance criteria:
  - 지원 등급 두 종류는 정상 로드한다.
  - 그 밖의 등급은 시작 전에 명확한 오류로 거부한다.
  - 예제 Pack과 기본 프롬프트는 전체 이용가 표현 범위를 지킨다.
- 테스트: `general`, `teen`, 지원하지 않는 등급.

### ST-015 예제 `rainy-reunion` Pack

- Feature: Example content
- Task: 1캐릭터·3장소·5플래그 이하의 한국어 Pack을 작성한다.
- 우선순위: P0
- 선행 작업: ST-011~014
- Acceptance criteria:
  - validator error와 warning이 0이다.
  - 오프닝, 인물 말투·지식, 규칙, prompt 3종을 포함한다.
  - 첫 Mock 응답이 reducer를 통과한다.
- 테스트: pack validate, golden prompt, first-turn reducer.

## Epic E3 - Provider와 MVP 수직 슬라이스

### ST-016 LlmProvider와 MockProvider

- Feature: Provider abstraction
- Task: capability, health, generation, cancel 계약과 fixture queue Mock을 구현한다.
- 우선순위: P0
- 선행 작업: ST-006
- Acceptance criteria:
  - Mock은 성공, 지연, malformed, timeout, late result를 주입한다.
  - core test가 Reqwest와 네트워크 없이 실행된다.
  - unsupported Director/Writer capability를 명시적으로 반환한다.
- 테스트: contract test suite.

### ST-017 OpenAI-compatible MLX Provider

- Feature: Local inference
- Task: Reqwest client, 요청 설정, health와 chat completions adapter를 구현한다.
- 우선순위: P0
- 선행 작업: ST-003, ST-016
- Acceptance criteria:
  - base URL, model, sampling, timeout이 TOML에서 적용된다.
  - redirect를 기본 거부하고 non-loopback을 경고한다.
  - `mlx_vlm.server` 실제 smoke를 통과한다.
- 테스트: HTTP fixture server, 실제 MLX ignored test.

### ST-018 PromptContext와 단일 호출 prompt

- Feature: Prompt assembly
- Task: Pack·상태·최근 행동을 안정 순서로 직렬화해 MVP prompt를 만든다.
- 우선순위: P0
- 선행 작업: ST-015, ST-016
- Acceptance criteria:
  - 같은 입력의 prompt hash가 동일하다.
  - 최종 본문은 한국어 직접 생성하도록 명시한다.
  - 허용 ID·delta·선택지 수와 플레이어 주권 규칙이 포함된다.
- 테스트: golden prompt snapshot, ordering test.

### ST-019 GeneratedTurn JSON parser와 복구

- Feature: Structured generation
- Task: 엄격 파싱, fence 제거, 안전한 경미 수정, repair/full retry budget을 구현한다.
- 우선순위: P0
- 선행 작업: ST-007, ST-017
- Acceptance criteria:
  - 복구 단계와 시도 횟수가 상한을 넘지 않는다.
  - unknown field, invalid enum, 과다 선택지를 임의로 버리지 않는다.
  - 실패 시 원 응답 전체를 기본 로그에 남기지 않는다.
- 테스트: malformed fixture 12종, retry count, redaction.

### ST-020 generation coordinator와 취소

- Feature: Async orchestration
- Task: generation ID, background task, cancellation token, late response 폐기를 구현한다.
- 우선순위: P0
- 선행 작업: ST-002, ST-016
- Acceptance criteria:
  - 생성 중 UI event가 처리된다.
  - 취소 후 도착한 결과가 state/history를 변경하지 않는다.
  - 동시에 두 generation을 commit할 수 없다.
- 테스트: delayed Mock race, cancel, duplicate finish.

### ST-021 Story 화면 선택 수직 슬라이스

- Feature: MVP gameplay
- Task: 새 게임→첫 장면→3개 선택→다음 장면을 TUI와 core에 연결한다.
- 우선순위: P0
- 선행 작업: ST-008, ST-015, ST-018~020
- Acceptance criteria:
  - 선택 직전 checkpoint가 생긴다.
  - 검증된 patch만 commit되고 본문·choice가 표시된다.
  - 실제 모델로 10턴을 완료할 수 있다.
- 테스트: Mock 3턴 E2E, 실제 10턴 smoke.

## Epic E4 - 자유 입력과 History

### ST-022 grapheme 기반 자유 입력기

- Feature: Free action
- Task: 다중 줄 buffer, cursor, delete, paste와 제출/취소를 구현한다.
- 우선순위: P1
- 선행 작업: ST-004, ST-021
- Acceptance criteria:
  - 한글 grapheme 중간을 잘라 삭제하지 않는다.
  - 제어 문자와 최대 길이를 검사한다.
  - 취소 시 입력이 state/history에 남지 않는다.
- 테스트: unicode unit, TUI snapshot, 두 터미널 manual.

### ST-023 규칙 기반 ParsedAction

- Feature: Free action
- Task: known ID·대사·행동·이동을 보조 구조화하고 원문 fallback을 구현한다.
- 우선순위: P1
- 선행 작업: ST-015, ST-022
- Acceptance criteria:
  - 성공·실패 모두 `original_text`를 보존한다.
  - 낮은 확신은 `general_action`으로 처리한다.
  - 모델이 플레이어 의도를 확정·왜곡하지 않도록 prompt contract가 갱신된다.
- 테스트: 한국어 입력 fixture 20개, fallback.

### ST-024 History·재생성·체크포인트

- Feature: Backlog and recovery
- Task: 확정 턴 기록, generation lineage, 재생성과 이전 안정 상태 복구를 구현한다.
- 우선순위: P1
- 선행 작업: ST-020, ST-021, ST-023
- Acceptance criteria:
  - 기본 History는 확정 generation만 보여준다.
  - 재생성 실패가 기존 확정 턴을 덮지 않는다.
  - 선택/자유 입력과 적용 patch를 한 entry에서 추적한다.
- 테스트: regenerate success/failure/cancel, snapshot.

## Epic E5 - Persistence

### ST-025 SaveEnvelope·checksum·round-trip

- Feature: JSON save
- Task: 세이브 envelope와 canonical payload checksum을 구현한다.
- 우선순위: P1
- 선행 작업: ST-006, ST-024
- Acceptance criteria:
  - stable state, memory, history, audit tail을 복원한다.
  - prompt/API secret/불완전 buffer는 포함하지 않는다.
  - serialize→load 후 도메인 동등성이 유지된다.
- 테스트: golden save, 100회 round-trip.

### ST-026 원자 쓰기와 backup

- Feature: Durable save
- Task: same-directory temp, flush/sync, rename, `.bak`을 구현한다.
- 우선순위: P1
- 선행 작업: ST-025
- Acceptance criteria:
  - 각 쓰기 단계 실패 시 기존 save가 유효하다.
  - 완료 후 재열기와 checksum 검증을 통과한다.
  - 불완전 generation 중 stable state만 쓴다.
- 테스트: fault injection, process kill 시나리오.

### ST-027 Save migration과 손상 복구

- Feature: Save compatibility
- Task: 순차 migration, future major 거부, backup 복구를 구현한다.
- 우선순위: P1
- 선행 작업: ST-025, ST-026
- Acceptance criteria:
  - v0 fixture를 v1 in-memory state로 로드한다.
  - checksum mismatch와 잘린 JSON이 현재 game을 바꾸지 않는다.
  - `.bak` 사용 전에 사용자에게 출처와 시각을 보여준다.
- 테스트: old/future/corrupt fixtures.

### ST-028 Save/Load 슬롯 화면

- Feature: Save UX
- Task: autosave, quick save, 수동 3슬롯, 설명·복제·삭제 UI를 구현한다.
- 우선순위: P1
- 선행 작업: ST-026, ST-027
- Acceptance criteria:
  - 챕터·장면·시각·설명을 표시한다.
  - delete와 overwrite는 확인을 요구한다.
  - load는 임시 검증 후 활성 state를 교체한다.
- 테스트: TUI snapshot, collision, delete confirmation.

## Epic E6 - 장기 기억

### ST-029 MemoryEntry·사건 기억

- Feature: Hierarchical memory
- Task: facts/interpretation/known_by/source IDs와 규칙 기반 사건 기억을 구현한다.
- 우선순위: P1
- 선행 작업: ST-009, ST-025
- Acceptance criteria:
  - 약속·비밀 공개·관계 단계 변화·중요 아이템을 deterministic하게 생성한다.
  - 사실과 해석을 분리한다.
  - source history ID 없는 모델 fact를 확정하지 않는다.
- 테스트: event-to-memory fixture, hallucinated fact rejection.

### ST-030 장면·챕터 요약 계약

- Feature: Summarization
- Task: facts, unresolved, relationship cause, knowledge, address form, player action을 보존하는 요약을 구현한다.
- 우선순위: P1
- 선행 작업: ST-029
- Acceptance criteria:
  - 필수 필드 누락 시 요약을 교체하지 않는다.
  - 이전 요약을 덮기 전에 source coverage를 검사한다.
  - 호칭 변화 원인과 미해결 약속이 남는다.
- 테스트: loss fixtures, snapshot, actual model sample.

### ST-031 규칙 기반 MemorySelector

- Feature: Prompt memory
- Task: 절대 포함 규칙, relevance score와 token budget을 구현한다.
- 우선순위: P1
- 선행 작업: ST-018, ST-029, ST-030
- Acceptance criteria:
  - 현재 route·미해결 약속·참여자 호칭 규칙은 항상 포함된다.
  - budget을 넘지 않고 선택 trace를 제공한다.
  - 동일 state의 선택 결과가 결정적이다.
- 테스트: 20/50/100턴 synthetic contexts.

### ST-032 100턴 soak harness

- Feature: Long-run quality
- Task: Mock deterministic 100턴과 실제 모델 지정 시나리오 runner를 만든다.
- 우선순위: P1
- 선행 작업: ST-028, ST-031
- Acceptance criteria:
  - 매 턴 불변식·save/load·memory budget을 검사한다.
  - 20·50·100턴 report를 분리한다.
  - 실제 모델 실행은 ignored suite로 분리된다.
- 테스트: CI synthetic soak, local actual soak.

## Epic E7 - Director/Writer

### ST-033 DirectorPlan과 검증

- Feature: Two-stage generation
- Task: 장면 목표·beats·감정·공개/금지 정보·choices·patch·종료 조건 schema를 구현한다.
- 우선순위: P1
- 선행 작업: ST-031
- Acceptance criteria:
  - Director 응답은 비스트리밍 완전 JSON이다.
  - plan의 patch가 reducer dry-run을 통과해야 Writer로 간다.
  - 금지 정보와 knowledge scope 충돌을 검출한다.
- 테스트: invalid plans, repair/retry, actual Director smoke.

### ST-034 Writer SSE와 확정 commit

- Feature: Streaming prose
- Task: 확정 plan을 한국어 본문으로 스트리밍하고 완료 후 lint·history commit한다.
- 우선순위: P1
- 선행 작업: ST-020, ST-033
- Acceptance criteria:
  - UTF-8이 chunk 경계에서 깨지지 않는다.
  - 중간 종료·취소 시 partial text가 save/history에 확정되지 않는다.
  - Writer가 새 patch/choice를 출력해도 권위가 없다.
- 테스트: SSE byte split, disconnect, cancel, late chunk.

## Epic E8 - 출시 완성도

### ST-035 Settings·health·privacy 진단

- Feature: Operations UX
- Task: provider preset, health 상태, loopback 경고, 설정 화면과 redacted 진단 bundle을 구현한다.
- 우선순위: P1
- 선행 작업: ST-017, ST-028, ST-034
- Acceptance criteria:
  - API key와 본문·자유 입력이 기본 로그·진단에 없다.
  - non-loopback 주소는 첫 요청 전에 경고한다.
  - Loading/Unavailable/Degraded를 사용자용 한국어로 구분한다.
- 테스트: log secret scan, URL cases, TUI snapshot.

### ST-036 벤치마크·v1.0 릴리스 게이트

- Feature: Quality and release
- Task: MLX 4bit/GGUF Q6_K/Q8_0 비교, 블라인드 평가, clean install과 100턴 gate를 자동화·문서화한다.
- 우선순위: P1
- 선행 작업: ST-032, ST-034, ST-035
- Acceptance criteria:
  - 동일 prompt bundle과 sampling metadata를 사용한다.
  - 자동 지표와 블라인드 점수를 별도로 보고한다.
  - clean M1 Max 환경에서 설치→서버→새 게임→save/load가 재현된다.
  - 실제 100턴, 중간 서버 중단, 앱 재시작과 두 번째 Pack 교체가 통과한다.
- 테스트: release checklist 전체, artifact checksum, actual model soak.

## Definition of Done

모든 티켓은 다음을 만족해야 완료다.

- acceptance criteria 각각에 대응하는 테스트 또는 수동 증거가 있다.
- 사용자용 오류는 한국어이며 로그용 원인과 분리된다.
- 본문·프롬프트·자유 입력이 기본 로그에 추가되지 않는다.
- 새 public schema 또는 타입은 version·migration 영향을 검토한다.
- 실제 runtime이 필요한 티켓은 Mock 결과만으로 완료하지 않는다.
- 문서·fixture·테스트 이름이 티켓 ID를 추적할 수 있다.
