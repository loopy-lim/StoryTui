# StoryTUI 기술 명세서

문서 버전: 1.0-draft  
작성 기준일: 2026-07-31  
기준 하드웨어: Apple Silicon M1 Max, 통합 메모리 64GB  
규범 키워드: “해야 한다”는 필수, “권장한다”는 기본안, “검토한다”는 미확정 선택지다.

## 1. 설계 요약

StoryTUI는 모델의 자연어 생성과 게임의 권위 있는 상태 변경을 분리한다. LLM 응답은 제안이며, Rust의 검증기·규칙 엔진·reducer를 통과한 결과만 게임 상태가 된다.

```text
사용자 입력
   |
   v
story-tui ── AppCommand ──> story-core ── GenerationRequest ──> story-provider
   ^                           |                                  |
   |                           v                                  v
AppEvent <── 렌더 상태 ── validated GameState              로컬 MLX 서버
                               |
                               v
                        story-persistence
```

핵심 불변식은 다음과 같다.

1. `story-core`는 Ratatui, Crossterm, Reqwest와 파일 시스템을 알지 못한다.
2. `GameState`는 단일 권위 소스이며 LLM 대화 문맥은 캐시·표현에 불과하다.
3. `StatePatch`는 전체 검증 후 원자적으로 적용한다.
4. 생성 중 상태와 마지막 안정 상태를 분리한다.
5. Provider와 저장소는 포트(trait)이며 구체 구현은 앱 조립 계층에서 주입한다.
6. 최종 한국어 본문은 영어 본문의 번역 단계 없이 Writer가 직접 생성한다.

## 2. 공식 정보 확인 결과와 초기 호환성 결정

### 2.1 초기 모델에 대한 중요 주의

실제 로컬 모델 ID와 모델 카드는 Git에 기록하지 않는다. 모델이 MLX VLM이면
`mlx_vlm.server`, 텍스트 모델이면 `mlx_lm.server`를 선택한다. MLX-VLM 공식
서버는 `/v1/chat/completions`, `/health`, `top_k`, 스트리밍과 JSON Schema
구조 출력을 문서화하고 있다.

Rust의 설계는 서버 이름에 결합하지 않는다. `OpenAiCompatibleProvider`는 URL과 capability만 보고 동작하며, `mlx_lm.server`와 `mlx_vlm.server`는 같은 provider adapter로 처리한다.

M0 결정 절차:

1. ignored 로컬 설정의 모델로 텍스트-only 요청을 검증한다.
2. 모델·processor·chat template 로딩 실패 시 정확한 오류와 버전을 기록한다.
3. 같은 모델을 `mlx_lm.server`에서 억지로 로드하지 않는다.
4. 후보가 실패하면 Provider 구현은 유지하고 호환되는 MLX 텍스트 모델을 기준 fixture로 교체한다.
5. StoryTUI 배포 문서에서 MLX 서버 프로세스는 “지원 실행 예시”이지 앱에 내장된 구성요소가 아님을 명시한다.

### 2.2 확인한 버전

아래는 2026-07-31에 crates.io와 공식 프로젝트 문서에서 확인한 개발 시작점이다. `Cargo.toml`은 호환 범위(예: `ratatui = "0.30"`)를 사용하고, 재현 가능한 실제 버전은 `Cargo.lock`과 CI가 고정한다. 보안·회귀 검증 없이 자동 상향하지 않는다.

| 구성요소 | 확인 버전 | 책임 |
|---|---:|---|
| Rust stable | 1.97.1 | 2024 edition 빌드 툴체인. 워크스페이스 MSRV는 의존성 검토 후 1.88 이상으로 시작 |
| ratatui | 0.30.2 | 선언적 TUI 레이아웃, 위젯, 상태 기반 렌더링 |
| crossterm | 0.29.0 | raw mode, alternate screen, 키·resize 이벤트 |
| tokio | 1.53.1 | async runtime, task, channel, timeout, signal |
| reqwest | 0.13.4 | OpenAI-compatible HTTP와 스트리밍 바이트 수신 |
| serde / serde_json | 1.0.229 / 1.0.151 | 도메인·API·세이브 직렬화 |
| toml | 1.1.4+spec-1.1.0 | 앱 설정과 Story Pack manifest 파싱 |
| thiserror / anyhow | 2.0.19 / 1.0.104 | 라이브러리 typed error / 앱 경계 context |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23 | 구조화 로그와 필터링 |
| insta | 1.48.0 | TUI·프롬프트·오류 메시지 snapshot |
| async-trait | 0.1.91 | 초기 async provider/storage trait. 추후 native async trait 가능성 검토 |
| uuid / chrono | 1.24.0 / 0.4.45 | 안정 ID와 저장 시각 |
| unicode-width / unicode-segmentation | 0.2.2 / 1.13.3 | 한글 표시 폭, grapheme 단위 편집 |
| jsonschema | 0.49.2 | 외부 Story Pack·Director JSON Schema 검증 |
| tokio-util / futures-util | 0.7.19 / 0.3.33 | 취소 토큰과 스트림 조합 |
| sha2 / tempfile / semver | 0.11.0 / 3.27.0 / 1.0.28 | checksum, 원자 저장 보조, 버전 호환 |

의도적으로 `ratatui = "*"` 같은 비고정 선언은 사용하지 않는다. 버전 갱신 PR은 unit·snapshot·save migration·실제 모델 smoke를 별도로 통과해야 한다.

## 3. 워크스페이스와 crate 구조

MVP부터 5개 라이브러리 crate를 만드는 것은 경계가 명확하지만 빌드와 탐색 비용이 있다. 다음 구조를 기본으로 하되, `story-model`은 작아도 장기적인 스키마 안정성을 위해 독립시킨다.

