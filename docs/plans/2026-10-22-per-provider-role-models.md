# 프로바이더 프로필별 역할 모델 배정

2026-10-22 · 요청: "지금 설정 보니까 여러개의 provider를 설정하고, 그거에 따라서 넣어주는거
변경해야하는거 아니야??" / "그래서 알아서 여러개로 할 수 있도록 하자. auth만 안되고."

## 연구 (SDD discovery)

기존 구현([[providers]] 로스터, `[E]` 선택자, `apply_provider` 런타임 전환)은 이미 있고,
갭은 "전환 시 함께 바뀌지 않는 값"에 있다:

1. **역할 배정(director/choice/writer)이 전역** — `FileConfig` 상위 레벨 1세트
   (`config.rs:17-25`), user-settings도 flat 필드(`config.rs:64-68`). 프로필 전환 시
   새 서버가 광고하지 않으면 active model 하나로 무너뜨리고(`host/settings.rs:366-381`)
   원래 프로필로 돌아와도 복구되지 않는다.
2. **요청 주입은 세션 진실원 하나** — `RuntimeConfig.{director,choice,writer}_model`만 읽는다
   (choice refresh `host.rs:1103`, Director `host.rs:1149`, Writer `host.rs:1180`).
   따라서 세션 필드만 프로필에 맞게 복원하면 주입 경로는 무변경이다.
3. **샘플링 오버라이드도 전환 시 묻힘** — 시작 때만 `UserSettings::apply_to`가 적용되고
   (`config.rs:94-108`), `apply_provider`는 `selected.clone()`이라 재적용되지 않는다
   (`host/settings.rs:340`).
4. 시작 프리플라이트의 stale 수리는 `repaired_stage_models` 플래그로 flat 필드에 영속한다
   (`host.rs:152-168, 282-286`) — 프로필 키가 없다.

## 목표와 범위

- **달라지는 결과**:
  1. `[E]` 프로바이더 전환 시, 그 프로필에 저장된 역할 배정이 복원된다. 저장된 배정이
     없으면 현재 배정이 유지되고(서버가 광고하는 한) 기존 수리 동작은 그대로다.
  2. UI에서 역할 모델을 바꾸면 현재 프로필 이름으로 저장된다 — 전환 후 복귀·재시작에도 유지.
  3. `[[providers]]` 항목에 `director_model`/`choice_model`/`writer_model`을 쓰면 그
     프로필의 기본 배정이 된다. 우선순위: user-settings 프로필 항목 > 프로필 TOML 필드 >
     기존 전역 기본.
  4. 프로바이더를 전환해도 user-settings의 샘플링 오버라이드(temperature/max_tokens/
     context_budget)가 유지된다(머신 수준 취향).
- **제외 (사용자 지정)**: auth/자격증명 처리 — `api_key_env`/`api_key_file` 해석 경로를
  변경하지 않는다. `model`(채팅 모델)의 프로필 귀속도 제외 — 기존 전역 오버라이드 + 전환 시
  stale 수리가 의도한 동작이며, 이번 갭이 아니다.
- **호환 근거**: 기존 `user-settings.toml`의 flat director/choice/writer는 로드 때 활성
  프로필의 맵 항목으로 폴딩하고(맵에 해당 프로필 항목이 없을 때만) flat 필드를 비워 다음
  저장 때 마이그레이션을 완료한다. 요청 주입 경로·설정 화면 렌더는 무변경이다.

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | 프로필 A에서 B로 전환하면 B의 저장된 배정이 설정 화면 공급자 블록에 표시되고, A로 돌아오면 A의 배정이 복원된다 | T-2, T-3 | V-1 |
| AC-2 | UI에서 역할 모델을 바꾸면 현재 프로필 이름으로 영속되고 재시작 후에도 유지된다 | T-1, T-2 | V-1 |
| AC-3 | `[[providers]]`의 역할 필드가 그 프로필 전환(또는 시작 선택) 시 기본 배정으로 적용되고, user-settings 프로필 항목이 있으면 그것이 이긴다 | T-1, T-2 | V-1 |
| AC-4 | 전환 후에도 user-settings 샘플링 오버라이드가 활성 프로필에 유지된다 | T-1, T-2 | V-1 |
| AC-5 | 회귀 없음 — 단일 프로필 흐름(시작 수리, 전환 수리, 임포트 roster), auth 경로 동결, 게이트 5종 통과 | T-3, T-4 | V-2 |

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 태스크 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | `StageModels`+프로필 맵 영속, legacy 폴딩, 시딩 우선순위, sampling 분리, `ProviderProfile`(flatten)+`stage_defaults` 조립 (AC-2,3,4) | apps/storytui/src/config.rs | 없음 | V-1 | done |
| T-2 | `resolve_stage_assignments` 순수 함수, `apply_provider` 복원·수리·영속, `apply_*_model` 프로필 키 저장, refresh 진실원 변경 (AC-1,2,4) | apps/storytui/src/host/settings.rs | T-1 | V-1 | done |
| T-3 | 시작 `repaired_stage_models`의 맵 영속, flat 쓰기 제거, roster 수용 (AC-1,5) | apps/storytui/src/host.rs | T-2 | V-2 | done |
| T-4 | `provider_roster`(임포트) 수용 + KNOWN-LIMITATIONS·UI-NAVIGATION 문서 (AC-3,5) | apps/storytui/src/config.rs, docs/ | T-1 | V-2 | done |

