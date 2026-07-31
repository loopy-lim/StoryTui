# StoryTUI 구현 계획서

문서 버전: 1.0-draft  
작성 기준일: 2026-07-31  
계획 단위: M0~M9, 달력 날짜 대신 상대 난이도와 검증 게이트 사용

## 1. 실행 원칙

1. 마일스톤은 앞 단계의 완료 조건이 증명된 뒤에만 시작한다.
2. 문서·컴파일·Mock 테스트만으로 실제 모델 경로를 완료 처리하지 않는다.
3. MVP 품질 게이트 전에는 이미지·음성·웹·벡터 검색 작업을 시작하지 않는다.
4. 각 마일스톤은 실행 가능한 vertical evidence를 남긴다: 명령, fixture, 로그 요약, 스크린샷 또는 세이브 아티팩트.
5. 실제 모델 테스트는 일반 단위 테스트와 분리하고 모델·서버 버전·chat template·sampling을 기록한다.
6. “부분 완료”는 완료로 간주하지 않는다. 제외 범위는 backlog로 옮기고 현재 milestone의 acceptance와 섞지 않는다.

## 2. 우선순위와 난이도

- **P0**: 다음 단계 진입을 막는 필수 기반
- **P1**: 해당 릴리스 목표의 필수 기능
- **P2**: 품질·운영성 향상, 핵심 경로 후 수행
- **P3**: 이후 검토

상대 난이도:

- **S**: 경계가 좁은 0.5~1 작업 단위
- **M**: 여러 파일과 테스트가 필요한 2~4 작업 단위
- **L**: 복수 하위 기능과 실제 환경 검증이 필요한 5~8 작업 단위
- **XL**: 실험·반복·장기 테스트를 포함한 9개 이상 작업 단위

## 3. 마일스톤 의존성

```text
M0 기술 검증
  -> M1 Story Core
      -> M2 Story Pack
          -> M3 최초 수직 슬라이스 (MVP 핵심)
              -> M4 자유 입력과 기록
              -> M5 세이브와 로드
                  -> MVP 품질 게이트
                      -> M6 장기 기억
                          -> M7 Director/Writer
                              -> M8 완성도
                                  -> M9 v1.0
```

M4와 M5는 M3 이후 일부 병행할 수 있으나, shared `GameState`와 persistence 계약을 동시에 바꾸지 않도록 integration 순서를 정한다.

## 4. M0 - 기술 검증

### 목적

M1 Max 64GB에서 TUI, 한글 입력과 초기 후보 모델의 실제 HTTP 생성 경로가 성립하는지 확인한다. 이 단계는 제품 기능 개발이 아니라 가장 위험한 가정을 제거한다.

### 우선순위·난이도·의존성

- 우선순위: P0
- 난이도: L
- 선행 조건: macOS, Rust stable, Python 가상환경, 모델 접근 가능

### 구현 범위

- Rust workspace skeleton과 단일 앱
- Ratatui/Crossterm 기본 레이아웃과 이벤트 loop
- `TerminalGuard` 최소 버전
- Reqwest로 `/health`, `/v1/models`, `/v1/chat/completions` 호출
- 후보 모델의 한국어 text-only 응답과 JSON 응답 확인
- Terminal.app·iTerm2 한글 IME와 표시 폭 검증
- 4K·8K·16K 이상 컨텍스트의 기초 성능 측정

### 제외 범위

- 정식 GameState, reducer, Story Pack 로더
- 세이브 호환성, production UI, Director/Writer
- 모델 품질 최적화와 다중 Provider

### 주요 작업