```text
storytui/
├── Cargo.toml
├── rust-toolchain.toml
├── crates/
│   ├── story-model/
│   │   └── src/{lib.rs,state.rs,turn.rs,memory.rs,pack.rs,save.rs}
│   ├── story-core/
│   │   └── src/{lib.rs,engine.rs,reducer.rs,validator.rs,rules.rs,
│   │             memory_selector.rs,prompt_context.rs}
│   ├── story-provider/
│   │   └── src/{lib.rs,provider.rs,openai_compatible.rs,mock.rs,
│   │             recovery.rs,sse.rs}
│   ├── story-persistence/
│   │   └── src/{lib.rs,json_store.rs,atomic_write.rs,migration.rs,
│   │             checksum.rs}
│   └── story-tui/
│       └── src/{lib.rs,app.rs,event.rs,terminal_guard.rs,input.rs,
│                   screens/,widgets/,theme.rs}
├── apps/storytui/src/main.rs
├── stories/
├── config/
├── saves/                 # 기본은 gitignore
├── tests/{fixtures,contracts,soak}
└── benches/
```

### 3.1 `story-model`

- 공통 ID newtype, enum, `GameState`, `StatePatch`, 생성 계약과 Story Pack DTO를 소유한다.
- Serde 표현과 schema version을 정의한다.
- UI, HTTP, 실제 파일 I/O에 의존하지 않는다.
- 저장·API용 DTO와 내부 타입이 달라질 때 명시적 변환을 둔다.

### 3.2 `story-core`

- command를 받아 결정적 transition을 계산한다.
- 규칙 조건식, patch 검증, reducer, route·event·ending state machine을 소유한다.
- `MemorySelector`와 생성용 `PromptContext`를 만든다.
- 외부 효과는 `EngineEffect`로 반환한다. HTTP 호출이나 저장을 직접 수행하지 않는다.

### 3.3 `story-provider`

- `LlmProvider` trait, OpenAI-compatible 구현, Mock, JSON 복구와 SSE 해석을 소유한다.
- 모델 응답을 `GeneratedTurnDraft`/`DirectorPlan`/Writer chunk로 변환한다.
- 도메인 규칙을 판단하거나 `GameState`를 변경하지 않는다.

### 3.4 `story-persistence`

- JSON 세이브, checksum, 백업, 원자 쓰기와 migration을 소유한다.
- Story Pack 파일 로더를 함께 둘 수 있으나, 커지면 v1.0 이전에 `story-pack` 모듈로만 분리하고 별도 crate는 만들지 않는다.
- 저장 완료 전 세이브 목록에 새 버전을 노출하지 않는다.

### 3.5 `story-tui`

- `AppViewModel`을 Ratatui 위젯으로 그린다.
- 키를 도메인 의미의 `AppCommand`로 변환한다.
- 한글 grapheme 편집, scroll, focus, modal, 오류 상세를 담당한다.
- 상태 검증과 루트 판정은 구현하지 않는다.

### 3.6 `apps/storytui`

- 설정·Story Pack·Provider·저장소·로거·Tokio runtime을 조립한다.
- panic hook과 `TerminalGuard` 수명을 소유한다.
- `anyhow::Context`로 시작 실패 원인을 사용자 메시지와 로그에 연결한다.

## 4. 핵심 데이터 모델

ID는 문자열을 직접 섞지 않도록 newtype을 사용한다. 아래 코드는 스키마 초안이며 컴파일 가능한 방향을 보여준다.

```rust
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CharacterId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LocationId(pub String);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CharacterState {
    pub affection: i16,
    pub trust: i16,
    pub tension: i16,
    pub relationship_stage: RelationshipStage,
    pub current_emotion: Emotion,
    pub known_facts: BTreeSet<String>,
    pub address_form_overrides: BTreeMap<CharacterId, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameState {
    pub save_schema_version: u32,
    pub story_id: String,
    pub story_version: semver::Version,
    pub pack_schema_version: semver::Version,
    pub session_id: Uuid,
    pub current_chapter: String,
    pub current_scene_id: String,
    pub turn: u32,
    pub location: LocationId,
    pub time_slot: TimeSlot,
    pub active_route: Option<String>,
    pub characters: BTreeMap<CharacterId, CharacterState>,
    pub flags: BTreeMap<String, bool>,
    pub numbers: BTreeMap<String, i64>,
    pub strings: BTreeMap<String, String>,
    pub inventory: BTreeMap<String, u32>,
    pub occurred_events: BTreeSet<String>,
    pub recent_summaries: VecDeque<String>,
    pub memories: Vec<MemoryEntry>,
    pub history: Vec<HistoryEntry>,
    pub last_choice: Option<ChoiceRef>,
    pub revision: u64,
    pub stable_generation_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipStage {
    Stranger, Acquaintance, Friend, CloseFriend, RomanticInterest, Partner,
    Estranged,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub intent: String,
    pub tone: Option<String>,
    pub target: Option<CharacterId>,
    pub risk: RiskLevel,
    pub hidden_requirements: Vec<ConditionExpr>,
    pub preview: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParsedAction {
    pub action_type: ActionType,
    pub target: Option<CharacterId>,
    pub intent: Option<String>,
    pub tone: Option<String>,
    pub content: String,
    pub original_text: String,
    pub confidence: Option<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatePatch {
    pub affection_delta: BTreeMap<CharacterId, i16>,
    pub trust_delta: BTreeMap<CharacterId, i16>,
    pub tension_delta: BTreeMap<CharacterId, i16>,
    pub relationship_transitions: BTreeMap<CharacterId, RelationshipStage>,
    pub set_flags: BTreeMap<String, bool>,
    pub number_delta: BTreeMap<String, i64>,
    pub set_strings: BTreeMap<String, String>,
    pub inventory_delta: BTreeMap<String, i32>,
    pub add_events: BTreeSet<String>,
    pub next_location: Option<LocationId>,
    pub next_time_slot: Option<TimeSlot>,
    pub proposed_route: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeneratedTurn {
    pub generation_id: Uuid,
    pub narration: String,
    pub choices: Vec<Choice>,
    pub proposed_patch: StatePatch,
    pub accepted_patch: StatePatch,
    pub summary: TurnSummary,
    pub warnings: Vec<GenerationWarning>,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: Uuid,
    pub turn: u32,
    pub scene_id: String,
    pub created_at: DateTime<Utc>,
    pub narration: String,
    pub player_action: Option<ParsedAction>,
    pub choices: Vec<Choice>,
    pub applied_patch: StatePatch,
    pub generation_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Uuid,
    pub kind: MemoryKind,
    pub facts: Vec<MemoryFact>,
    pub interpretation: Option<String>,
    pub participants: BTreeSet<CharacterId>,
    pub location: Option<LocationId>,
    pub importance: u8,
    pub unresolved: bool,
    pub source_history_ids: Vec<Uuid>,
    pub created_turn: u32,
    pub last_recalled_turn: Option<u32>,
}
```

