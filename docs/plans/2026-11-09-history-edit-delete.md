# History 턴 편집·삭제 + 잔여 이슈(#3–#6) 마무리

2026-11-09 · `/sdd` · 기준 리비전 `main @ ea1c636`

## 목표와 범위

- **달라지는 결과**
  1. History(`h`)에서 임의 턴의 **내레이션**을 메뉴 진입으로 편집하고, 세이브(자동 저장)에 반영한다.
  2. History에서 턴을 **삭제**하면 해당 턴 이후가 함께 잘리고, 엔진 상태는 체크포인트 링의 직전 안정 상태로 롤백된다(정합 규칙 명시).
  3. 열린 이슈 `#4`·`#5`·`#6`은 이미 main에 구현돼 있으므로 AC 대조 검증 후 이슈를 닫고, 갭 문서의 낡은 P0 서술을 고친다.
- **제외**: 플레이어 발화/선택지 편집, 임의 중간 턴만 제거하고 이후 턴을 유지하는 수술적 삭제, redo/이력 트리 UI, 편집 본문의 진단·로그 기록(NFR-006).
- **환경·평가**: repo root, `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings`. 독립 평가자 위임 가능(G3). UI는 스냅샷·키 계약 테스트로 관찰(실단말 대화형은 기존 컨벤션상 사용자).

## G0 요구 (intake)

| 이슈 | 판정 | 근거 |
|---|---|---|
| #3 턴 편집·삭제 | **구현 필요** | `EditTurn`/`DeleteTurn` 심볼 없음, History `render_list` 읽기 전용 |
| #4 턴 되돌리기 | **구현 완료·검증만** | `RestoreCheckpoint` + `u`/`ㅕ`, plan `2026-10-22-turn-undo.md` |
| #5 분기·스와이프 | **구현 완료·검증만** | `CycleTurnVariant` + `[`/`]`, plan `2026-10-23-parity-followup.md` |
| #6 대화 내보내기 | **구현 완료·검증만** | `story-persistence/export.rs` + `storytui export`, plan `2026-10-23-frontend-parity.md` |

### 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | History에서 선택 턴의 내레이션을 편집 모달로 고칠 수 있다(Enter 진입, Esc 취소). 플레이어 발화는 편집하지 않는다 | T-1, T-3 | V-1, V-3 |
| AC-2 | 편집 결과가 엔진 history·(마지막 턴이면) 스토리 뷰에 반영되고 autosave가 갱신된다. 체크섬/스키마 계약은 기존 SavePayload 필드로 유지 | T-1, T-4 | V-1, V-4 |
| AC-3 | 삭제 = 선택 턴부터 이후 전부 제거 + `turn == selected.turn - 1` 체크포인트로 복원. 링에 없으면 no-op(안내). 오프닝(turn 1) 삭제는 거부(새 게임 경로) | T-2, T-3 | V-2, V-3 |
| AC-4 | 편집·삭제 본문/프롬프트가 기본 로그·진단에 실리지 않는다(NFR-006). 세이브에는 내레이션이 정상 포함(내보내기용) | T-1, T-2 | V-1, V-5 |
| AC-5 | `#4`·`#5`·`#6` 이슈 AC가 현 main에서 충족됨을 테스트/코드로 확인하고 이슈를 닫으며 갭 문서를 갱신한다 | T-5 | V-5, V-6 |
| AC-6 | `cargo test --workspace` · clippy `-D warnings` 통과 | T-1..T-5 | V-6 |

## 설계

### 엔진

- `EngineCommand::EditHistoryNarration { entry_id, narration }`
  - 로딩/오류 중 identity. 빈 문자열( trim 후) identity.
  - history에서 id 매칭 → `narration` 교체, `revision++`.
  - 마지막 턴이면 `current_variants`의 현재 슬롯도 같은 텍스트로 맞춘다(배지 불일치 방지).
  - `upsert_current_checkpoint` + `PublishView` + (플래그 시) `PersistStableState`.
  - **패치·플래그·선택지는 재계산하지 않음** — 내레이션은 표시/프롬프트 산문만(플레이어 주권: 모델 실수를 고치는 축).
- `EngineCommand::DeleteHistoryFrom { entry_id }`
  - 해당 엔트리의 `turn`을 찾고, `checkpoints`에서 `game.turn == turn - 1`인 항목을 골라 `reduce_restore_checkpoint`에 위임.
  - `turn <= 1` 또는 체크포인트 없음 → identity.
  - 결과적으로 선택 턴과 이후가 사라지고 엔진 상태(rules/flags/history)가 스냅샷과 일치(AC-3).

