# 임포트 파일 브라우저 + 팩 삭제 계획

2026-10-18 · 요청: "경로 붙여넣기랑 파일 브라우저 가지고 오기도 하자… 사용자가 내부에서 제거하도록"

## 목표와 범위

- **달라지는 결과**:
  1. 카드 가져오기 모달에서 경로 입력(기존 유지) 외에 **파일 브라우저**로 디렉터리를 탐색해
     카드 파일을 고를 수 있다.
  2. "팩 선택" 목록 안에서 **팩을 삭제**할 수 있다 (확인 모달 필수).
- **제외**: 임의 파일 시스템 삭제(브라우저에서의 파일 삭제는 하지 않는다 — 팩 단위만), 세이브 삭제
  (세이브는 별도 저장소라 팩 삭제 후에도 유지 — 고아 세이브는 무해).
- **현재 구조(확인한 근거)**:
  - 키 디스패치 우선순위: greeting_picker → pack_picker → import_dialog → home
    (`crates/story-tui/src/app.rs:870-884`).
  - 모달 데이터는 호스트가 채워 넘기는 패턴: `HomeRequest::PickPack` → `packs::scan_packs`(인라인,
    ADR-003 호스트 I/O) → `app.open_pack_picker(dirs, labels, current)` (`apps/storytui/src/host.rs:723-731`).
  - 임포트: `ImportDialog::{Input,Running,Report}` (`app.rs:67-78`, InputBuffer는 `Clone`),
    제출은 `import_request` → 호스트 spawn_blocking → `ImportFinished` → `show_import_report`.
  - `PackEntry.dir`은 절대 경로 문자열, 스캔 루트는 활성 팩의 부모 (`host/packs.rs`).
  - 삭제는 파괴적 동작 — 확인 모달 + 가드 필수.
- **기준 상태**: HEAD `288c97c` + 임포트 워크스트림 미커밋 diff(story-import 등, 이번 태스크와 무관).
  참고: 타 워크스트림과 app.rs/host.rs가 겹칠 수 있어, 이번 변경은 임포트 다이얼로그·팩 피커 영역만
  만진다.
- **도구·환경**: cargo 테스트/렌더 테스트(TestBackend) — 대화형 TUI 실관찰은 사용자 실행으로 확인.
- **설계 확정(사용자 질문 불필요한 기본값)**: 브라우저 시작 위치는 HOME(없으면 `.`), 파일 필터는
  `.png/.json/.charx`(대소문자 무시), 숨김 항목 제외, 삭제 가드는 "스캔 루트 직계 + story.toml 존재
  + 활성 팩 아님".

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | 카드 가져오기에서 Tab으로 파일 브라우저를 열고 폴더를 탐색해 지원 카드를 골라 임포트할 수 있다 (경로 입력 모드 보존, 입력 버퍼는 전환 사이 유지) | T-2, T-3 | V-1, V-4 |
| AC-2 | 브라우저는 폴더 진입(Enter)·상위 이동(Backspace)을 지원하고, 파일은 지원 확장자만, 숨김 항목은 제외하고 표시한다 | T-1, T-2 | V-2 |
| AC-3 | 팩 선택 목록에서 d로 삭제 확인 모달을 열고 Enter로 삭제한다. 현재 활성 팩은 삭제 요청이 만들어지지 않는다 | T-2, T-3 | V-1 |
| AC-4 | 삭제는 확인 후에만 실행되며, 스캔 루트 직계이고 story.toml을 가진 디렉터리만 삭제된다 (가드 위반은 거절) | T-1, T-4 | V-2 |
| AC-5 | 기존 경로 입력 임포트·팩 스왑·홈 메뉴 흐름에 회귀가 없다 | T-4 | V-3 |

## 설계

- **브라우저 상태**: `ImportDialog::Browse { dir, entries, selected, input: InputBuffer }` — Input의
  버퍼를 소유 이동해 Tab 왕복 시 복원. `entries: Vec<(String 이름, bool 디렉터리)>`, 폴더 먼저 이름순.
  렌더 뷰는 `ImportDialogView::Browse` 추가 (`screens/import.rs`).
- **요청 라운드트립**: `browse_request: Option<String>` — 빈 문자열 = 시작(호스트가 HOME/`.` 결정),
  그 외 = 나열할 절대 경로. 호스트는 PickPack과 같은 인라인 스캔으로 답한다(로컬 read_dir, 수 ms).
- **키**: Tab 입력↔브라우저 전환, ↑↓/k/j 이동(한글 자판 변형 포함 — 기존 피커 관례), Enter 폴더 진입
  또는 카드 선택(→ 기존 Running/Report 흐름 재사용), Backspace 상위 폴더, Esc 브라우저→입력(다시 Esc로
  닫기). 풋터에 힌트 표기(온보딩 철학: 암기 키 없음).
- **삭제**: 피커에서 `d`(한글 ㅇ 포함) → `pack_delete_confirm = Some(선택)`(활성 팩은 무시) → 확인
  모달(title "팩 삭제", 본문에 되돌릴 수 없음·세이브 유지 명시) → Enter → `pack_delete_request` →
  호스트가 가드 검사 후 `remove_dir_all` → 재스캔해 피커를 다시 연다. 실패 시 `set_home_status`로
  안내. keymap 표는 스토리 화면 기준이라 미변경(모달 로컬 힌트로 충분).