`MemoryFact`는 최소 `subject`, `predicate`, `object`, `known_by`, `certainty`를 가진다. `facts`와 `interpretation`을 분리해 요약의 추론을 사실처럼 재주입하지 않는다.

## 5. 명령, 효과와 엔진 경계

```rust
pub enum EngineCommand {
    StartNewGame { story_id: String },
    SubmitChoice { choice_id: String },
    SubmitFreeAction { action: ParsedAction },
    AcceptGeneration { draft: GeneratedTurnDraft },
    CancelGeneration { generation_id: Uuid },
    RegenerateCurrentTurn,
    RestoreCheckpoint { checkpoint_id: Uuid },
}

pub enum EngineEffect {
    RequestGeneration(GenerationRequest),
    PersistStableState(SaveIntent),
    PublishView(AppViewModel),
    ShowRecoverableError(DomainErrorView),
}

pub struct Transition {
    pub next: GameState,
    pub effects: Vec<EngineEffect>,
    pub audit: Vec<StateChangeLog>,
}
```

`story-core`의 공개 함수는 `reduce(state, command, rules) -> Result<Transition, CoreError>` 형태를 유지한다. 생성 요청은 효과이고, 응답 수락은 새로운 command다. 따라서 Mock 테스트가 HTTP 없이 전체 턴을 재현할 수 있다.

## 6. Story Pack 규격

### 6.1 디렉터리와 책임

```text
stories/rainy-reunion/
├── story.toml       # 식별자, 버전, 장르, 등급, 초기 상태, 파일 참조
├── characters.toml  # 캐릭터 불변 설정, 관계, 말투, 지식
├── rules.toml       # 변수·장소 허용 목록, 범위, 전이, 엔딩
├── events.toml      # 핵심 사건과 발동 조건·효과
├── world.md         # 세계관·장소·사회적 사실
├── style.md         # 한국어 문체, 금지 습관, 장면 리듬
├── opening.md       # 첫 장면의 고정 사실과 도입
└── prompts/
    ├── system.md    # 절대 역할·안전·출력 계약
    ├── director.md  # 계획·충돌 검사·patch 제안
    └── writer.md    # 확정 계획을 한국어 장면으로 실현
```

### 6.2 `story.toml` 예

```toml
schema_version = "1.0.0"
story_id = "rainy-reunion"
story_version = "0.1.0"
min_engine_version = "0.1.0"
title = "비가 그친 뒤"
language = "ko-KR"
genres = ["modern-romance", "drama"]
rating = "teen"
default_choice_count = 3

[initial_state]
chapter = "chapter-1"
scene = "cafe-reunion"
location = "cafe-namu"
time_slot = "evening"

[content]
violence = "mild"
```

### 6.3 `characters.toml` 예

```toml
schema_version = "1.0.0"

[[characters]]
id = "yuna"
name = "한유나"
age = 29
pronouns = "그녀"
personality = ["관찰력이 좋다", "상처를 농담으로 감춘다"]
appearance = ["짧은 검은 머리", "은색 손목시계"]
values = ["솔직함", "약속"]
likes = ["비 온 뒤 냄새", "오래된 재즈"]
dislikes = ["일방적인 잠수", "감정의 강요"]
secrets = ["3년 전 떠난 이유를 아직 말하지 않았다"]
initial_player_relationship = "대학교 동아리 친구, 3년간 연락 두절"
speech_style = "짧고 정확한 문장. 당황하면 말끝을 흐린다."
honorific_rule = "초기에는 해요체. 신뢰 15 이상이고 event.reconciled 이후 반말 가능."
frequent_phrases = ["그건 네 해석이고.", "일단 앉아."]
avoid_phrases = ["운명", "심장이 미친 듯이"]
known_facts = ["player가 약속 장소에 늦었던 사실"]
unknown_facts = ["player가 여러 번 연락을 시도했던 사실"]
current_goal = "재회의 이유를 확인한다"
long_term_desire = "일방적으로 사라지지 않는 관계"
```

감정별 예문과 타 인물 관계는 array of tables로 추가한다. `secrets`는 프롬프트에 무조건 넣지 않고 Director의 `must_not_reveal` 또는 지식 조건과 함께 선택한다.

### 6.4 스키마 검증과 migration

로드 순서:

1. UTF-8·파일 크기·허용 파일명 검사
2. TOML/Markdown 파싱
3. `schema_version` major 호환 검사
4. ID 유일성, 참조 무결성, 범위·enum 검사
5. 이벤트·루트 조건식 컴파일
6. 도달 불가능 장면·순환 전이 경고
7. 지원 등급 검사
8. canonical `StoryPack`으로 변환

