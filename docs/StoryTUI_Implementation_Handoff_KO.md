# StoryTUI 구현 핸드오프 계획

문서 버전: 1.0  
작성 기준일: 2026-07-31  
목적: 이 디렉터리의 설계 문서를 실제 구현 담당자가 순서대로 실행할 수 있는 작업 단위로 분해한다.  
범위: 계획·구조화만 포함하며 Rust 소스, 설정, Story Pack 파일은 이 문서에서 생성하지 않는다.

## 1. 현재 상태 감사

현재 디렉터리에는 다음 문서만 있다.

- `StoryTUI_PRD_KO.md`: 제품 범위, MVP/v1.0 요구사항과 성공 지표
- `StoryTUI_Technical_Spec_KO.md`: crate 경계, 상태 모델, provider, 저장, TUI, 테스트 계약
- `StoryTUI_ADR_KO.md`: Rust/Ratatui, 별도 모델 서버, core 분리, JSON 저장, TOML+Markdown Pack 등의 결정
- `StoryTUI_Backlog_KO.md`: ST-001~ST-036의 기능 티켓
- `StoryTUI_Implementation_Plan_KO.md`: M0~M9 마일스톤 계획
- `StoryTUI_Example_Story_Pack_KO.md`: `rainy-reunion` 예제와 Mock 응답 fixture
- `StoryTUI_All_Documents_KO.docx`, `StoryTUI_All_Documents_KO.pdf`: 위 문서의 묶음 산출물

현재 없는 것:

- Git 저장소와 커밋 이력
- `Cargo.toml`, Rust workspace, `apps/storytui`
- `crates/story-model`, `story-core`, `story-provider`, `story-persistence`, `story-tui`
- 실제 Story Pack 디렉터리와 테스트 fixture
- MLX 서버 실행 결과, 모델 버전 기록, 터미널 수동 검증 결과

따라서 기존 문서에 적힌 완료 조건은 아직 달성된 것으로 취급하면 안 된다. 특히 “실제 모델 10턴”, “Terminal.app/iTerm2 검증”, “100턴 soak”, “clean install”은 구현 후 별도 증거가 필요하다.

## 2. 구현 담당자가 따라야 할 기준 문서

우선순위는 다음과 같다.

1. 제품 범위와 게이트: `StoryTUI_PRD_KO.md`
2. 권위 있는 타입·경계·프로토콜: `StoryTUI_Technical_Spec_KO.md`
3. 설계 변경 판단: `StoryTUI_ADR_KO.md`
4. 실행 순서와 티켓: 이 문서와 `StoryTUI_Backlog_KO.md`
5. 예제 입력/출력: `StoryTUI_Example_Story_Pack_KO.md`

문서 사이에 충돌이 생기면 임의로 기존 결정을 덮어쓰지 말고 새 ADR을 추가한다. 특히 다음 경계는 유지한다.

- `story-core`는 Ratatui, Crossterm, Reqwest, 파일 시스템에 의존하지 않는다.
- LLM은 상태의 제안자이며 `GameState`를 직접 수정하지 않는다.
- `StatePatch`는 전체 검증과 dry-run 뒤 원자적으로 적용한다.
- MVP는 단일 비스트리밍 호출이다. Director/Writer와 SSE는 MVP 게이트 이후다.
- Story Pack은 코드 변경 없이 교체 가능해야 한다.
- 기본 모델 서버 주소는 `127.0.0.1`이며, 후보 VLM은 `mlx_vlm.server`를 먼저 검증한다.

## 3. 최종 구현 순서

```text
W0 착수 기준선
  -> W1 런타임·프로토콜 spike
  -> W2 도메인 타입
  -> W3 reducer·규칙 엔진
  -> W4 Story Pack 로더
  -> W5 Mock 기반 최초 수직 슬라이스
  -> W6 실제 MLX provider 연결
  -> W7 자유 입력·History
  -> W8 저장·복구
  -> MVP 게이트
  -> W9 장기 기억
  -> W10 Director/Writer
  -> W11 운영·릴리스
```

W1과 W2는 서로 독립적으로 시작할 수 있지만, W5부터는 반드시 앞 단계의 검증 결과를 입력으로 사용한다. W6은 W5의 Mock contract test가 통과한 뒤에만 진행한다.

## 4. 작업 묶음

### W0. 착수 기준선과 저장소 골격

관련 티켓: ST-001  
목표: 다른 작업자가 동일한 workspace에서 반복 가능한 테스트를 실행할 수 있게 한다.

구현 범위:

