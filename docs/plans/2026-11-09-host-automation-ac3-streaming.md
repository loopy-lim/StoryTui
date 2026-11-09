# #12 host_automation AC-3 — 스트리밍 폴백 계약 정렬

## 목표와 범위

- **목표**: main에서 선재 실패하는 `host_automation` AC-3 단일 경로 테스트 2건을, PR #7 이후 `generate_turn_stream`의 청크 전 실패 → 비스트리밍 폴백 계약에 맞게 갱신한다.
- **사용자 관찰 결과**: `cargo test -p storytui --test host_automation`가 전부 통과하고, 호스트 자동 재시도(AC-3) 의도(생성 **전체** 실패 1회 자동 재시도 → 2회째 실패 시 오버레이)가 유지된다.
- **제외**: 프로바이더 폴백 제품 동작 변경, Writer `write_stream`(폴백 없음) 경로, 이슈 #3–#6 기능.
- **기준**: `main` @ `cf4b02b`, 브랜치 `fix/host-automation-ac3-streaming-contract`.
- **환경**: 로컬 cargo workspace. 평가자: 위임 가능 시 independent, 소규모라 self-review도 허용. UI 변경 없음 → G4 N/A.

## 확인된 사실

- `ProviderConfig::defaults().stream_turns == true`. host 단일 경로가 `generate_turn_stream`을 씀 (`apps/storytui/src/host.rs` ~1988).
- 청크 전 `Protocol("HTTP 400")`은 비스트리밍 `generate_turn`으로 폴백 (`openai_compatible.rs` `generate_turn_stream`). `structured_output: Off` + `max_retries: 0`이어도 **요청 2회**(stream + non-stream)가 된다.
- `write_stream`은 폴백 없음 → 2-stage Writer 테스트 요청 수는 그대로 유효(재현에서 `two_stage_*` 통과).
- 재현: `generation_failure_autoretries_once_and_succeeds`는 첫 요청 400 뒤 폴백 200으로 호스트 재시도가 무장되지 않음; `second_failure_…`는 요청 4건(기대 2).

## 설계 결정

- **권장**: 테스트만 갱신. 프로바이더 폴백은 요청 shape 전환(ADR-002 4xx 재시도가 아님)이고, 호스트 AC-3는 “프로바이더 호출 **전체**가 실패한 뒤”의 1회 재시도다. `stream_turns=false`로 옛 계약을 복원하면 기본 경로 커버가 약해진다.
- 첫 성공 시나리오: 연속 2회 400(stream+non-stream) 후 200 → 호스트 재시도 무장 → 프레임 지연 후 성공. HTTP = 실패 2 + 재시도(최소 1, 스트림 성공 시 총 3).
- 이중 실패 시나리오: 모든 요청 400 → 시도당 2요청 × 호스트 2회 = **4**. 오버레이·`retry_spent` 단언 유지.

## 수락 기준

| ID | 관찰할 결과 | 태스크 | 검증 |
|---|---|---|---|
| AC-1 | 단일 경로에서 생성 **전체** 실패(스트림+비스트림) 1회 후 자동 재시도가 무장·상태줄 고지되고, 지연 뒤 성공 커밋되며 오버레이 없음 | T-1 | V-1 |
| AC-2 | 동일 전체 실패가 두 번이면 수동 오버레이, 호스트 3차 자동 시도 없음, HTTP 요청 수 = 4(시도당 stream+non-stream) | T-1 | V-1 |
| AC-3 | `host_automation` 전체·workspace test·clippy `-D warnings` 통과 | T-1 | V-1, V-2 |

## 태스크

| ID | 목적 | 수정 경계 | 선행 | 검증 | 상태 |
|---|---|---|---|---|---|
| T-1 | AC-1/2 테스트·주석을 스트리밍 시대 계약으로 갱신 | `apps/storytui/tests/host_automation.rs` (해당 2 테스트 + 주석 + `turn_stream_response`/`turn_content`) | 없음 | V-1, V-2 | done |

## 검증 계획

| ID | 방법 | cwd | 기대 | 실제 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p storytui --test host_automation` (`docs/INSTALL.md` / README) | repo root | 전부 통과 | 14 passed (구현 후) | PASS |
| V-2 | `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` | repo root | 클린·그린 | clippy exit 0; workspace 전부 ok (EXIT 0) | PASS |

## 평가

- G0 intake: PASS — 범위·AC·미결정 없음(이슈 제안 방향 = 테스트 갱신).
- G1 plan: PASS (self-review) — AC↔T↔V 연결, 확인된 명령만 사용.
- G2 task: PASS (self-review) — diff는 테스트 fixture/단언만; V-1·V-2 증거 있음.
- G3 code: PASS (independent, claude-sonnet-5-5) — F-* 없음. 리뷰어가 host_automation·clippy·workspace 재실행 확인.
- G4 design: N/A — UI/상호작용 변경 없음.
- G5 completion: PASS — 원 요청(#12)·AC-1..3·최종 working tree 일치.
- 최종 `/review` (independent): PASS — F-* 없음, G3 이후 드리프트 없음, host_automation/clippy/workspace 재실행 그린.
- 커밋·푸시·이슈 close는 사용자 요청 대기.

## 최종 AC 증거

| AC | 판정 | 증거 |
|---|---|---|
| AC-1 | PASS | `generation_failure_autoretries_once_and_succeeds` — 실패 2 + 재시도 SSE 성공 1, 무장·고지·커밋·오버레이 없음 |
| AC-2 | PASS | `second_failure_hands_control_to_the_manual_overlay` — 요청 4, 오버레이, 3차 호스트 시도 없음 |
| AC-3 | PASS | host_automation 14/14; clippy `-D warnings` exit 0; `cargo test --workspace` exit 0 |