같은 major의 minor 변화는 기본값을 채우는 in-memory migration으로 처리한다. major 변화는 명시적 `PackMigrator<V1,V2>`가 없으면 거부한다. 원본 Pack을 자동 덮어쓰지 않고 `storytui pack migrate --out <new-dir>` 같은 별도 명령으로 복사본을 생성하는 방식을 권장한다.

## 7. 규칙 표현

MVP는 임의 스크립트를 실행하지 않는다. 조건은 직렬화 가능한 제한 DSL로 표현한다.

```rust
pub enum ConditionExpr {
    All(Vec<ConditionExpr>),
    Any(Vec<ConditionExpr>),
    Not(Box<ConditionExpr>),
    FlagEquals { id: String, value: bool },
    NumberCompare { id: String, op: CompareOp, value: i64 },
    AffectionAtLeast { character: CharacterId, value: i16 },
    TrustAtLeast { character: CharacterId, value: i16 },
    EventOccurred { id: String },
    AtLocation(LocationId),
    TurnAtLeast(u32),
}
```

효과는 `StatePatch`의 허용된 부분집합으로 선언한다. 조건 평가에는 부작용이 없고, 동일 입력에 동일 결과를 반환한다. 이벤트 우선순위가 같으면 Story Pack 순서가 아니라 안정 ID 정렬을 사용한다.

## 8. StatePatch 검증과 reducer

### 8.1 검증 파이프라인

1. **구조 검증**: unknown field, enum, 길이, 선택지 수
2. **참조 검증**: 캐릭터·장소·플래그·변수·아이템 존재
3. **범위 검증**: delta·최종 값·본문·요약 크기
4. **권한 검증**: 모델이 바꿀 수 없는 필드 차단
5. **전이 검증**: 관계 단계, 장소, 시간, route·event 조건
6. **교차 검증**: 선택 의도·Director plan·patch 간 모순
7. **dry-run**: 복제 상태에 reducer 적용 후 전체 불변식 검사
8. **commit**: revision을 1 증가시키며 새 상태 교체

### 8.2 기본 정책

- 일반 `affection_delta`, `trust_delta`, `tension_delta`: 각 -3~+3
- 캐릭터 상태 최종 범위: 기본 -100~+100
- 없는 ID: 전체 patch 거부
- 없는 플래그: 거부하고 `PATCH_UNKNOWN_FLAG` 진단
- 장소 전이: `rules.toml`의 adjacency 또는 이벤트 override 필요
- 관계 단계: 한 단계 전이가 기본. event가 허용하면 건너뛰기 가능
- route 제안: 조건이 참일 때만 reducer가 확정
- ending: patch 필드로 받지 않는다. reducer가 event 후 별도 판정
- inventory 수량: 음수가 되면 거부
- 문자열 변수: Pack이 선언한 최대 길이와 enum을 적용

전체 patch가 실패하면 이전 상태를 그대로 유지한다. 안전하게 부분 적용할 수 있는 “cosmetic warning”은 patch가 아니라 생성 경고로 분리한다. 자동으로 delta를 clamp하면 모델 오류가 숨겨지므로 기본은 거부이며, 복구 요청에서 허용 범위를 명시해 다시 생성한다.

### 8.3 상태 변경 로그

```rust
pub struct StateChangeLog {
    pub id: Uuid,
    pub generation_id: Uuid,
    pub turn: u32,
    pub source: ChangeSource,
    pub field_path: String,
    pub before: serde_json::Value,
    pub proposed: serde_json::Value,
    pub applied: Option<serde_json::Value>,
    pub decision: ChangeDecision,
    pub rule_id: Option<String>,
    pub reason_code: Option<String>,
}
```

본문은 로그에 넣지 않는다. 로그가 세이브에 포함될 경우 사용자가 내보낼 수 있는 별도 진단 파일과 구분한다.

## 9. 선택지와 자유 입력

### 9.1 선택지 검증

- 개수 2~4(MVP 정확히 3)
- ID 유일, label 공백 제외 8~80자 권장
- `intent` 유일성이 부족하면 경고 또는 repair 요청
- 같은 target·intent·risk·결과 힌트가 반복되면 가짜 선택 후보로 점수화
- `hidden_requirements`는 모델이 임의 생성하지 않고 Director가 Pack 조건 ID를 참조
- preview는 확정 결과를 누설하지 않고 태도·위험만 설명

### 9.2 자유 입력 편집

입력 버퍼는 Unicode scalar가 아니라 grapheme cluster 기준으로 커서를 이동·삭제한다. 터미널에서 IME 조합 중간 상태를 완전히 통제할 수 없으므로 Crossterm이 확정한 `KeyEvent`만 버퍼에 반영한다. 붙여넣기는 bracketed paste를 지원할 수 있으면 별도 이벤트로 받고 최대 길이·제어 문자를 검사한다.

규칙 기반 사전 해석은 따옴표, “말한다/묻는다/간다/건넨다” 같은 동사, 알려진 인물·장소 이름을 사용한다. 확신이 없으면 다음처럼 보존한다.

```json
{
  "action_type": "general_action",
  "target": null,
  "intent": null,
  "tone": null,
  "content": "사실 몇 번이나 연락하려 했어",
  "original_text": "사실 몇 번이나 연락하려 했어",
  "confidence": null
}
```

`$EDITOR` 연동은 MVP 이후다. 실행 전 raw mode를 해제하고 alternate screen을 떠난 뒤 child process 종료 결과와 임시 파일을 읽고 TUI를 복원한다. 임시 파일은 사용자 승인 없이 장기 보관하지 않는다.

## 10. LLM Provider

### 10.1 trait