1. `rust-toolchain.toml`, workspace lint와 CI 기본 명령을 고정한다.
2. Story·선택지·상태줄의 정적 mock 화면을 80x24와 120x40에서 그린다.
3. 키·resize·tick 이벤트를 channel로 전달하고 생성 task 동안 spinner가 움직이는지 확인한다.
4. 후보 모델 카드가 `mlx-vlm` 변환본임을 반영해 `mlx_vlm.server --host 127.0.0.1`를 우선 실행한다.
5. `mlx_lm.server`는 호환 텍스트 모델 비교에만 사용하며 후보에 억지 적용하지 않는다.
6. 한국어 system/user 메시지로 비스트리밍 응답을 10회 수집한다.
7. JSON Schema on/off, `top_k`, `max_tokens`, timeout 동작을 확인한다.
8. 서버 미실행·로딩·주소 오류·취소를 재현한다.
9. 한글 조합, 확정, 백스페이스, 문장 중간 이동, 붙여넣기와 긴 줄을 두 터미널에서 점검한다.
10. cold load, warm TTFT, total latency, tokens/s와 peak memory를 기록한다.

### 산출물

- 실행 가능한 기술 spike
- `docs/validation/M0-model-server.md`
- `tests/manual/M0-korean-ime-checklist.md`
- 기초 benchmark CSV/JSON
- 실제 request/response에서 콘텐츠를 제거한 protocol fixture

### 테스트 방법

- `cargo test --workspace`
- 정적 TUI snapshot
- 실제 서버 smoke 10회
- 서버 프로세스 종료 중 요청 테스트
- Terminal.app·iTerm2 수동 체크리스트
- 4K·8K·16K 컨텍스트별 최소 3회 측정

### 완료 조건

- TUI가 생성 task 중 멈추지 않는다.
- 종료·panic 시 터미널을 복원한다.
- 후보 또는 명시한 대체 MLX 모델에서 한국어 `/v1/chat/completions`가 성공한다.
- 후보 모델의 실제 서버가 `mlx_vlm.server`인지 `mlx_lm.server`인지 증거와 함께 결정된다.
- JSON 유효 응답을 최소 8/10회 얻거나, structured output이 안정적으로 동작함을 확인한다.
- 두 터미널에서 한글 확정 입력과 줄바꿈이 깨지지 않는다.
- 메모리·지연 결과에 따라 MVP context 예산을 잠정 고정한다.

### 예상 위험

- 후보 모델이 text-only server와 호환되지 않음
- 첫 로드가 timeout을 초과하거나 메모리 압박 발생
- IME 조합 이벤트가 터미널별로 다름
- 서버의 structured output 구현이 모델과 충돌

### 다음 단계 진입 조건

모델 서버·TUI·한글 입력 3개 축의 증거가 모두 있어야 한다. 후보가 실패해도 Provider 계약에서 호환 대체 모델로 성공하고 실패 원인이 문서화되면 M1로 진행할 수 있다.

## 5. M1 - Story Core

### 목적

LLM 없이도 게임 상태와 규칙 전이를 완전히 테스트할 수 있는 결정적 코어를 만든다.

### 우선순위·난이도·의존성

- 우선순위: P0
- 난이도: L
- 선행: M0 workspace·toolchain

### 구현 범위

- `story-model`, `story-core`
- ID newtype, `GameState`, `CharacterState`, `Choice`, `ParsedAction`
- `StatePatch`, validator, reducer, audit log
- 조건식·기본 route/event/ending state machine

### 제외 범위

- 파일 로딩, HTTP, TUI 연결, 실제 story 콘텐츠
- memory summarization

### 주요 작업

1. Serde schema와 unknown field 정책을 정의한다.
2. 기본 상태 범위와 Pack 규칙 override 구조를 구현한다.
3. 구조·참조·범위·권한·전이·dry-run 검증 단계를 분리한다.
4. reducer가 성공 시 새 state와 change log를, 실패 시 원본 state를 반환하게 한다.
5. route·ending은 모델 patch 필드가 아닌 state machine이 결정하게 한다.
6. relationship stage 전이와 예외 event를 구현한다.
7. proptest 또는 table-driven test로 경계값과 rollback을 검증한다.

### 산출물

- `story-model`, `story-core` crate
- 공통 오류 코드
- domain test fixture와 상태 전이 문서

### 테스트 방법