## 설계 메모

- **세션 진실원 유지**: `RuntimeConfig.{director,choice,writer}_model`만 요청 주입이 읽는다.
  복원·수리는 이 필드를 쓰고, 표시도 이 필드를 읽는다(`refresh_projection`이 user-settings를
  읽던 것을 수정). user-settings 맵은 영속 계층일 뿐이다.
- **우선순위 사슬** (프로필 P, 스테이지별 독립): `user_settings.stage_models[P]` >
  `RuntimeConfig.stage_defaults[P]`(TOML `[[providers]]` 필드) > 기존 전역 기본
  (FileConfig 상위 필드). None은 항상 "다음 수준 상속"을 의미한다 — 배정을 지우는 UI는
  없으므로 Some만 덮어쓴다.
- **전환 알고리즘** (`resolve_stage_assignments`, 순수 함수): 스테이지마다
  `saved → profile_default → current(계승)` 순으로 desired를 정하고, advertised가 비어
  있지 않고 desired가 광고되지 않으면 active model로 수리한다(기존 의미 유지).
  `map[name]` 영속 조건은 `saved 존재 || 수리 발생` — 계승만으로 생긴 값을 굳이
  기록하지 않아 나중에 추가된 TOML 기본값이 묻히지 않게 한다.
- **sampling 재적용**: `UserSettings::apply_to`를 `apply_sampling_overrides`(max_tokens,
  context_budget, temperature)와 model 적용으로 분리. 로드는 둘 다, 전환은 sampling만.
  전환 시 provider 재빌드는 오버라이드가 반영된 clone으로 한다.
- **auth 동결**: `api_key_env`/`api_key_file`/`api_key` 해석은 `load()`와
  `provider_roster()`의 기존 코드를 그대로 둔다. `ProviderProfile`은 `#[serde(flatten)]`
  으로 ProviderConfig를 감싸 자격증명 필드 구성을 바꾸지 않는다.

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p storytui` — 시딩 우선순위, resolve 순수 함수(복원/기본/계승/수리), `apply_provider` 복원 배선(닫힌 루프백 포트 — advertised 공백 시 수리 없이 복원만 적용), UserSettings 라운드트립, 기존 config 테스트 갱신분 | repo root | 신규+기존 전부 통과 | 6 스위트 전부 ok — lib 22(config, 신규 3: profile_toml_role_fields…/stage_models_outrank…/legacy_flat… fold) + bin 34(settings, 신규 5: resolve 3 + switch_provider 배선 2) + 4. `switch_restores_saved_stage_assignments_without_a_server`는 저장 항목 복원·계승 필드 유지·프로필명 영속까지 단언 (2026-10-22) | PASS |
| V-2 | `cargo test --workspace` + `cargo clippy --workspace -- -D warnings` + `cargo fmt --check` + `./scripts/check-core-deps.sh` + `./scripts/check-public-config.sh` — 게이트 5종, story-tui 스냅샷 무변경 포함 | repo root | 회귀 0 | workspace 63 스위트 전부 ok(스냅샷 포함 — UI 렌더 무변경 확인), clippy 경고 0(신규 테스트의 field_reassign_with_default 1건 수정), fmt 정렬, ADR-003·public config 양호. 참고: fmt가 본 사이클 이전 미커밋 변경 2곳(apps/storytui)의 정렬만 함께 corrected (2026-10-22) | PASS |

## 평가와 인계

- **G1 계획 평가**: PASS (self, plan 모드) — 보강 1건 반영(V-1에 apply_provider 배선 테스트 추가).
- **G3 코드 리뷰**: PASS (independent, 위임 에이전트). 우선순위 사슬 일관성, legacy 폴딩 멱등·무손실, persist 조건 설계 일치, flatten 호환·auth 동결, 표시 동등, V-1/V-2 재현(수치 일치) 확인. 비차단 P3 2건은 후속 제안으로 분리:
  - F-1(P3): 시작 수리가 1건이라도 발생하면 나머지 필드까지 맵에 고정돼, 이후 TOML 기본값 편집이 반영되지 않을 수 있음(구 flat 영속과 같은 형태, 회귀 아님). 후속: 필드별 변경분만 영속.
  - F-2(P3): saved 엔트리가 있는 프로필 전환 시 계승 필드도 함께 기록(테스트가 명시 단언한 설계 동작). F-1과 같은 귀결, 조치 불요.
- **G4 사용성·디자인**: N/A — 렌더 코드·화면 흐름 무변경(설정 화면은 기존 공급자 블록을 그대로 사용). 관찰되는 값 변화는 AC-1 배선 테스트와 무변경 스냅샷으로 검증.
- **G5 완료 판정**: PASS (self, completion 모드) — AC-1 전환 복원(V-1 배선+순수함수), AC-2 프로필 귀속 영속(형식 직렬화+시작 재시딩 테스트, apply_*_model 쓰기 3줄은 직접 테스트 없음 — 잔여 한계 기록), AC-3 우선순위(outrank 테스트), AC-4 샘플링 유지(배선 테스트), AC-5 게이트 5종·auth 동결. 실단말 육안 확인(두 프로필 오가기)은 터미널 앱 특성상 사용자 실행.
- 커밋은 사용자 요청 시에만.