```rust
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> ProviderCapabilities;

    async fn health(&self) -> Result<ProviderHealth, ProviderError>;

    async fn generate_turn(
        &self,
        request: TurnRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<GeneratedTurnDraft, ProviderError>;

    async fn direct(
        &self,
        request: DirectorRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<DirectorPlan, ProviderError>;

    async fn write_stream(
        &self,
        request: WriterRequest,
        sink: tokio::sync::mpsc::Sender<WriterEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<WriterFinish, ProviderError>;
}
```

MVP 구현은 `generate_turn`만 필요하고 나머지는 `UnsupportedCapability`를 반환할 수 있다. `MockProvider`는 fixture queue, 의도한 지연, malformed 응답과 중간 실패를 주입할 수 있어야 한다.

### 10.2 설정

```toml
[provider]
kind = "openai_compatible"
name = "local-mlx-vlm"
base_url = "http://127.0.0.1:8080"
chat_path = "/v1/chat/completions"
health_path = "/health"
model = "local-model-id"
temperature = 0.85
top_p = 0.92
top_k = 40
max_tokens = 1800
request_timeout_ms = 180000
connect_timeout_ms = 3000
max_retries = 2
context_budget_tokens = 24576
structured_output = "auto"
```

`context_budget_tokens`는 모델이 주장하는 최대 context가 아니라 M0 실측을 바탕으로 한 앱 예산이다. prompt 구성은 `fixed + state + memory + recent + response_reserve <= context_budget_tokens`를 만족해야 한다.

### 10.3 MVP 요청

```json
{
  "model": "local-model-id",
  "messages": [
    {"role": "system", "content": "<Story Pack system + 출력 계약>"},
    {"role": "user", "content": "<현재 상태와 다음 장면 요청>"}
  ],
  "temperature": 0.85,
  "top_p": 0.92,
  "top_k": 40,
  "max_tokens": 1800,
  "stream": false,
  "response_format": {
    "type": "json_schema",
    "json_schema": {
      "name": "generated_turn",
      "strict": true,
      "schema": "<capability 확인 시에만 포함>"
    }
  }
}
```

서버가 `response_format`을 거부하면 capability를 세션 동안 비활성화하고 프롬프트 JSON 계약+클라이언트 검증으로 fallback한다. HTTP 4xx를 무조건 재시도하지 않는다.

### 10.4 MVP 응답 계약

```json
{
  "narration": "한국어 장면 본문",
  "choices": [
    {
      "id": "choice-1",
      "label": "젖은 우산을 접고 유나에게 먼저 사과한다",
      "intent": "direct_apology",
      "tone": "sincere",
      "target": "yuna",
      "risk": "medium",
      "hidden_requirements": [],
      "preview": "관계를 정면으로 다룬다"
    }
  ],
  "state_patch": {
    "affection_delta": {"yuna": 1},
    "trust_delta": {"yuna": 1},
    "tension_delta": {"yuna": -1},
    "relationship_transitions": {},
    "set_flags": {"apologized": true},
    "number_delta": {},
    "set_strings": {},
    "inventory_delta": {},
    "add_events": [],
    "next_location": null,
    "next_time_slot": null,
    "proposed_route": null
  },
  "summary": {
    "facts": ["플레이어가 유나에게 먼저 사과했다"],
    "unresolved": ["유나가 3년 전 떠난 이유"],
    "relationship_changes": ["사과로 긴장이 조금 낮아졌다"],
    "knowledge_changes": []
  }
}
```

### 10.5 Director/Writer 계약

Director는 완전한 JSON만 반환한다.

```rust
pub struct DirectorPlan {
    pub scene_goal: String,
    pub required_beats: Vec<String>,
    pub emotion_arcs: BTreeMap<CharacterId, EmotionArc>,
    pub reveal_fact_ids: Vec<String>,
    pub must_not_reveal_fact_ids: Vec<String>,
    pub proposed_patch: StatePatch,
    pub choices: Vec<Choice>,
    pub conflict_checks: Vec<ConflictCheck>,
    pub end_condition: String,
}
```

Director plan을 검증·확정한 뒤 Writer에는 `accepted_patch`가 아니라 독자가 알아야 할 결과와 확정 plan을 전달한다. Writer는 본문만 생성하며 상태·선택지를 새로 만들 수 없다. 스트리밍 중 본문은 임시 UI buffer에만 존재하고 `WriterFinish`와 lint 통과 후 history에 commit한다.

## 11. JSON 복구, 재시도와 부분 실패

복구 단계는 반드시 단계·원문 hash·오류 코드를 기록한다.

1. 원 응답을 엄격한 Serde 타입으로 파싱
2. 단일 Markdown code fence와 앞뒤 공백만 제거 후 재파싱
3. 안전한 경미 수정: UTF-8 BOM 제거, 명백한 trailing comma 등 승인된 변환만 적용
4. 모델에 원문 전체가 아니라 schema 오류 경로와 응답을 보내 JSON repair 1회
5. 원 generation 요청 전체 재시도 1회
6. 한국어 오류와 `다시 시도/진단/체크포인트 복구` 제공
7. 마지막 안정 상태 유지

기본 상한은 최초 1회 + repair 1회 + 전체 재시도 1회다. connect timeout 3초, 전체 요청 180초를 시작값으로 하며 구성 가능하다. 취소는 `CancellationToken`을 전달하고 task를 abort하더라도 generation ID가 다르면 늦은 결과를 폐기한다.

다음은 자동 복구하지 않는다.

- 존재하지 않는 캐릭터·장소를 patch가 참조
- 허용 범위를 크게 넘는 delta
- 조건을 만족하지 않는 route·ending
- narration이 비어 있음
- 4개를 초과한 선택지 중 임의 일부 선택