- delta -4~-2, 0, +2~+4 경계 테스트
- unknown character/location/flag
- inventory 음수, 관계 단계 건너뛰기
- patch의 마지막 필드가 실패할 때 앞 필드도 미적용
- 같은 입력 100회 결과·직렬화 동일
- route와 ending 조건 truth table

### 완료 조건

- 전체 domain suite 통과
- `story-core` dependency tree에 UI·HTTP crate 없음
- 실패 patch의 부분 적용 사례 0
- 핵심 route·ending을 LLM 입력 없이 재현

### 예상 위험

- DTO와 domain type이 너무 일찍 복잡해짐
- 규칙 DSL이 MVP보다 과도하게 일반화됨

### 다음 단계 진입 조건

예제 상태로 새 게임→선택→patch→event→ending까지 순수 테스트로 시뮬레이션할 수 있어야 한다.

## 6. M2 - Story Pack

### 목적

코드 변경 없이 이야기·캐릭터·규칙·프롬프트를 로드하고 시작 전에 오류를 발견한다.

### 우선순위·난이도·의존성

- 우선순위: P0
- 난이도: L
- 선행: M1 타입·규칙 계약

### 구현 범위

- TOML/Markdown 로더
- schema version과 semver 호환
- 참조 무결성·경로 탈출·모든 인물 Pack 메타데이터 검증
- 1캐릭터·3장소·5플래그 예제 Pack

### 제외 범위

- 자동 migration 파일 덮어쓰기
- 원격 Pack 설치·서명·market

### 주요 작업

1. Pack manifest DTO와 canonical `StoryPack` 변환을 구현한다.
2. 문자 인코딩·파일 크기·상대 경로 보안을 검사한다.
3. character·location·flag·event·route ID registry를 만든다.
4. 조건식 참조와 도달 가능성을 검사한다.
5. Markdown prompt fragment의 필수 placeholder를 검사한다.
6. 모든 인물 등급일 때 주요 인물 나이·모든 인물 선언을 검증한다.
7. 좋은 fixture와 오류별 작은 fixture를 작성한다.

### 산출물

- Pack loader와 validator
- 예제 `rainy-reunion` Pack 초판
- `storytui pack validate` 내부 API 또는 CLI
- 오류 위치·코드·한국어 메시지

### 테스트 방법

- 누락 파일, TOML 문법, 중복 ID, 없는 참조
- Pack root 밖 `../` 참조
- unsupported major, same-major minor default
- 미성년 또는 나이 누락 teen Pack 거부
- snapshot으로 진단 메시지 고정

### 완료 조건

- 좋은 Pack을 canonical model로 로드
- 모든 잘못된 fixture가 예상 code로 실패
- Pack 파일만 복사해 새 story ID를 발견
- 로더가 임의 코드를 실행하지 않음

### 예상 위험

- line/column 정보가 DTO 변환 과정에서 사라짐
- writer-friendly TOML과 엄격 schema의 긴장

### 다음 단계 진입 조건

예제 Pack으로 M1 초기 상태와 규칙을 만들고 정적 첫 장면 prompt context를 생성할 수 있어야 한다.

## 7. M3 - 최초 수직 슬라이스

### 목적

새 게임→첫 장면→3개 선택지→다음 장면→자동 저장의 실제 경로를 완성한다.

### 우선순위·난이도·의존성

- 우선순위: P0
- 난이도: XL
- 선행: M0, M1, M2

### 구현 범위

- MLX OpenAI-compatible Provider
- MVP 단일 호출 비스트리밍 JSON
- Story 화면과 선택
- 1캐릭터·3장소·5플래그 이하
- 안정 상태 개념과 자동 저장 1개

### 제외 범위

- Director/Writer, Writer streaming
- 복수 수동 세이브 UI, 장기 요약, 다중 route

### 주요 작업