- **삭제 가드(`packs::delete_pack`)**: 대상이 스캔 루트(활성 팩 부모)의 직계 자식인지, `story.toml`을
  가졌는지, 활성 팩이 아닌지 — 하나라도 아니면 거절(에러 문자열). 심링크/경로 탈출 방지는 "직계 자식"
  검사로 차단.
- **오류·복구**: 삭제 실패는 팩을 건드리지 않고 `set_home_status`로 안내. 브라우저에서 읽기 실패한
  디렉터리는 빈 목록으로 열고 모달이 "표시할 항목이 없습니다 (읽기 실패했을 수 있습니다)"로 안내해
  모달이 죽지 않게 한다(구현에서 단순화 — 목록 항목 대신 빈 상태 안내).

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 태스크 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | 호스트 I/O: 브라우저 목록·삭제 가드 (AC-2,4) | apps/storytui/src/host/packs.rs | 없음 | V-2 | done |
| T-2 | App 상태·키: Browse 변형, 요청 플래그, 삭제 확인 (AC-1,2,3) | crates/story-tui/src/app.rs | 없음 (T-3와 같은 크레이트 — T-2 후 순차) | V-1 | done |
| T-3 | 렌더: 브라우저 뷰·삭제 확인 모달·푸터 힌트 (AC-1,3) | crates/story-tui/src/screens/{import.rs,pack.rs}, app.rs render 전달 | T-2 | V-1 | done |
| T-4 | 호스트 루프 연결 + 회귀 검증 (AC-1..5) | apps/storytui/src/host.rs | T-1, T-2 | V-2, V-3 | done (검증은 V-3 진행) |

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-tui` — 브라우저 상태 기계·삭제 확인 흐름·렌더 테스트 | repo root | 신규+기존 전부 통과 | 최종: lib 111/111 + 전체 타깃 127개 전부 ok (2026-10-18, G5 독립 재실행으로 확정 — 초기 100개는 타 세션 popup 작업 전 상태) | PASS |
| V-2 | `cargo test -p storytui` — list_dir_for_browse 필터·delete_pack 가드 테스트 | repo root | 가드 위반 전부 거절, 성공 케이스 삭제 | 신규 가드 테스트는 **bin 타깃**에서 실행(`cargo test -p storytui` bin 23개 전부 통과 — G3 독립 재실행으로 확인: browse_lists_dirs_first_and_filters_files, browse_unreadable_dir_reports_instead_of_panicking, delete_pack_guards_active_root_and_manifest). `--lib` 17개는 기존 config/diagnostic 테스트라 신규 테스트 미포함 — 초기 기록의 "lib 17개 통과(신규 포함)"는 부정확했고 본 행으로 정정(G3 F-1) | PASS |
| V-3 | `cargo test --workspace` + clippy + fmt | repo root | 회귀 없음 | 워크스페이스 63 스위트 전부 ok(실패·에러 0), clippy(story-tui+storytui) 경고 0, 범위 파일 fmt 정렬 (2026-10-18) | PASS |

## G5 결과 (2026-10-18, 위임 평가자 — completion)

**PASS** — AC-1..5 전부 독립 재실행 증거로 충족(story-tui 127개·storytui 55개 전부 ok, clippy 0,
stale 증거 없음). 남음: ① 대화형 실단말 체감 확인은 사용자 실행(계획에 명시된 한계), ② F-3(가드
거절 오류가 10+ 팩에서 피커에 가려질 수 있음)은 후속 과제로 분리.
| V-4 | 실관찰: 렌더 테스트로 화면 확인(대화형 실사용은 사용자 실행) | repo root | 모달이 그려지고 힌트가 보임 | 브라우저 뷰(항목/`sub/`/힌트), 빈 목록 안내, 삭제 확인 모달(제목·경고·Enter 삭제) 렌더 테스트로 확인 | PASS |

## 평가와 인계

- G3 코드 리뷰·G5 완료: 위임 평가자. G4(UI): 렌더 테스트 증거 + self-review 병기 — 대화형
  실관찰은 터미널 앱 특성상 사용자 실행으로 남긴다(한계 명시).
- 커밋은 사용자 요청 시에만, 임포트 워크스트림 diff와 분리.

## G3 결과 (2026-10-18, 위임 평가자 — review-changes)

**PASS** — AC-1..5 충족, P0–P2 없음. 검증: story-tui lib 111/111, storytui bin 23+lib 17,
clippy 0, 범위 파일 fmt 정렬. 삭제 가드 우회 없음(생산·소비 경로 전수 확인), 심링크 `remove_dir_all`
안전성 rustc 재현 입증, 브라우저 라운드트립은 단일 이벤트 루프에서 요청 즉시 소비로 구조적으로 안전,
ADR-003 위반 없음(범위 내 story-tui 3파일 `std::fs` 0건).

- F-1 (P3): V-2 증거 기록이 lib/bin 타깃을 혼동 → 위 표로 정정(검증 자체는 유효).
- F-2 (P3): 재열람 시 `import_last_dir` 프리필로 빈 Enter가 디렉터리 경로 임포트 시도가 됨 —
  동시 세션(기능 소유)이 **의도된 동작**으로 확인, 갱신된 테스트로 고정. 유지.
- F-3 (P3, 후속 제안): 가드 거절 오류가 set_home_status로 표시되는데 10+ 팩에서 재열람된 피커
  팝업이 가릴 수 있음 — 후속 과제로 분리(피커 인라인 오류 표시).
- 범위 밖 관찰: story.rs:1419에 fmt 위반 1건(동시 세션 소속 파일).
