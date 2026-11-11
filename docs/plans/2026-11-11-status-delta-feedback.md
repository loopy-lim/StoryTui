# 계획: 스토리 화면 상태 변화량(Delta) 시각화 피드백

작성: 2026-11-11 · 상태: 완료 (G0 요구, G1 계획, G2 구현, G3 코드리뷰, G4 디자인/사용성, G5 완료 게이트 전체 PASS)

## 1. 배경 및 문제 정의 (G0)

StoryTUI의 스토리 화면 하단 상태 바는 `애정:3  신뢰:1  긴장:8  턴: 1`과 같이 현재 시점의 절대 수치만 표시하고 있다.
플레이어가 선택지를 확정했을 때, 직전 턴의 결과로 호감도·신뢰도·긴장도가 어떻게 변했는지(상승/하강) 즉각 알기 어려워 서사적 피드백의 몰입감이 떨어진다.

각 턴의 엔진 전이 결과인 `HistoryEntry.applied_patch`에는 모델이 제시하고 엔진이 검증한 `affection_delta`, `trust_delta`, `tension_delta`가 정확히 기록되어 있다. 이를 `build_view`를 통해 뷰 모델 `StatusLine`으로 전달하고, 0이 아닌 유의미한 변화가 있을 때 직관적인 색상 뱃지(예: `애정:3 (+1)`)로 렌더링한다.

## 2. 수용 기준 (AC)

- **AC-1 (수치 델타 추출)**: `story-core`의 `build_view`는 `game.history.last()`의 `applied_patch`에서 대표 캐릭터(primary character)의 `affection_delta`, `trust_delta`, `tension_delta`를 추출하여 `StatusLine`의 필드(`affection_delta`, `trust_delta`, `tension_delta`)에 전달한다. 턴 0(오프닝, 히스토리 없음)이거나 델타가 없는 경우 `None`이다.
- **AC-2 (시각화 및 색상 렌더링)**: `story-tui`의 `status_values_line`은 델타가 존재하고 0이 아닐 때 수치 뒤에 ` (+N)` 또는 ` (-N)` 형태로 렌더링한다. 양수(+) 변화는 `Palette::SUCCESS` (녹색), 음수(-) 변화는 `Palette::DANGER` (빨간색)으로 구분한다. 변화량이 0이거나 `None`인 수치는 기존과 동일하게 숫자만 표시한다.
- **AC-3 (80열 레이아웃 예산 준수)**: 3개 수치 모두에 델타가 표시되더라도 전체 상태 바 폭은 약 41~45열로, 최소 터미널 규격인 80열 내에서 35열 이상의 안전 마진을 유지하며 어떠한 클리핑이나 줄바꿈 왜곡도 발생하지 않는다.
- **AC-4 (되돌리기/히스토리 연동 및 결정론적 일관성)**: 턴 되돌리기(`u`), 히스토리 삭제/편집, 세이브 불러오기 시 `game.history.last()`가 갱신되면 직전 턴에 해당하는 델타가 정확하게 반영된다.
- **AC-5 (품질 게이트 및 하위 호환)**:
  - `cargo test --workspace` 통과 (기존 테스트 및 신규 델타 렌더링 단위/스냅샷 테스트 포함).
  - `cargo clippy --workspace --all-targets -- -D warnings` 무경고 통과.
  - `cargo fmt --all -- --check` 통과.
  - `./scripts/check-core-deps.sh` (ADR-003: `story-core`는 UI/reqwest에 의존하지 않음).

## 3. 설계 (G1)

### 3.1 모델 레이어 (`crates/story-model/src/view.rs`)
`StatusLine`에 3개 필드를 추가:
```rust
pub struct StatusLine {
    pub route: Option<String>,
    pub affection: Option<i16>,
    pub trust: Option<i16>,
    pub tension: Option<i16>,
    pub turn: u32,
    #[serde(default)]
    pub affection_delta: Option<i16>,
    #[serde(default)]
    pub trust_delta: Option<i16>,
    #[serde(default)]
    pub tension_delta: Option<i16>,
}
```
`#[serde(default)]`로 기존 직렬화 데이터와의 하위 호환성을 100% 보장한다.