1. `LlmProvider`와 `OpenAiCompatibleProvider`를 구현한다.
2. Pack·상태·최근 입력을 deterministic prompt로 조립한다.
3. 응답 envelope에서 content를 추출하고 엄격 파싱한다.
4. code fence 제거와 안전한 1차 복구를 구현한다.
5. generated choice와 patch를 M1 validator에 연결한다.
6. 선택 직전 checkpoint, 성공 후 stable commit과 autosave를 구현한다.
7. generation ID와 late response 폐기를 구현한다.
8. spinner, 취소, retry와 기본 오류 modal을 연결한다.
9. 실제 모델로 10턴 플레이를 기록한다.

### 산출물

- `storytui new/play` 실행 경로
- 예제 Pack MVP
- Mock end-to-end fixture
- 실제 모델 10턴 smoke 결과

### 테스트 방법

- Mock으로 3턴 deterministic flow
- malformed JSON, unknown flag, 과다 delta
- 서버 미실행, timeout, cancel, 늦은 응답
- 실제 모델 10턴과 autosave 재시작

### 완료 조건

- 실제 모델에서 최소 10턴을 선택지로 진행
- patch는 검증 후에만 적용
- 생성 실패·취소 후 체크포인트 유지
- TUI가 모델 호출 중 입력·resize를 처리
- autosave로 프로세스 재시작 후 마지막 확정 턴 복구

### 예상 위험

- chat template가 JSON 지시를 약화
- narration과 patch의 의미 불일치
- 한 호출에 장면·선택·patch를 모두 요구해 truncation

### 다음 단계 진입 조건

핵심 vertical flow가 Mock과 실제 모델 양쪽에서 통과하고, 실패 복구 로그가 남아야 한다.

## 8. M4 - 자유 입력과 기록

### 목적

플레이어가 준비된 선택지 밖에서 행동하고, 그 의도와 결과를 검토·재생성할 수 있게 한다.

### 우선순위·난이도·의존성

- 우선순위: P1
- 난이도: L
- 선행: M3

### 구현 범위

- grapheme 기반 한글 입력
- 규칙 기반 `ParsedAction`
- History/Backlog
- 재생성·이전 체크포인트 복구
- `$EDITOR` 기술 검토

### 제외 범위

- 별도 LLM action parser
- 완전한 다중 줄 편집기, editor 정식 출시

### 주요 작업

1. 입력 buffer cursor·delete·paste·max length를 구현한다.
2. known character/location과 단순 한국어 동사로 action을 보조 파싱한다.
3. confidence가 낮으면 `general_action`과 원문을 보존한다.
4. prompt에 “플레이어가 명시하지 않은 감정·행동을 확정하지 말 것”을 추가한다.
5. History에 선택/자유 입력·본문·적용 patch를 연결한다.
6. 재생성 결과를 commit하기 전 원본 generation을 보존한다.
7. editor 전환 시 terminal 복원 spike를 별도 feature flag로 만든다.

### 산출물

- FreeInput와 History 화면
- action parser fixture
- 재생성·rollback 시나리오
- editor spike 결정 메모

### 테스트 방법

- 한글·영문·emoji·붙여넣기·긴 입력
- “유나에게 사과한다”, 대사, 이동, 모호한 문장
- 자유 입력이 다음 prompt와 history에 원문 그대로 포함
- 재생성 취소·성공·실패에서 checkpoint 검사

### 완료 조건

- Terminal.app·iTerm2 자유 입력 체크리스트 통과
- 파싱 실패 시 입력 손실 0
- 20개 고정 자유 입력 중 핵심 의도 반영 수동 평가 85% 이상
- History에서 확정 generation만 기본 표시

### 예상 위험

- 한글 형태소 분석을 과도하게 구현
- 재생성이 history를 모호하게 만듦

### 다음 단계 진입 조건

선택지와 자유 입력이 같은 state transition 경로를 사용하고 안정 저장 경계를 공유해야 한다.

## 9. M5 - 세이브와 로드

### 목적

장기 플레이의 데이터 내구성과 버전 호환성을 제품 수준으로 만든다.

### 우선순위·난이도·의존성