스트리밍 Writer가 중간에 끊기면 부분 본문을 `aborted_generation` 진단에 메모리 내 보관할 수 있으나 확정 history와 세이브에는 넣지 않는다. 사용자에게 부분 표시를 유지할지 지울지 선택권을 줄 수 있다.

## 12. 계층형 기억과 Memory Selector

### 12.1 기억 계층

- **고정 기억**: world, 캐릭터 불변 설정, 절대 규칙, 현재 route 전제
- **상태 기억**: location, time, 관계 수치·단계, flags, inventory, 현재 목표
- **최근 기억**: 최근 N턴 원문, 직전 행동, 열린 대화
- **요약 기억**: 장면·챕터·관계 변화·미해결 갈등 요약
- **사건 기억**: 고백·약속·배신·비밀 공개·첫 만남·단계 변화·중요 물건

### 12.2 선택 규칙

예산 배분 시작값:

| 계층 | 최소 보장 | 선택 규칙 |
|---|---:|---|
| 시스템·출력 계약 | 12% | 항상 포함, 중복 문장 제거 |
| 고정 기억 | 18% | 현재 장면 참여자·route 관련 항목 |
| 현재 상태 | 10% | canonical compact JSON |
| 최근 원문 | 28% | 현재 장면 우선, 최근 4~6턴 |
| 사건·요약 | 22% | 관련 인물·장소·미해결·중요도 |
| 응답 여유 | 별도 10% 이상 | `max_tokens`와 안전 마진 |

점수 예:

```text
score =
  importance * 4
  + unresolved * 8
  + participant_match * 5
  + location_match * 3
  + route_match * 6
  + explicit_reference * 10
  + recency_bucket
  - already_recalled_recently * 3
```

절대 포함 규칙은 점수보다 우선한다: 현재 route 전제, 진행 중 약속, 관계 단계 변화 원인, 현재 참여자의 호칭 규칙, 이번 장면에서 언급된 고유명사, 아직 해결되지 않은 핵심 사건.

### 12.3 요약 생성·검증

요약은 `facts`, `interpretation`, `unresolved`, `knowledge_changes`, `address_form_changes`, `important_player_actions`, `source_history_ids`를 분리한다. 모델 요약이 원본에 없는 fact를 추가하면 거부한다. MVP는 턴 응답의 summary를 검증해 사용하고, M6에서는 장면 경계에서 별도 요약 task를 실행할 수 있다.

### 12.4 벡터 검색 도입 조건

다음 중 둘 이상이 지속되면 ADR을 새로 작성한다.

- 100턴 프롬프트에서 규칙 기반 selector의 중요 기억 회수율이 90% 미만
- Story Pack의 named fact가 500개 이상
- 등장인물 10명 이상, 장소 30개 이상에서 관련성 규칙 유지 비용 급증
- 사용자가 오래된 비정형 자유 행동을 자주 참조
- 벡터 도입이 latency·메모리·설치 복잡도 대비 블라인드 품질을 유의미하게 개선

벡터 DB는 canonical state의 대체물이 아니며 후보 검색에만 사용한다.

## 13. 저장 시스템

### 13.1 저장 envelope

```rust
pub struct SaveEnvelope {
    pub save_schema_version: u32,
    pub engine_version: semver::Version,
    pub story_id: String,
    pub story_version: semver::Version,
    pub pack_schema_version: semver::Version,
    pub slot_id: String,
    pub description: Option<String>,
    pub saved_at: DateTime<Utc>,
    pub chapter_label: String,
    pub scene_label: String,
    pub checksum_algorithm: String,
    pub payload_checksum: String,
    pub payload: SavePayload,
}

pub struct SavePayload {
    pub stable_state: GameState,
    pub memories: Vec<MemoryEntry>,
    pub history: Vec<HistoryEntry>,
    pub audit_tail: Vec<StateChangeLog>,
}
```

프롬프트 전체, provider API key, 불완전 Writer buffer와 기본 로그는 저장하지 않는다.

### 13.2 원자 저장

1. payload를 canonical JSON bytes로 직렬화
2. SHA-256 계산
3. 목적 파일과 같은 디렉터리에 고유 임시 파일 생성
4. bytes 작성, flush, 파일 sync
5. 기존 목적 파일을 `.bak`으로 교체(정책에 따라 1~3세대)
6. 임시 파일을 목적 파일로 atomic rename
7. 가능한 플랫폼에서 부모 디렉터리 sync
8. 파일을 다시 열어 checksum·schema·최소 필드 검증
9. `SaveFinished` 이벤트 발행

실패하면 기존 목적 파일과 stable state를 유지한다. rename의 원자성은 같은 파일시스템을 전제로 하므로 임시 파일은 같은 디렉터리에 둔다.

### 13.3 로드와 migration

checksum → JSON syntax → envelope version → Story Pack 존재·호환 → 순차 migration → 도메인 불변식 → 임시 state로 복원 → 사용자 확인 → 활성 state 교체 순서다. 로드 실패가 현재 게임을 덮어쓰면 안 된다.

## 14. TUI 상태와 이벤트 루프

### 14.1 화면

- `Story`: 제목·챕터·장소·시간, 본문, 선택지, 상태줄
- `FreeInput`: 다중 줄 편집, 문자 수, 취소·제출
- `History`: 턴별 본문·행동·patch 접기
- `Character`: 관계 수치, 단계, 현재 감정, 알려진 사실(스포일러 제외)
- `GameState`: 위치·시간·route·flags·inventory
- `SaveLoad`: 슬롯 메타데이터, 복제·삭제
- `Settings`: provider·샘플링·키·테마. 비밀 값은 마스킹
- `ErrorDetail`: 한국어 요약, 코드, 복구 동작, 진단 복사

권장 키:

| 키 | 동작 |
|---|---|
| `↑/↓`, `j/k` | 선택지·목록 이동 |
| `Enter`, `1..4` | 선택 확정 |
| `i` | 자유 입력 |
| `r` | 현재 안정 상태에서 재생성 |
| `s` / `l` | 빠른 저장 / 빠른 불러오기 |
| `h` | History |
| `Tab`, `Shift+Tab` | 상태 화면 전환 |
| `c`, `g`, `,` | Character, Game State, Settings |
| `Esc` | 취소·이전 |
| `Ctrl+C` | 생성 중 취소, 두 번째 입력 시 종료 확인 |
| `q` | 종료 확인 |

### 14.2 이벤트

```rust
pub enum AppEvent {
    Key(crossterm::event::KeyEvent),
    Paste(String),
    Resize { width: u16, height: u16 },
    Tick,
    GenerationStarted { id: Uuid },
    GenerationChunk { id: Uuid, text: String },
    GenerationFinished { id: Uuid, result: GeneratedTurnDraft },
    GenerationFailed { id: Uuid, error: UserFacingError },
    SaveFinished { slot: String, result: Result<SaveMeta, UserFacingError> },
    LoadFinished { slot: String, result: Result<SavePayload, UserFacingError> },
    ProviderHealthChanged(ProviderHealth),
}
```

메인 loop는 한 번의 이벤트를 받아 앱 상태를 갱신하고 렌더한다. 모델 호출, 저장, health polling은 별도 Tokio task다. bounded `mpsc`로 backpressure를 적용한다. Writer chunk는 너무 자주 그리지 않도록 16~33ms 단위로 합칠 수 있다.

### 14.3 terminal 복원

`TerminalGuard::enter()`가 raw mode와 alternate screen을 설정하고 `Drop`에서 역순 복원한다. panic hook은 기존 hook을 보존하고 최소한 `disable_raw_mode`, `LeaveAlternateScreen`, cursor show를 시도한 뒤 panic 정보를 기록한다. signal handler도 정상 shutdown command를 보내되, 프로세스가 강제 종료된 경우 복원을 보장한다고 표현하지 않는다.

최소 크기 시작값은 80x24다. 이하에서는 단일 경고 화면만 그리며 resize 이벤트를 계속 받는다.

## 15. 오류 모델과 사용자 경험

### 15.1 분류

```rust
pub enum ErrorCategory {
    ProviderUnavailable, ProviderLoading, ProviderProtocol, Timeout,
    Cancelled, InvalidGeneration, StoryPack, Persistence, Migration,
    Terminal, Input, RuleViolation, Internal,
}

pub struct UserFacingError {
    pub code: &'static str,
    pub title_ko: String,
    pub message_ko: String,
    pub recoverable: bool,
    pub actions: Vec<RecoveryAction>,
    pub correlation_id: Uuid,
}
```

### 15.2 오류별 기본 UX

| 상황 | 사용자 메시지 요지 | 데이터 처리 | 복구 |
|---|---|---|---|
| 서버 미실행/주소 오류 | “로컬 모델 서버에 연결할 수 없습니다.” | stable state 유지 | 다시 확인, 설정 열기 |
| 모델 로딩 중 | “모델을 불러오는 중입니다.” | 요청 보류 안 함 | health 재확인 |
| timeout | “제한 시간 안에 응답이 오지 않았습니다.” | 체크포인트 유지 | 재시도, timeout 조정 |
| 스트림 종료 | “본문 생성 연결이 중간에 끊겼습니다.” | 부분 본문 미확정 | 다시 생성 |
| JSON 오류/필드 누락 | “모델 응답 형식을 복구하지 못했습니다.” | patch 미적용 | repair/재시도 |
| Pack 오류 | 파일·필드·줄 위치 표시 | 게임 시작 차단 | 진단 보기 |
| 세이브 손상 | checksum·파싱 단계 표시 | 현재 게임 유지 | `.bak` 복구 |
| 미래 세이브 버전 | “이 앱보다 새 버전의 세이브입니다.” | 로드 차단 | 앱 업데이트 안내 |
| 한글 입력 문제 | 입력 확정·터미널 설정 안내 | 원문 손실 방지 | 붙여넣기/향후 editor |
| 작은 터미널 | 필요한 크기 표시 | 상태 유지 | 창 확대 |
| 생성 취소 | “생성을 취소했습니다.” | 체크포인트 유지 | 재개 아님, 새 요청 |
| 장면 반복 | 유사도 경고 | 자동 patch 금지 | anti-repeat 지시 재생성 |
| 설정·지식 위반 | 위반 항목 표시 | 규칙 위반 patch 차단 | Director/Writer 재시도 |
| 없는 장소·인물 | 규칙 코드 표시 | patch 전체 롤백 | repair |
| route 무시 | “이 경로는 아직 열리지 않았습니다.” | route 미변경 | 규칙 기반 대안 생성 |

## 16. 로깅과 개인정보

기본 INFO 로그에 허용:

- app·engine·pack schema 버전
- provider host가 loopback인지 여부와 경로(쿼리·비밀 제외)
- 요청 ID, generation ID, 소요 시간, token usage
- HTTP status, 오류 category·code
- patch 필드 경로와 검증 결과(서술 값은 필요시 hash)
- 저장 슬롯 ID, bytes, migration 단계

기본 로그에서 금지:

- 전체 프롬프트, system prompt와 chat template
- 자유 입력 원문
- 생성 본문과 대사
- 세이브 payload와 캐릭터 비밀
- 인증 header·API key

`content_logging = true`는 개발자 명시 설정과 시작 시 경고가 있어야 한다. 그 경우에도 로그 디렉터리·보존 기간·삭제 명령을 보여주며 자동 업로드하지 않는다. `tracing` field에는 `sensitive` 값을 직접 넣지 않고 길이·hash·ID만 넣는다.

