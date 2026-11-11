# 계획: Settings에서 단일 패스 스트리밍 토글 노출 (KNOWN-LIMITATIONS "not yet" 항목)

작성: 2026-11-09 · 상태: 완료 (독립 재리뷰 전체 PASS, 2026-11-09 · 잔여 한계는
2026-11-10 후속 라운드에서 전부 해소 · 3차 재리뷰 PASS + P3 해소 후 4차 확인
PASS — 잔여 결과물 없음, 2026-11-10)

## 3차 재리뷰 (2026-11-10, 잔여 한계 해소 델타)

**PASS** — AC-1..5 통과, 게이트 전항목 재실행 확인(fmt/clippy/test 65 스위트
1072 passed/check-core-deps/diff-check). 검토점 4건 모두 확인: (a) 토글 lockstep이
F1 수정과 무모순, R1 구조적 폐쇄 (b) stream_supported 계약 정확 (c) 푸터 80컬럼
검증 변형 정확 (d) R2 스테일 시드가 실제 미러 오염 시뮬레이션.

- **P3 1건 보고 → 즉시 해소**: 홈 메뉴 유입 변형의 Esc 힌트 "시작 메뉴"(9열)가
  81열로 [Q] 종료 마지막 1열 클리핑. 붙여쓰기 "시작메뉴"(8열)로 홈/게임 두 변형
  모두 정확히 80열. `snapshot_settings_loopback_ready`가 홈 변형 렌더링을 추가해
  `[Esc] 시작메뉴`·`[Q] 종료` 무클리핑을 명시 assert(기존 무커버 변형).
- **4차 확인(마이크로 델타) PASS**: 폭 80열 재계산 확인, `render_to_string` 참조화가
  스냅샷 불변(변이는 assert 이후) 확인, 81열 회귀 시 ratatui `LineTruncator`가 마지막
  글자를 렌더 전에 버려 assert가 실제로 실패함을 라이브러리 소스 차원에서 증명.
  게이트 전항목 재실행 통과(65 스위트 1072 passed / 0 failed). 잔여 결과물 없음.

## 잔여 한계 (독립 리뷰 P3, 비차단) — 2026-11-10 해소

- ~~**R1** 첫 실행 세션(모델 미지정)에서 자동 설정 채택 전 `s` 토글을 쓰면
  `reprobe_if_unconfigured`의 작업 사본이 시작 시점 값으로 되돌린다~~ — **해소**:
  host 토글 블록이 `provider_config`(작업 사본)을 `config.provider.stream_turns`와
  lockstep으로 유지하므로, reprobe/전환 경로가 `provider_config`에서
  `config.provider`를 재구성해도 낡은 값이 되살아나지 않는다.
- ~~2단계 경로 활성 중 `s`를 누르면 보이지 않게 값이 바뀐다~~ — **해소**: 행이
  저장된 선호를 "2단계 중 (켜짐/꺼짐)"으로 표시(MUTED)해 누름이 즉시 보인다.
- ~~행은 SSE 지원 여부와 무관하게 선호를 표시한다~~ — **해소**: 지원 미광고
  프로바이더 위 켜짐 선호는 "켜짐 · SSE 미지원"(CAUTION)으로 렌더.
  `stream_supported`가 `refresh_view`/`set_settings`/`SettingsView`를 관통해
  전달된다(현재 provider 스택은 항상 SSE 광고 — 도달 불가 상태는 미래 비SSE
  프로바이더용).
- ~~푸터 힌트에 `[S]`가 없다~~ — **해소**: `[S] 스트림` 추가. 푸터가 정확히 80컬럼
  (구분 공백 2→1) — 스냅샷 테스트가 `[Q] 종료` 클리핑을 못하게 고정.
- ~~`?` 키맵 오버레이는 20/20 무여유~~ — **해소**: 게임 `s`/`l` 행을 "s / l · 저장 ·
  불러오기"로 병합해 오른쪽 열 1행 여유 확보(현재 20/19).
- ~~R2: 전환 테스트가 낡은 미러 시딩을 검증하지 않는다~~ — **해소**:
  `provider_switch_carries_the_stream_preference_across_profiles`가 스위치 전
  미러를 `refresh_view(off)`로 오염시킨 뒤 `apply_provider`가 권위 값에서 복구함을
  검증. Settings `s` 테스트는 세이브 다이얼로그 미누출도 검증.

## 독립 리뷰 결함 수정 (2026-11-09)

- **F1(P1) 프로바이더 전환이 세션 토글을 되돌림** — 시작 오버라이드가
  `provider_config`(작업 사본)에도 반영되도록 수정(`host.rs`), `switch_provider`가
  roster 사본에 샘플링 taste와 동일하게 선호를 재적용, `refresh_view` 두 호출부가
  미러 대신 권위 값 `config.provider.stream_turns`를 투영하도록 교체. 회귀 테스트
  `provider_switch_carries_the_stream_preference_across_profiles` 추가.