- 우선순위: P1
- 난이도: L
- 선행: M3, M4의 안정 generation 모델

### 구현 범위

- autosave, quick save, 수동 슬롯 3개 이상
- 설명·시각·챕터·장면 metadata
- checksum, backup, atomic write, migration
- 삭제·복제·손상 복구 UI

### 제외 범위

- 클라우드 동기화, SQLite
- 다른 major의 무제한 호환

### 주요 작업

1. `SaveEnvelope`와 canonical checksum을 고정한다.
2. 같은 디렉터리 temp→sync→rename을 구현한다.
3. `.bak` 1세대와 checkpoint retention을 구현한다.
4. 로드는 임시 state 검증 후 활성 state와 교체한다.
5. v0 fixture에서 v1 migration을 구현한다.
6. future version, checksum mismatch와 잘린 JSON을 진단한다.
7. 슬롯 목록·삭제 확인·복제 이름 충돌을 구현한다.
8. fault injection으로 쓰기 단계별 실패를 재현한다.

### 산출물

- `story-persistence`
- Save/Load 화면
- migration fixture
- 손상 복구 안내

### 테스트 방법

- state/history/memory round-trip
- temp write, fsync, backup, rename 단계 실패
- 앱 종료·재시작·서버 중단 중 load
- save 생성 중에는 stable state만 포함
- `.bak` 복구와 원본 보존

### 완료 조건

- 100회 반복 save/load에서 checksum·상태 동일
- 단계별 실패가 기존 세이브를 손상시키지 않음
- future major 로드가 현재 게임을 바꾸지 않음
- 수동 3슬롯·autosave·quick save UX 통과

### 예상 위험

- macOS filesystem의 sync·rename 의미 차이
- history 증가로 JSON 파일이 커짐

### 다음 단계 진입 조건

MVP 기능 전체의 데이터 내구성 테스트와 10턴 실제 플레이 복구를 통과한다.

## 10. MVP 품질 게이트

M5 후 M6로 넘어가기 전에 다음을 평가한다.

- 10턴 실제 플레이 세션 10회 중 9회 이상 정상 완료
- JSON 복구 포함 성공률 98% 이상
- 선택지 중복 턴 15% 이하
- 자유 입력 의도 반영 85% 이상
- 한국어 수동 평균 3.8/5 이상
- 세이브·복구 테스트 100%

실패 시 이미지·웹·벡터 DB가 아니라 prompt, Pack, patch 계약, 모델 preset과 UX를 개선한다.

## 11. M6 - 장기 기억

### 목적

전체 history를 프롬프트에 넣지 않고 50~100턴의 설정·관계·미해결 사건을 유지한다.

### 우선순위·난이도·의존성

- 우선순위: P1
- 난이도: XL
- 선행: MVP 품질 게이트, M5

### 구현 범위

- 고정·상태·최근·요약·사건 기억
- `MemorySelector`
- 장면·챕터·관계 요약과 근거 ID
- 20·50·100턴 fixture·soak

### 제외 범위

- embedding, vector DB, 외부 memory service

### 주요 작업

1. `MemoryEntry`와 `MemoryFact`를 구현한다.
2. event 기반 중요 기억을 deterministic하게 생성한다.
3. 요약 contract에서 사실·해석·미해결·지식 소유자를 분리한다.
4. selector의 절대 포함 규칙과 score를 구현한다.
5. token estimator와 계층별 예산을 구현한다.
6. 현재 장면 참여자·장소·route에 따른 fixture를 만든다.
7. 요약 손실을 검출하는 source history 검증을 구현한다.
8. 50·100턴 synthetic/actual test를 실행한다.

### 산출물

- memory subsystem과 selector trace
- 100턴 soak harness
- 요약 품질 평가표

### 테스트 방법

- 약속·비밀·호칭 변화·관계 원인 보존
- 모르는 인물에게 사실이 누출되지 않음
- budget 초과에서 우선순위 순서
- save/load 후 selector 결과 안정성
- 실제 모델 50턴 1회, 100턴 지정 시나리오