### 3.2 코어 레이어 (`crates/story-core/src/engine.rs`)
`build_view`에서 최신 히스토리 엔트리의 `applied_patch`로부터 primary character의 델타를 추출:
```rust
let (affection_delta, trust_delta, tension_delta) = game.history.last()
    .and_then(|entry| primary.map(|id| (
        entry.applied_patch.affection_delta.get(id).copied(),
        entry.applied_patch.trust_delta.get(id).copied(),
        entry.applied_patch.tension_delta.get(id).copied(),
    )))
    .unwrap_or((None, None, None));
```
이를 `StatusLine` 생성자에 매핑.

### 3.3 TUI 레이어 (`crates/story-tui/src/screens/story.rs`)
`status_values_line`에서 각 stat 항목에 델타 뱃지 생성:
```rust
let stat = |label: &'static str, v: Option<i16>, delta: Option<i16>, color: Color| -> Vec<Span<'static>> {
    let val = match v {
        Some(n) => n.to_string(),
        None => "—".to_string(),
    };
    let mut spans = vec![
        Span::styled(format!("{label}:"), Styles::subtle()),
        Span::styled(val, Style::default().fg(color).add_modifier(Modifier::BOLD)),
    ];
    if let Some(d) = delta && d != 0 {
        let (d_str, d_color) = if d > 0 {
            (format!("(+{d})"), Palette::SUCCESS)
        } else {
            (format!("({d})"), Palette::DANGER)
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            d_str,
            Style::default().fg(d_color).add_modifier(Modifier::BOLD),
        ));
    }
    spans
};
```

## 4. 태스크 분해 (T-*)

- **T-1 (Backend/Core Engineer)**: `crates/story-model/src/view.rs`의 `StatusLine` 정의 확장 및 `crates/story-core/src/engine.rs`의 `build_view`에서 델타 추출 로직 구현.
- **T-2 (Frontend/UI Engineer)**: `crates/story-tui/src/screens/story.rs`의 `status_values_line` 델타 시각화 렌더링 구현 및 `Palette::SUCCESS` / `Palette::DANGER` 스타일 적용.
- **T-3 (Frontend/UI Engineer)**: `crates/story-tui/src/app.rs` 및 테스트 코드들의 `StatusLine` 생성부 보정.
- **T-4 (Verification/Test Engineer)**:
  - `crates/story-core/tests/reduce.rs` 등에 턴 진행 시 `StatusLine` 델타 반영 검증 테스트 추가.
  - `crates/story-tui/tests/snapshots.rs` 또는 신규 단위 테스트로 델타 유무에 따른 렌더링 및 80열 폭 검증.
  - 전체 게이트 실행: `fmt`, `clippy`, `test`, `check-core-deps.sh`.

## 5. 검증 계획 및 결과 (V-*)
 
- **V-1**: `cargo test -p story-core` — **PASS** (180+ tests passed, `build_view_extracts_relationship_deltas_from_latest_history_entry` 포함).
- **V-2**: `cargo test -p story-tui` — **PASS** (188 tests passed, `status_values_line_renders_deltas_with_semantic_colors` 및 스냅샷 전체 통과).
- **V-3**: `cargo clippy --workspace --all-targets -- -D warnings` — **PASS** (경고 0건).
- **V-4**: `cargo fmt --all -- --check` — **PASS** (포맷팅 준수).
- **V-5**: `./scripts/check-core-deps.sh` — **PASS** (`story-core` 의존성 경계 ADR-003 유지).
- **V-6**: `cargo test --workspace` — **PASS** (전체 65 스위트 1070+ 테스트 통과, 0 failed).

## 6. 관문 판정 요약

| 관문 | 직군 | 판정 | 주요 확인 사항 |
|---|---|---|---|
| G0 요구 | Product Manager, Requirements Auditor | PASS | AC-1..5 확정 및 범위 식별 |
| G1 설계 | System Architect, UX/Design Auditor | PASS | 모델/코어/TUI 3계층 설계 및 80열 폭 예산 검증 |
| G2 구현 | Backend/Core Engineer, Frontend/UI Engineer | PASS | T-1..4 구현 완료, diff 정밀 검증 |
| G3 코드 | Staff Code Reviewer | PASS | 결함 없음, 하위 호환성(`serde(default)`) 및 클리피 무경고 |
| G4 사용성 | UX/Design Auditor | PASS | 80열 터미널 가독성(폭 40~45열로 여유 35열+), 양수/음수 시맨틱 색상 구분, 0/None 무노이즈 |
| G5 완료 | Release Gatekeeper | PASS | 원 요구조건 대비 AC-1..5 전체 충족 증거 확보 |