- **F2(P2) persona 행이 상태 패널에서 밀려남** — 상태 블록 `Length(9)`→`Length(10)`.
  24행 바닥에서는 단축키 미리보기가 1행 양보(전체 키맵은 `?` 오버레이 유지,
  그 이상 터미널은 `Min(3)`이 회복). 스냅샷 재생성으로 persona 행 복원 확인.

## 배경 (G0)

- 2026-11-09 시점 열린 GitHub 이슈/PR은 없음(전부 CLOSED/MERGED). 코드로 닫을 수 있는
  문서상 열린 항목은 KNOWN-LIMITATIONS의 "settings UI does not yet expose
  `stream_turns`" 한 건이었다(`use_director_writer`는 Settings `g` 토글과
  user-settings 지속이 이미 완비 — 문서 문장이 낡았음).
- RELEASE-RUNTIME-GATE G0–G5는 라이브 MLX 서버·Apple Silicon·인간 평가가 필요한
  런타임 전용 게이트로 현재 Linux 호스트에서 BLOCKED. 이 변경과 무관.

## 수용 기준 (AC)

- **AC-1** Settings 화면에서 `s`(한글 2벌식 ㄴ) 키로 단일 패스 경로의 SSE 스트리밍
  렌더를 켜고 끄을 수 있고 현재 상태가 화면에 표시된다.
- **AC-2** 토글 값은 `user-settings.toml`의 `stream_turns`에 지속되고, `None`(미사용)은
  프로필의 `[provider] stream_turns`를 상속한다.
- **AC-3** 2단계(Director→Writer) 경로가 활성일 때 이 토글은 무의미하므로 행이
  "2단계 경로 사용 중"(후속 라운드에서 저장 선호 표시 "2단계 중 (켜짐/꺼짐)"으로
  개선)으로 표시된다(2단계 Writer는 항상 스트리밍).
- **AC-4** 세션 중 토글은 다음 생성 요청부터 반영된다(host `config.provider.stream_turns`가
  세션 권위; 진행 중 생성은 무영향 — 기존 `g` 토글과 동일 계약).
- **AC-5** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace` 통과.

## 설계 (G1)

기존 `g`(생성 방식) 토글 패턴(`host.rs` take_권한→config 플립→user-settings 저장→
`app.update_settings_*`)을 그대로 재사용:

- **D1** `UserSettings::stream_turns: Option<bool>` — `None` 상속, `Some(bool)` 오버라이드.
- **D2** 시작 시 `config.provider.stream_turns = user_settings.stream_turns
  .unwrap_or(프로필값)` (`use_director_writer` 오버라이드와 같은 지점).
- **D3** app 미러: `SettingsState.stream_turns` + `stream_turns()` getter +
  `update_settings_stream` — host 권위 값의 표시용 사본(기존 미러 계약과 동일).
- **D4** 키는 Settings 전용 `s`|`ㄴ`. story 화면의 `s`(세이브)와는 화면 스코프가 분리됨.
- **D5** `?` 키맵 오버레이 2×20 행 예산 초과 분은 설정 블록과 별칭 노트 사이 빈 줄
  제거로 흡수(표가 예산을 넘으면 기존 예산 테스트가 실패하도록 유지).

## 변경 파일 (G2)

- `apps/storytui/src/config.rs` — UserSettings 필드 + round-trip 테스트
- `apps/storytui/src/host.rs` — 시작 오버라이드, 토글 소비·지속·뷰 패치, refresh_view 인자
- `apps/storytui/src/host/settings.rs` — refresh_view 시그니처/호출 3곳
- `crates/story-tui/src/app.rs` — 상태·플래그·키 핸들러·getter·뷰 조립, `s` 키 테스트
- `crates/story-tui/src/screens/settings.rs` — SettingsView 필드 + "스트림 (s)" 행
- `crates/story-tui/src/screens/keymap.rs` — Settings 그룹 행 + 예산 조정
- `crates/story-tui/tests/settings_snapshot.rs` — 스냅샷 3건 재생성 + 상태 3종 테스트
- `apps/storytui/tests/host_automation.rs` — 토글 off 세션이 비스트리밍 요청 1회만
  보내는지 검증
- `docs/KNOWN-LIMITATIONS.md` — 낡은 "not yet" 문장 갱신
- `thoughts/shared/plans/2026-08-21-storytui-stability-hardening.md` — 수동 검증 2건을
  자동화 증거로 확정 체크

## 검증 (V)

- `cargo test --workspace` — 65 스위트 전부 통과(스냅샷 재생성 포함)
- `cargo clippy --workspace --all-targets -- -D warnings` — 통과
- `./scripts/check-core-deps.sh`, `git diff --check` — 통과
- keymap 예산 테스트(`both_columns_fit_the_help_overlay_body_budget`) — 통과
  (초기 20/20 → s/l 병합 후 20/19, 2026-11-10 검증에서 재확인)