### 완료 조건

- 100턴에서 메모리 사용량과 prompt budget이 상한 내
- 핵심 사건 recall 90% 이상
- 미해결 사건·관계 변화 원인·knowledge owner 손실이 release fixture에서 0
- 전체 raw history 없이 장기 플레이 성공

### 예상 위험

- 요약 모델의 hallucination
- token 추정 오차와 과도한 fixed prompt

### 다음 단계 진입 조건

Director와 Writer가 사용할 compact context가 명세대로 안정적으로 생성되어야 한다.

## 12. M7 - Director와 Writer 분리

### 목적

구조적 계획과 한국어 문장을 분리해 규칙 충돌을 줄이고 Writer 본문을 스트리밍한다.

### 우선순위·난이도·의존성

- 우선순위: P1
- 난이도: XL
- 선행: M6

### 구현 범위

- 비스트리밍 Director JSON
- plan 검증·patch 확정
- Writer 한국어 본문 SSE
- 생성 취소·retry·부분 실패
- Writer lint와 확정 commit

### 제외 범위

- 동시에 여러 모델 사용
- speculative multi-writer voting

### 주요 작업

1. `DirectorPlan` schema와 Pack prompt를 작성한다.
2. `must_reveal`/`must_not_reveal`, emotion arc와 end condition을 검증한다.
3. Director의 choice·patch를 reducer dry-run에 연결한다.
4. Writer에는 확정 plan과 표현용 상태만 제공한다.
5. SSE parser, chunk coalescing, UTF-8 경계와 `[DONE]` 처리를 구현한다.
6. generation ID, cancellation과 late chunk 폐기를 구현한다.
7. Writer가 상태·선택지를 새로 쓰면 무시하고 lint 경고를 낸다.
8. 단일 호출 MVP와 2단계 구조를 같은 fixture로 비교한다.

### 산출물

- Director/Writer provider path
- 스트리밍 Story 화면
- protocol·failure fixture
- 품질 비교 보고서

### 테스트 방법

- 불완전 JSON, code fence, invalid enum, 5 choices, 큰 delta
- repair 1회, full retry 1회 상한
- stream UTF-8 split, 연결 종료, 취소, 늦은 chunk
- Writer가 금지 사실을 공개하는 fixture
- 실제 모델 20턴 A/B

### 완료 조건

- 규칙 위반 patch 적용 0
- Writer streaming 중 UI 반응성 유지
- 중간 실패가 history·save를 오염시키지 않음
- 수동 평가에서 단일 호출 대비 장면 품질·일관성 악화 없음

### 예상 위험

- 호출 2배로 latency 증가
- Director와 Writer context 중복

### 다음 단계 진입 조건

20턴 실제 플레이에서 latency가 허용 가능하고 구조 위반률이 목표에 도달해야 한다.

## 13. M8 - 완성도 개선

### 목적

설정·진단·접근성·벤치마크와 사용자 편의성을 출시 후보 수준으로 올린다.

### 우선순위·난이도·의존성

- 우선순위: P1/P2
- 난이도: XL
- 선행: M7

### 구현 범위

- Settings, model presets, keymap, theme
- Character·Game State·오류 상세 화면
- provider health·진단 bundle
- 품질 benchmark·blind scoring 도구
- 성능 최적화와 로그 privacy

### 제외 범위

- 웹 UI, plugin market, cloud

### 주요 작업

1. 안전한 설정 reload 범위와 restart-required 필드를 구분한다.
2. loopback 외 URL 경고와 secret 마스킹을 구현한다.
3. key conflict와 80x24 접근성을 검증한다.
4. 로그 content redaction audit를 추가한다.
5. MLX 4bit와 GGUF Q6_K/Q8_0 benchmark runner를 만든다.
6. prompt hash, sampling, seed 가능 여부와 결과 metadata를 저장한다.
7. blind review sheet를 생성한다.
8. profiling으로 app overhead와 history 렌더 병목을 개선한다.

### 산출물

