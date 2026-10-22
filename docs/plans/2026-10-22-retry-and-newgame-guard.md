# 생성 재시도(로딩 중 R)와 새 게임 확인 — 판단 미스 방지

2026-10-22 · 요청: "다른 이런 앱들을 모두 찾아보고 해결해보자" +
"무엇보다도 중요한 것은 사용자가 잘 판단하지 않더라도 잘 동작하도록 하는 것."
(같은 날 두 번째 사이클 — 2026-10-22-turn-undo.md의 후속.)

## 연구 (SDD discovery)

첫 사이클에서 AI Dungeon(Retry/Rewind)·NovelAI(undo/branch)·SillyTavern(swipe)의
복구 수단 중 "확정 후 되돌리기"를 개방했다. 남은 절반은 "확정 전 다시 뽑기"다:

1. **확정 전 재시도** — AI Dungeon Retry는 응답 후보를 캐시해 순환 선택하게 하고
   (reddit r/AIDungeon), SillyTavern swipe가 커밋 전 대안 생성을 요청한다.
   StoryTui는 로딩 중 `c` 취소 후 선택을 다시 고르는 2단 판단이 필요한데,
   엔진의 `EngineCommand::RegenerateCurrentTurn`(같은 체크포인트에서 재요청,
   lineage 기록)이 W3부터 구현돼 있고 오류 오버레이의 `R`로만 연결돼 있다.
   호스트는 `RequestGeneration`/`RequestDirector` 처리 시 기존 취소 토큰을
   take→cancel→교체하므로(host.rs:1093,1145) 재생성 시 진행 중 요청 정리에
   **호스트 변경이 불필요**하다.
2. **파괴 동작 무확인** — UI-NAVIGATION이 스스로 "⚠ 확인 없이 즉시 새 게임
   (현재 진행 리셋) — 의도치 않은 입력 주의"라고 경고하는 유일한 키가
   `n`/ㅜ다(event.rs가 곧바로 `StartNewGame` 발행). 팩 삭제에는 확인 모달
   (`render_pack_delete_confirm`, DANGER 테두리·Enter/Esc)이 이미 있고
   세이브 덮어쓰기에도 확인이 있지만, 진행 전체를 지우는 새 게임만 없다.
   첫 사이클에서 StartNewGame이 체크포인트 링까지 비우게 되어(되돌리기
   불가) 우발 `n`의 실제 손실이 커졌다.

Novelcrafter Codex(로어북 위키+AI 보조 작성)도 확인 — StoryTui의 런타임
로어북(키워드 활성화·sticky/cooldown)은 기능적으로 앞서므로 이번 범위 밖.

## 목표와 범위

- **달라지는 결과**:
  1. 로딩 중 `r`/`ㄱ` 한 번으로 같은 선택지·같은 체크포인트에서 생성을 다시
     요청한다(취소→재선택의 2단 판단 불필요). 로딩 안내에 `[R] 다시 만들기`가
     표시된다.
  2. 게임 중 `n`/ㅜ는 진행이 있을 때(턴 > 0) 확인 모달을 먼저 보여준다 —
     Enter 확정·Esc 취소. 턴 0(잃을 것이 없는 시작 전)은 종전대로 즉시.