### TUI

- History 목록에 선택 커서(`↑↓`/`k j`), 제목/푸터: `Enter 편집 · d 삭제 · Esc 닫기`.
- App에 `history_entry_ids: Vec<Uuid>`(표시 순서=역순)와 선택 인덱스 보관; `set_lists`가 id·narration 프리필 소스를 채움.
- 편집 모달: Author's Note 패턴(`InputBuffer`, 큰 상한). Enter 적용 → 커맨드, Esc 취소.
- 삭제: 새 게임 확인 모달 패턴. 삭제 불가(오프닝/링 밖)면 모달 대신 짧은 상태 문구/무시.
- 핫키 남발 금지: 스토리 화면 신규 단축키 없음 — History 메뉴 안에서만.

### 문서

- `UI-NAVIGATION.md` History 행 갱신, `KNOWN-LIMITATIONS.md`에 삭제=링 한정·오프닝 불가, `StoryTUI_Feature_Gap_vs_ST_Risu_KO.md` P0-2~5 현황 정정.

### UI 설계 모드 (review-design)

- 핵심 작업: 잘못된 내레이션 고치기 / 분기점부터 다시 하기.
- 상태: 빈 History, 선택, 편집 중, 삭제 확인, 삭제 불가, 로딩 중 차단.
- 디자인 출처: 기존 모달·리스트(설정 작가노트, 새게임 확인, 모델 피커 선택).
- 80×24: 모달 `popup` 스케일 재사용; 긴 내레이션은 버퍼 스크롤/래핑.

## 태스크

| ID | 목적·AC | 수정 경계 | 선행 | 검증 | 상태 |
|---|---|---|---|---|---|
| T-1 | 엔진 EditHistoryNarration + reduce 테스트 | `engine_cmd.rs`, `engine.rs`, `tests/reduce.rs` | 없음 | V-1 | done |
| T-2 | 엔진 DeleteHistoryFrom + reduce 테스트 | 동상 | 없음 | V-2 | done |
| T-3 | TUI History 선택·편집 모달·삭제 확인·키 | `story-tui` app/screens/input | T-1,T-2 | V-3 | done |
| T-4 | 호스트 `refresh_app_lists` + 문서 | host/saves, docs | T-3 | V-4 | done |
| T-5 | #3–#6 close·갭 문서 | docs, `gh issue` | T-1..T-4 | V-5,V-6 | done |

## 검증 계획

| ID | 방법 | cwd | 기대 | 실제 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-core --test reduce history_` | repo | 편집·뷰·autosave·빈문자 no-op | 6 passed | PASS |
| V-2 | (V-1) delete_history_* | repo | 접미 절단·링 밖/오프닝 no-op | 포함 | PASS |
| V-3 | `cargo test -p story-tui --lib history_` | repo | 키·모달·빈제출·링밖 안내 | 6 passed | PASS |
| V-4 | `saves::refresh_app_lists` + set_lists_with_checkpoints | repo | 삭제 마스크 전달 | host 배선 | PASS |
| V-5 | #4/#5/#6 심볼·갭 문서·`gh issue close` | repo | 이슈 종료 | #3–#6 closed | PASS |
| V-6 | `cargo test --workspace` · clippy `-D warnings` | repo | 전부 통과 | 2026-11-09 PASS | PASS |

## 평가와 인계

- G0 intake: PASS
- G1 plan: PASS (삭제=체크포인트 접미 절단)
- G2 구현: PASS (T-1..T-5 done)
- G3 code review (independent): PASS — F-1/F-2(P2) 수정 반영(삭제 마스크·빈 편집 유지), F-3 문서 정합, F-4 이슈 close
- G4 UI (self-review): PASS — 키 계약·모달 패턴·상태(불가 안내) 테스트로 관찰. 실단말 대화형 미실행(기존 컨벤션)
- G5 completion: PASS — AC-1..6 충족. 커밋/푸시는 사용자 요청 시에만.

### 잔여 한계
- 삭제 가능 범위는 체크포인트 링(8) 안; 오프닝 삭제 불가; 중간 턴만 제거하고 이후 유지하는 수술은 없음
- 변형 풀은 세션 메모리(#5 부분 충족 유지)