- Settings·상태·진단 화면
- model preset과 benchmark schema
- privacy audit와 운영 가이드
- release candidate

### 테스트 방법

- 설정 오류·hot reload·재시작 필요
- 작은/큰 터미널 snapshot
- loopback/non-loopback URL
- 로그에 fixture 본문·자유 입력이 없는지 검사
- 4bit/Q6_K 비교 benchmark

### 완료 조건

- 필수 화면과 오류 UX가 완성
- 기본 로그 민감 텍스트 누출 0
- benchmark가 동일 prompt bundle을 재현
- 앱 자체 turn overhead 목표 200ms 이하

### 예상 위험

- 설정 화면이 provider별 세부 옵션에 결합
- benchmark 결과가 seed 비결정성 때문에 오해됨

### 다음 단계 진입 조건

릴리스 체크리스트의 기능·진단·privacy·성능 항목이 모두 통과해야 한다.

## 14. M9 - v1.0

### 목적

재현 가능한 설치, 100턴 안정성, 스키마 호환성과 코어 API를 고정해 첫 정식 릴리스를 만든다.

### 우선순위·난이도·의존성

- 우선순위: P0 release
- 난이도: XL
- 선행: M8와 모든 이전 게이트

### 구현 범위

- 설치·실행·문제 해결·Pack 제작 문서
- Apple Silicon release binary
- example Pack
- save/Pack compatibility policy
- 100턴 실제·synthetic 검증
- public `story-core` API review

### 제외 범위

- notarization을 포함한 GUI installer는 별도 결정
- Windows/Linux MLX 지원
- 웹 렌더러 구현 자체

### 주요 작업

1. clean machine 또는 새 사용자 환경에서 설치 문서를 실행한다.
2. 모델 서버 명령과 Python lock 버전을 고정한다.
3. release profile, binary size와 panic 정책을 검토한다.
4. save v1 golden fixture와 migration policy를 게시한다.
5. example Pack을 `pack validate`와 100턴 test에 사용한다.
6. 실제 100턴 세션에서 server interruption과 app restart를 삽입한다.
7. `story-core` public API에서 UI·provider leak를 검사한다.
8. 알려진 제한·모델 license·모든 인물 Pack 원칙을 문서화한다.

### 산출물

- v1.0 release binary와 checksum
- 설치·사용·Pack authoring·troubleshooting 문서
- 예제 Pack
- 100턴 release evidence
- 호환성 정책과 changelog

### 테스트 방법

- clean install smoke
- unit/integration/snapshot/soak 전체
- 실제 모델 100턴과 중간 서버 종료·재시작
- Terminal.app/iTerm2 final checklist
- save v1 재시작·migration
- 두 번째 Pack 코드 변경 없는 실행

### 완료 조건

PRD의 v1.0 조건을 모두 만족하고, 미충족 항목을 “알려진 제한”으로 옮겨 완료 처리하지 않는다. 특히 실제 100턴, 데이터 복구, Pack 교체와 코어 독립성 증거가 필요하다.

### 예상 위험

- 실제 100턴의 실행 비용과 비결정성
- 모델·Python 패키지 업데이트로 문서가 빠르게 낡음
- unsigned binary 경고와 배포 경험

### 다음 단계 진입 조건

v1.0 태그 이후 별도 roadmap에서 웹 렌더러, 벡터 검색, 이미지·음성을 평가한다.

## 15. 단계별 검증 산출물 규칙

각 milestone result 문서에는 다음을 포함한다.

- commit 또는 release 식별자
- 하드웨어, macOS, Rust, Python, 서버와 모델 버전
- 실행한 자동 테스트와 결과
- 실제 runtime 경로와 재현 명령
- 실패·fallback·미검증 범위
- 성능·품질 표본 수
- 다음 단계 진입 여부와 근거

빌드 성공은 runtime proof를 대체하지 않는다. 실제 모델을 실행하지 못한 단계는 그 사실을 명시하고 해당 게이트를 열지 않는다.