- **제외**: 시작 메뉴의 "새 게임" 행(메뉴에서 명시적 선택이므로 무확인 유지),
  AI Dungeon식 응답 후보 캐시/순환(단순 재요청만 — 후보 보관은 redo 사이클과
  통합 검토), 2단계 스트리밍 중 화면 정리 이상(엔진이 plan을 폐기하고
  Director부터 재요청 — 기존 reduce_regenerate 동작).

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | 로딩 중 `r`/`ㄱ`를 누르면 진행 생성이 같은 체크포인트에서 새 generation_id로 재요청되고 lineage가 늘어난다 | T-1 | V-1, V-2 |
| AC-2 | 로딩 오버레이 안내에 `[C] 취소 · [R] 다시 만들기`가 표시된다 | T-2 | V-3 |
| AC-3 | 게임 중 턴 > 0에서 `n`/ㅜ를 누르면 확인 모달이 뜨고, Enter만 새 게임을 확정하며 Esc는 아무 상태도 바꾸지 않는다. 턴 0에서는 종전대로 즉시 시작한다 | T-3 | V-3 |
| AC-4 | 기존 흐름(취소, 오류 재시도, 선택지 재생성, 새 게임, 체크포인트/undo)에 회귀가 없다 | T-1..T-4 | V-4, V-5 |

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 태스크 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | `map_key`에 로딩 중 `r`/`ㄱ` → `RegenerateCurrentTurn` 추가 (AC-1) | crates/story-tui/src/event.rs | 없음 | V-1 | done |
| T-2 | 로딩 안내 문구 3곳에 `[R] 다시 만들기` 추가, 스냅샷 갱신 (AC-2) | crates/story-tui/src/screens/story.rs, tests/snapshots.rs | T-1 | V-3 | done |
| T-3 | `new_game_confirm` 모달(팩 삭제 확인 패턴 재사용): 턴>0 `n`/ㅜ 게이트, Enter 확정→StartNewGame, Esc 취소, 렌더 (AC-3) | crates/story-tui/src/app.rs, crates/story-tui/src/screens/story.rs | 없음 | V-3 | done |
| T-4 | 호스트 무변경 확인(토큰 교체가 effect 처리에서 자동 수행) + 문서 2종 + 본 계획 (AC-1,3,4) | docs/{UI-NAVIGATION.md,KNOWN-LIMITATIONS.md}, 본 계획 | T-1..T-3 | V-4 | done |

## 설계 메모

- **재생성 의미론**: `reduce_regenerate`는 pending(또는 retry_pending)의
  같은 체크포인트·같은 선택으로 새 generation_id를 발행하고 lineage에
  추가한다 — 커밋된 턴은 건드리지 않는다. 2단계 경로에서는 plan을 폐기하고
  Director부터 재요청한다(기존 구현). 오류 오버레이의 `R`과 같은 커맨드라
  의미가 일관된다: "다시 뽑기".
- **토큰 정리**: 호스트가 RequestGeneration/RequestDirector effect마다 기존
  토큰을 take→cancel하고 새 토큰을 심는다 — dispatch의 CancelGeneration
  전용 취소는 "새 요청이 없는 취소"를 위한 것이므로 재생성에는 불필요.
  늦게 도착한 구 generation_id 응답은 엔진이 id 불일치로 폐기한다.