## 17. 보안

- 기본 URL은 `http://127.0.0.1:8080`; 서버도 `127.0.0.1`에 바인딩하도록 안내한다.
- MLX-LM 공식 문서도 서버의 보안 검사가 기본 수준이며 로컬 endpoint 용도라고 설명한다. 외부 공개 운영을 제품 범위로 보지 않는다.
- `localhost`가 임의 DNS로 해석되는 경우를 피하기 위해 기본은 숫자 loopback을 사용한다.
- loopback 이외 주소는 설정 화면과 첫 요청 전에 경고한다.
- URL의 userinfo, file scheme, 예상 밖 redirect는 거부한다. redirect는 기본 비활성화한다.
- Story Pack Markdown/TOML을 데이터로만 읽고 shell·Lua·JS를 실행하지 않는다.
- Pack의 상대 경로는 canonicalize 후 Pack root 밖으로 탈출하면 거부한다.
- `$EDITOR`는 사용자가 설정한 실행 파일만 명시적으로 실행하고 Story Pack 내용으로 command line을 만들지 않는다.
- 텔레메트리, crash upload와 원격 로그는 기본 제공하지 않는다.

## 18. 성능과 M1 Max 64GB 검증

M0 벤치 매트릭스:

- 입력 예산: 4K, 8K, 16K, 24K, 가능하면 32K tokens
- 출력: 512, 1024, 1800 tokens
- cold load / warm first request / warm repeat
- structured output on/off
- 모델: MLX 4bit, 이후 동일 계열 GGUF Q6_K, 필요 시 Q8_0

수집값:

- 모델 load time, RSS·통합 메모리 peak(측정 도구와 오차 명시)
- prompt tokens/s, generation tokens/s, TTFT, total latency
- JSON 1차 유효률, repair율, truncation률
- 한국어 평가와 20/50/100턴 일관성

M1 Max의 “64GB에 들어간다”는 모델 파일 크기만으로 단정하지 않는다. KV cache·Metal/MLX buffer·앱·OS 여유를 포함한 실측으로 context default를 정한다. 메모리 압박과 swap 발생은 품질이 아니라 사용성 실패로 별도 기록한다.

## 19. 테스트 전략

### 19.1 자동 기본 스위트

- 도메인 unit: 조건식, route, event, ending
- `StatePatch` 경계값·unknown ID·원자 rollback property test
- reducer의 동일 입력 결정성
- Story Pack good/bad fixture와 참조 무결성
- 저장 round-trip, checksum, 중간 쓰기 실패, `.bak` 복구
- v0→v1 migration과 미래 버전 거부
- Mock Provider 성공·timeout·취소·late response
- JSON fence·trailing comma·missing field·invalid enum·과다 선택지 복구
- TUI snapshot: 각 화면, loading, error, 80x24와 넓은 화면
- 한글 폭: 한글·ASCII·emoji·combining mark·CJK 혼합 줄바꿈
- 100턴 synthetic soak: 메모리 크기, 상태 불변식, save/load 반복

### 19.2 실제 모델 스위트

`#[ignore]` 또는 별도 `cargo test --test mlx_smoke -- --ignored`로 분리한다.

- `/health`와 모델 목록
- 한국어 한 턴 JSON
- 고정 fixture 10회 JSON 유효률
- 취소·timeout·서버 종료
- 10턴 최소 플레이
- Director JSON과 Writer stream(구현 단계 이후)

실제 모델 스위트는 일반 PR의 필수 unit test로 만들지 않는다. 릴리스 게이트와 지정된 로컬 하드웨어 작업에서 실행하고 모델·서버·chat template·seed·sampling을 결과에 기록한다.

### 19.3 품질 평가표

각 항목을 1~5점으로 블라인드 평가한다.

- 조사·어미, 번역체 억제, 반복 억제
- 말투 구분, 존댓말·호칭
- 장면 연결과 show-don't-tell
- 선택 의도 구분과 결과 반영
- 자유 입력 보존, 기억 회상
- 지식 범위, 관계 단계, 미해결 사건

평가자는 모델·양자화 이름을 보지 않는다. 순서를 무작위화하고 “더 문학적” 같은 단일 선호가 규칙 준수를 압도하지 않도록 항목별 점수를 유지한다.

## 20. 완료 시 기술 승인 기준

- `cargo test --workspace`가 네트워크·실제 모델 없이 통과
- 실제 MLX smoke가 별도 명령으로 통과
- `story-core` dependency tree에 Ratatui/Crossterm/Reqwest 없음
- 잘못된 patch가 상태에 일부라도 적용되는 테스트 사례 0
- 세이브 원자 쓰기와 backup 복구 fault injection 통과
- 100턴 soak 후 스키마·상태·기억 불변식 유지
- DOC에 명시한 한글 터미널 매트릭스 통과
- Pack 2개를 코드 수정 없이 교체 실행
- provider URL을 Mock/MLX/다른 OpenAI-compatible fixture로 교체 가능

## 21. 공식 참고 자료

- [Rust 공식 릴리스 목록](https://blog.rust-lang.org/releases/) - 2026-07-31 확인
- [crates.io](https://crates.io/) - Rust crate 최신 안정 버전 확인, 2026-07-31
- [MLX-LM 공식 저장소](https://github.com/ml-explore/mlx-lm)
- [MLX-LM HTTP 서버 문서](https://github.com/ml-explore/mlx-lm/blob/main/mlx_lm/SERVER.md)
- [MLX-VLM 공식 저장소와 서버 문서](https://github.com/Blaizzy/mlx-vlm)

이 문서의 버전·서버 capability는 변경될 수 있다. 구현 시작 시 `Cargo.lock`, Python lockfile, 실제 `--help` 출력과 smoke 결과를 저장해 다시 고정해야 한다.