- Rust 2024 workspace와 `rust-toolchain.toml`
- `apps/storytui` 및 5개 library crate의 빈 골격
- 공통 `fmt`, `clippy`, `test` 명령
- 의존성 방향과 금지 의존성 검사
- `tests/fixtures`, `tests/contracts`, `tests/soak` 기본 디렉터리

산출물:

- 빌드 가능한 workspace
- 각 crate의 책임을 설명하는 짧은 README 또는 module 문서
- CI에서 실행할 명령 목록

완료 증거:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo tree -p story-core`에 UI·HTTP·파일 I/O 의존성이 없음

중단 게이트: workspace 명령이 재현되지 않으면 다음 작업을 시작하지 않는다.

### W1. TUI 런타임과 로컬 모델 HTTP spike

관련 티켓: ST-002, ST-003, ST-004, ST-005  
목표: 제품 기능과 무관한 가장 큰 환경 위험을 먼저 제거한다.

구현 범위:

- Ratatui/Crossterm의 80x24·120x40 정적 화면
- key, resize, tick channel과 background task
- `TerminalGuard`, panic hook, 정상 종료 복원
- Reqwest 기반 `/health`, `/v1/models`, `/v1/chat/completions` 호출
- `mlx_vlm.server` 후보 실행과 호환 대체 모델 기록
- 한글 grapheme/표시 폭 fixture
- cold/warm latency, TTFT, tokens/s, peak memory 측정 도구

이 단계에서 만들지 않을 것:

- `GameState`, reducer, Story Pack 로더
- 실제 게임 화면과 선택지 처리
- 자동 서버 실행·모델 다운로드

완료 증거:

- 후보 모델의 실제 서버 명령과 package/model revision
- 한국어 text-only 요청 10회 결과와 JSON 유효률
- 서버 미실행·timeout·취소 결과
- Terminal.app/iTerm2 수동 체크리스트
- 4K·8K·16K 기준 측정 결과

중단 게이트: 실제 모델 경로가 실패하면 실패 원인을 기록하고 대체 모델을 선택한 뒤에야 provider 구현으로 넘어간다. 추정으로 `mlx_lm.server`와 `mlx_vlm.server`를 결정하지 않는다.

### W2. 도메인 타입과 직렬화 계약

관련 티켓: ST-006, 일부 ST-007  
목표: 모든 후속 crate가 공유할 타입과 버전 정책을 먼저 고정한다.

구현 범위:

- `CharacterId`, `LocationId`, `EventId`, `FlagId` 등 ID newtype
- `GameState`, `CharacterState`, `Choice`, `HistoryEntry`, `MemoryEntry`
- `StatePatch`, `GeneratedTurn`, `ParsedAction`
- `ConditionExpr`, route/event/ending 관련 enum
- Serde round-trip과 golden JSON
- unknown field, default, enum 호환 정책

작업 규칙:

- 문자열 ID를 public API에서 종류별로 섞지 않는다.
- 저장 DTO와 provider DTO가 달라지면 명시적 변환을 둔다.
- UI 타입(`KeyEvent`, `Rect`, `Widget`)을 이 crate에 넣지 않는다.
- 미확정 필드는 `Option` 또는 명시적 enum으로 표현하고 임의의 빈 문자열을 쓰지 않는다.

완료 증거:

- 모든 public 타입의 Serde round-trip
- golden JSON fixture
- schema version과 unknown field 정책 테스트
- `story-model`이 UI/HTTP/file system 없이 빌드됨

중단 게이트: 상태 타입이 변경되면 이후 reducer/provider 작업을 잠시 멈추고 save/API migration 영향을 먼저 기록한다.

### W3. 검증기·원자 reducer·규칙 엔진

관련 티켓: ST-007~ST-010  
목표: 모델 없이도 상태 변경의 안전성과 이야기 규칙을 증명한다.

구현 순서:

1. 구조 검증: schema, 필수 필드, choice 수, 빈 본문
2. 참조 검증: character/location/flag/item/event ID
3. 범위 검증: affection/trust/tension delta, inventory, 문자열 길이
4. 권한 검증: 현재 장면·Pack 규칙상 허용되는 변경인지
5. 전이 검증: location, relationship, route, ending
6. dry-run 후 전체 commit
7. `StateChangeLog`에 before/proposed/applied/decision 기록

반드시 지킬 불변식:

- patch 후반부 실패가 앞부분의 부분 적용을 남기지 않는다.
- 성공한 commit은 revision을 정확히 한 번 증가시킨다.
- route·ending의 최종 판정 필드는 LLM patch에 노출하지 않는다.
- 같은 입력과 같은 state는 결정적으로 같은 결과를 낸다.

완료 증거:

- delta 경계값·unknown ID·음수 inventory 테스트
- rollback fault test
- 조건식 truth table
- route/relationship/ending forbidden transition 테스트
- 동일 입력 100회 결정성 테스트

중단 게이트: 이 묶음의 테스트가 통과하기 전에는 실제 모델의 결과를 연결하지 않는다.

### W4. Story Pack 로더와 예제 콘텐츠

관련 티켓: ST-011~ST-015  
목표: 파일만 교체해 이야기 정의를 바꾸고, 시작 전에 오류를 모두 진단한다.

구현 순서:

1. `story.toml`, `characters.toml`, `rules.toml`, `events.toml` DTO
2. `world.md`, `style.md`, `opening.md`, prompt fragment 로딩
3. canonical `StoryPack` 변환
4. root-relative path와 파일 크기·UTF-8 검사
5. ID registry와 참조 무결성 검사
6. schema major 호환 및 in-memory default migration
7. 도달 가능성·장소 전이 검사
8. teen metadata 검사
9. `rainy-reunion` 실제 fixture 작성

완료 증거:

- 정상 Pack validate 결과
- 누락 파일·TOML 오류·중복 ID·없는 참조·path traversal fixture
- unsupported major와 same-major migration fixture
- 예제 Pack의 validator warning/error 0
- 첫 Mock 응답이 W3 reducer를 통과하는 fixture

중단 게이트: Pack 검증이 통과하지 않은 상태에서 prompt나 TUI를 Pack 구조에 맞춰 임시 하드코딩하지 않는다.

### W5. Mock 기반 최초 수직 슬라이스

관련 티켓: ST-016, ST-018~ST-021  
목표: 네트워크와 실제 모델 없이 새 게임부터 선택·다음 장면까지 완성한다.

구현 범위:

- `LlmProvider` trait와 capability/health/cancel 계약
- 성공·지연·malformed·timeout·late result를 주입하는 MockProvider
- deterministic `PromptContext`와 prompt hash
- GeneratedTurn strict parser와 제한된 repair/retry
- generation ID, cancellation, late response 폐기
- Story 화면의 선택지 3개와 loading/error 상태
- 검증된 patch만 commit하는 coordinator

권장 작업 순서:

1. provider contract test를 Mock만으로 작성
2. prompt golden fixture 작성
3. parser/retry 정책 작성
4. coordinator race/cancel 테스트 작성
5. TUI에 command/event를 연결
6. `rainy-reunion` 3턴 Mock E2E 작성

완료 증거:

- Mock 3턴 E2E
- malformed JSON, unknown field, invalid enum, 과다 choice, 과다 delta fixture
- 취소 후 late response가 state/history를 바꾸지 않음
- 모델 호출 중 resize와 입력 처리
- 선택 직전 checkpoint와 성공 후 stable commit

중단 게이트: Mock E2E가 통과하기 전에는 실제 provider를 붙이지 않는다.

### W6. 실제 MLX provider 연결과 MVP 게이트 준비

관련 티켓: ST-017, ST-019 일부, ST-021 일부  
목표: W5에서 검증한 계약을 실제 로컬 MLX 서버에 연결한다.

구현 범위:

- TOML 설정의 base URL/model/sampling/timeout
- loopback 기본값과 non-loopback 경고
- OpenAI-compatible response envelope 처리
- 실제 health/models/chat smoke
- 10턴 선택지 플레이와 autosave 최소 경로

하지 않을 것:

- Director/Writer
- SSE
- 자동 모델 다운로드와 서버 lifecycle 관리

완료 증거:

- 서버·모델·chat template·sampling·hardware 기록
- 실제 10턴 세션 기록
- JSON 1차 파싱률과 repair 포함 성공률
- 서버 중단·timeout·취소 후 마지막 안정 상태 복귀

중단 게이트: 실제 모델 증거가 없으면 MVP 수직 슬라이스 완료로 표시하지 않는다.

### W7. 자유 입력과 History

관련 티켓: ST-022~ST-024  
목표: 선택지와 자유 입력이 같은 상태 전이 및 안정 상태 경계를 사용하게 한다.

구현 순서:

1. grapheme 기반 cursor/delete/paste/max length
2. known ID·행동·대사·이동의 제한된 규칙 parser
3. 낮은 확신의 `general_action` fallback
4. 원문 보존 및 prompt contract 반영
5. 확정 generation만 기본 History에 표시
6. generation lineage와 재생성·rollback

완료 증거:

- 한글·영문·emoji·combining mark 입력 fixture
- Terminal.app/iTerm2 수동 입력 결과
- 20개 자유 입력 의도 평가
- 재생성 성공·실패·취소에서 기존 확정 턴 보존

중단 게이트: 자유 입력 parser가 모호한 문장을 확정 행동으로 과잉 해석하지 않아야 한다. 불확실하면 원문을 보존한다.

### W8. 저장·복구와 MVP 품질 게이트

관련 티켓: ST-025~ST-028  
목표: 안정 상태를 손상 없이 저장·로드하고 MVP를 판정한다.

구현 순서:

1. `SaveEnvelope`, canonical payload, checksum
2. same-directory temp → flush/sync → rename
3. `.bak`와 checkpoint retention
4. v0→v1 in-memory migration
5. future major, checksum mismatch, truncated JSON 진단
6. autosave·quick save·수동 3슬롯 화면
7. delete/overwrite/duplicate 확인 UX

저장에 포함하지 않을 것:

- prompt/API secret
- 완료되지 않은 자유 입력 buffer
- 확정되지 않은 streaming 본문
- 생성 중간의 불안정한 state

완료 증거:

- 100회 save/load round-trip
- temp/sync/rename 각 단계 fault injection
- `.bak` 복구와 기존 파일 보존
- 앱 종료·재시작 후 마지막 안정 상태 복구
- 실제 플레이 세션의 autosave 재개

MVP 게이트:

- 예제 Pack 10턴 세션 성공률 95% 이상
- 복구 포함 JSON 성공률 98% 이상
- 선택지 중복률 15% 이하
- 자유 입력 핵심 의도 반영률 85% 이상
- 한국어 수동 평균 3.8/5 이상
- save/forced shutdown 테스트 100% 통과

게이트가 실패하면 W9 이후 기능을 시작하지 말고 prompt, Pack, patch 계약, 모델 preset 또는 UX를 수정한다.

### W9. 장기 기억과 100턴 synthetic soak

관련 티켓: ST-029~ST-032  
목표: raw history 전체를 보내지 않고 핵심 사실·관계·미해결 사건을 보존한다.

구현 범위:

- 사실/해석/source history ID/knowledge owner를 분리한 memory model
- event 기반 deterministic memory 생성
- 장면·챕터 요약 contract와 source coverage 검사
- fixed/state/recent/summary/event 계층 selector
- token budget과 선택 trace
- 20·50·100턴 synthetic 및 실제 지정 시나리오 runner

완료 증거:

- 약속·비밀·호칭 변화·관계 원인 보존 fixture
- 지식 소유자 위반 fixture
- 동일 state에서 deterministic selection
- 100턴 후 state/memory/save 불변식과 budget 보고서
- 핵심 기억 recall 90% 이상

중단 게이트: 요약이 원본 근거 없는 사실을 확정하거나 핵심 사건을 잃으면 Director/Writer로 진행하지 않는다.

### W10. Director/Writer 단계 분리

관련 티켓: ST-033, ST-034  
목표: 구조적 계획을 먼저 확정하고 한국어 본문은 그 계획을 표현하도록 만든다.

구현 순서:

1. 비스트리밍 `DirectorPlan` schema
2. must reveal/must not reveal, knowledge scope, emotion arc 검증
3. Director patch reducer dry-run
4. 확정 plan 기반 Writer 요청
5. SSE byte/UTF-8 boundary parser
6. cancel/disconnect/late chunk 처리
7. Writer lint와 확정 전 partial text 격리
8. 단일 호출과 2단계 호출 A/B 평가

권위 규칙:

- Writer는 새 patch·choice·route·ending을 만들 수 없다.
- Writer stream 중간 텍스트는 확정 전 save/history에 넣지 않는다.
- Director가 reducer를 통과하지 못하면 Writer를 호출하지 않는다.

완료 증거:

- invalid plan 및 retry fixture
- UTF-8 split, disconnect, cancel, late chunk 테스트
- 규칙 위반 patch 적용 0
- 실제 모델 20턴 A/B 비교와 latency 기록

### W11. 운영·릴리스 완성도

관련 티켓: ST-035, ST-036  
목표: 실제 사용·진단·재현 가능한 v1.0 release를 만든다.

구현 범위:

- Settings, provider health, model preset, keymap
- secret/content redaction과 진단 bundle
- 80x24 접근성 및 오류 화면
- 동일 prompt bundle 기반 benchmark
- blind quality review
- clean install, 두 번째 Pack 교체, 100턴 release test
- Apple Silicon binary, checksum, 설치·troubleshooting 문서

완료 증거:

- 기본 로그에 prompt·본문·자유 입력·secret 누출 0
- clean environment에서 설치→서버→새 게임→save/load 재현
- 실제 100턴과 서버 중단·앱 재시작
- 코드 변경 없이 두 번째 Story Pack 실행
- `story-core` public API에 UI/provider leak 없음

## 5. 구현 담당자용 파일 소유권

초기 구현은 아래 디렉터리 책임을 넘지 않도록 한다.

| 영역 | 소유 crate/경로 | 넣지 말아야 할 것 |
|---|---|---|
| 타입·스키마 | `crates/story-model` | HTTP, terminal, 실제 I/O |
| 규칙·전이 | `crates/story-core` | Ratatui, Reqwest, 파일 저장 |
| 모델 호출 | `crates/story-provider` | route/ending 판정, GameState 직접 변경 |
| 세이브·Pack 파일 | `crates/story-persistence` | UI 상태, 모델 prompt 조립 |
| 화면·입력 | `crates/story-tui` | reducer, 규칙 판정, 저장 포맷 결정 |
| 조립·실행 | `apps/storytui` | 도메인 규칙을 main에 임시 구현 |
| 콘텐츠 | `stories/rainy-reunion` | Rust 코드에 이야기 내용 하드코딩 |

각 작업은 가능한 한 한 영역의 파일을 소유하고, 다른 영역과의 변경은 trait/DTO/command 계약으로 연결한다.

## 6. 공통 테스트와 증거 규칙

모든 티켓은 다음 중 해당하는 증거를 남긴다.

- 단위/속성 테스트: deterministic domain, parser, migration
- contract test: provider/storage trait의 Mock 구현
- snapshot: TUI, prompt, 사용자 오류 메시지
- fixture: 정상·실패·migration·corruption·protocol 사례
- 실제 runtime: 모델 서버, 터미널, 강제 종료, clean install
- 결과 문서: 버전, 명령, 환경, 표본 수, 실패와 미검증 범위

`cargo build`, Mock 테스트, 문서 작성만으로 실제 runtime 티켓을 완료 처리하지 않는다. 실제 모델이 필요한 테스트는 일반 CI와 분리한 ignored/local suite로 두고 실행 결과를 별도 기록한다.

## 7. 먼저 구현할 작업 목록

다른 구현 담당자는 아래 순서로 시작하면 된다.

1. W0/ST-001: workspace와 lint/test 명령
2. W1/ST-002: TerminalGuard와 비동기 TUI spike
3. W1/ST-003: 후보 `mlx_vlm.server` smoke 기록
4. W2/ST-006: 도메인 타입·Serde golden fixture
5. W3/ST-007~010: validator/reducer/rules
6. W4/ST-011~015: Pack loader와 `rainy-reunion`
7. W5/ST-016, 018~021: Mock 수직 슬라이스
8. W6/ST-017: 실제 MLX 연결

각 단계의 완료 보고에는 “구현됨”, “자동 검증됨”, “실제 환경 검증됨”을 구분해 표시한다.

## 8. 명시적 제외 범위

MVP 게이트 전에는 다음 작업을 시작하지 않는다.

- 웹 UI, GUI, 이미지, 음성, Live2D
- vector DB와 embedding 검색
- 클라우드 동기화, 멀티플레이
- 자동 모델 다운로드·업데이트·서버 process manager
- plugin market과 복잡한 Ink 통합
- 다중 모델 voting

새 요구가 생기면 기존 작업에 끼워 넣지 말고 backlog의 별도 티켓과 영향받는 ADR을 먼저 만든다.

## 9. 계획의 결론

현재 문서는 제품 방향과 설계가 충분히 정리되어 있지만, 구현 가능한 코드 기반은 아직 시작 전이다. 따라서 첫 구현자는 기능 화면부터 만들지 말고 W0~W3에서 경계·상태·원자성을 고정해야 한다. 그 뒤 `rainy-reunion`을 사용한 Mock 수직 슬라이스로 전체 흐름을 증명하고, 실제 MLX provider를 연결한 후에만 자유 입력·저장·장기 기억을 확장한다.

이 순서를 지키면 모델 품질 문제, TUI 문제, 상태 무결성 문제를 서로 섞지 않고 단계별로 판정할 수 있다.