- **확인 모달 범위**: 게임 중 스토리 화면 키 경로만 막는다. 시작 메뉴·
  홈 경로는 명시적 선택으로 보고 유지. 모달은 진행 중(로딩/오류)에는
  열리지 않는다(기존 모달 선행 분기 순서 유지).

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-tui` — `r` 키 로딩/대기 분기 매핑(라틴/한글), **App 게이트 통과**(D1 재판정 지적로 V-1을 App 레벨로 상향), 기존 idle `r`=선택지 재생성 유지 | repo root | 신규+기존 전부 통과 | 전체 스위트 158 passed / 0 failed — App 레벨 `r_key_rerolls_the_in_flight_turn_through_the_app_gate`(loading=Some 상태 handle_key r/ㄱ → RegenerateCurrentTurn, 무관 키 흡수 유지), map_key 갱신 테스트, `n_starts_a_new_game` 로딩 차단 단정 포함 (2026-10-22) | PASS |
| V-2 | `cargo test -p story-core` — 재생성 lineage·체크포인트 불변 기존 테스트 회귀 | repo root | 전부 통과 | 282 passed / 0 failed — reduce_regenerate lineage·체크포인트 불변 기존 테스트 전부 통과(엔진 무변경) (2026-10-22) | PASS |
| V-3 | `cargo test -p story-tui` — 로딩 문구 스냅샷, `n` 확인 모달(턴>0 열림/Enter 발행/Esc 무효/턴0 즉시/로딩 중 무시) | repo root | 신규+기존 전부 통과 | 스냅샷 9종 통과 — 로딩 오버레이 1종 `[C] 취소 · [R] 다시 만들기` 문구 갱신, 설정 2종은 키맵 n 문구 단축(열 폭 절단 해소) 후 재생성. 신규 `n_key_asks_before_resetting_progress`(n/ㅜ 모달·Enter 발행·Esc 무효)·`n_key_at_turn_zero_starts_immediately` 통과 (2026-10-22) | PASS |
| V-4 | `cargo test -p storytui` — 호스트 통합 회귀(dispatch 무변경 확인 포함) | repo root | 전부 통과 | 63 passed / 0 failed — dispatch 무변경(토큰 교체는 RequestGeneration/RequestDirector effect 처리에서 기존 코드가 수행, host.rs:1093/1145) (2026-10-22) | PASS |
| V-5 | `cargo test --workspace` + clippy + fmt + check-core-deps + check-public-config | repo root | 회귀 0 | 880 passed / 0 failed, clippy 오류 0(collapsible-match 1건 수정 후), fmt 정렬, 스크립트 2종 OK (2026-10-22) | PASS |

### 검증 중 수정 이력

- 키맵 `n` 문구가 설정 화면 2열 폭을 넘어 절단되어 스냅샷 2종이 실패 → 문구를
  "새 게임 확인 모달"로 단축(상세는 UI-NAVIGATION/도움말이 담당) 후 재생성.
- clippy `collapsible-match` 1건(`n` arm의 중첩 if → 가드로 축약) 수정 후
  게이트 전체 재실행.
- **G3 독립 리뷰 결함 처리**:
  - D1(P1): 로딩 키 게이트(app.rs)가 `c/ㅊ/q/ㅂ`만 통과시켜 map_key의 신규
    `r` arm이 실제 입력 경로에서 도달 불가 — 평가자가 /tmp 격리 프로브로
    `handle_key('r')`(loading) → None을 관찰해 지적. **수정**: 게이트 통과
    집합에 `r/ㄱ` 추가 + App 레벨 회귀 테스트(V-1을 map_key 단위에서 App
    배선 증거로 상향). map_key 단위 테스트만으로 배선을 대신했던 증거
    층위 오류를 바로잡음.
  - D2(P3): 확인 모달이 키보드에만 모달 — 모달 뒤 푸터 클릭([U]/[Home] 등)이
    동작. **수정**: `handle_mouse` 초입에 `new_game_confirm` 조기 return +
    클릭 무시 테스트. 데이터 손실 경로는 없었음(평가자 확인).
  - 재판정 전 재검증: 워크스페이스 882 passed / 0 failed, clippy 0, fmt
    정렬, 스크립트 2종 OK.

## G5 완료 판정 (self-review — reviewer는 G3 재판정에서 런타임 프로브+전체 게이트로 통합 상태를 이미 대조함)

| 원 요청 | 결과 |
|---|---|
| "모두 찾아보고 해결해보자" (2차) | 경쟁 앱 복구 수단의 나머지 절반("확정 전 다시 뽑기") 개방 + 유일한 무확인 파괴 키(`n`)에 확인 모달 |
| "사용자가 잘 판단하지 않아도 잘 동작" | 마음에 드는 생성이 나올 때까지 `r` 한 번(취소→재선택의 2단 판단 제거), 실수 키는 모달이 받고 모달 뒤 마우스 클릭도 차단 |
| 독립 검증 | 1차 G3에서 P1(죽은 키 — 실입경로 도달 불가)·P3(모달 뒤 마우스 누수) 발견 → 수정 → 재판정 PASS(런타임 프로브+게이트 전수 재현) |

잔여 한계: 2단계 스트리밍 중 연속 `r`의 실단말 체감(사용자 실행), 응답 후보
캐시/순환과 시작 메뉴 새 게임 확인(후속 사이클 후보).

## 평가와 인계

- 실단말 체감(스트리밍 중 `r` 재생성 체감, `n` 모달)은 기존 컨벤션대로
  사용자 실행으로 남긴다.
- 응답 후보 캐시/순환(AI Dungeon Retry의 캐시형), 시작 메뉴 새 게임 확인은
  후속 사이클 후보.
- 커밋은 사용자 요청 시에만.
