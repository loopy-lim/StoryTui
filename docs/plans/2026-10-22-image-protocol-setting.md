# 이미지 렌더 방식 자동 감지 + 수동 선택 설정 (2026-10-22)

## 목표와 범위

- 사용자 요청: "이미지 보이는 거 터미널을 알아서 찾아서 이미지 잘 (볼) 수 있도록 설정까지".
- 현재 상태(조사 근거): 시작 시 `Picker::from_query_stdio()` 1회 감지(`apps/storytui/src/host.rs:192-201`)가 이미 있고, 설정 화면에 감지된 백엔드 라벨 표시(`crates/story-tui/src/screens/settings.rs:243-260`). 없는 것: 사용자가 렌더 프로토콜을 직접 고르는 수단, 감지 실패 시 halfblocks 강제 수단.
- 이번에 만드는 것: 설정 화면 `b` 키(한글 ㅠ)로 이미지 렌더 방식 순환 전환(자동 → kitty → sixel → iterm2 → halfblocks → 자동). 선택은 `user-settings.toml`에 영속화, 전환 즉시 표시 중 이미지를 새 방식으로 재인코딩.
- 제외: 이미지 생성·프롬프트 파이프라인, F-18(이미지 꺼짐 상태 decode churn), kitty transmit-once 근본 해결(기존 F-4 유지).

### 가정 로그 (기본값으로 진행한 선택)

| 선택 | 값 | 근거 / 기각된 대안 |
|---|---|---|
| 전환 키 | `b` / ㅠ (Settings) | `i`는 on/off 토글로 유지(의미 분리). 표현층 정리 동반: keymap 행 + 푸터 `[B]` 추가 |
| 영속 필드 | `UserSettings.image_protocol: Option<String>` (kitty/sixel/iterm2/halfblocks, 없음=자동) | `Option<String>` + 적용 시점 검증 — typed enum + serde는 잘못된 값이 설정 파일 전체 파싱 실패로 번짐(`load_user_settings`가 Err 반환). 무효 값은 자동 취급으로 관용 처리 |
| iterm2 포함 | 순환에 포함 | `ProtocolType` 4종이 모두 컴파일됨(crossterm-only, docs 확인). 미지원 터미널 강제 시 화면이 깨질 수 있음 → KNOWN-LIMITATIONS에 기록 |
| 감지 실패 시 순환 | kitty/sixel/iterm2 건너뛰고 halfblocks만 허용 | 강제 kitty/sixel은 셀 크기(폰트 크기) 쿼리 결과가 필요 — `Picker::halfblocks()`는 쿼리 불필요(ratatui-image 11.1 소스 확인). 세션 중 stdio 재쿼리는 EventStream과 충돌 위험 → 하지 않음 |
| 전환 적용 방식 | host가 `reset_scene_images` + `refresh_scene_images` (표정 콜드 토글과 동일 패턴, `host.rs:860-865`) | `ProtocolType::next()`는 Auto를 표현할 수 없고 순환 순서도 달라(Halfblocks→Sixel→Kitty→Iterm2) 우리 순환과 맞지 않으며, 기존 인코딩된 이미지와 어긋남 — 전량 재인코딩이 계약상 안전 |

### 기준 상태와 환경

- 기준 리비전: `e896138` + 작업 트리의 커밋되지 않은 ux-simplify 등 변경 다수(보존 대상, 덮어쓰지 않음).
- 검증 도구: `cargo test --workspace`, `clippy -D warnings`, `fmt --check`, `./scripts/check-core-deps.sh`, `./scripts/check-public-config.sh` (전부 저장소 관례, cwd=저장소 루트).
- 평가자: 위임 가능(Agent). G1/G3/G5 독립 평가 예정.
- 실기기 한계: 이 환경(kitty/sixel 미지원 터미널)에서 kitty/sixel 실렌더 관찰 불가 — halfblocks 경로만 실관찰 가능.

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | 설정 화면에서 `b`(ㅠ)를 누르면 이미지 렌더 방식이 자동↔수동(kitty/sixel/iterm2/halfblocks)으로 순환되고, 현재 방식과 자동/수동 여부가 image 행에 표시된다 | T-2, T-3 | V-2, V-4 |
| AC-2 | 선택한 방식이 `user-settings.toml`에 저장되어 다음 실행에도 유지된다. 무효한 저장 값은 자동으로 취급되어 시작이 실패하지 않는다 | T-1, T-4 | V-1, V-3 |
| AC-3 | 자동(기본)은 기존 감지 동작과 동일하다. 감지 실패(피커 없음)일 때 순환은 halfblocks 강제만 제안한다 | T-4 | V-3 |
| AC-4 | 방식 전환 시 표시 중인 장면 이미지·표정 스프라이트가 새 방식으로 재인코딩되어 즉시 반영된다(재시작 불필요) | T-4 | V-3, V-6 |
| AC-5 | 기존 `i`(이미지 on/off)·`f`(표정) 토글과 설정 스냅샷 렌더가 회귀 없이 동작한다 | T-3, T-5 | V-1, V-4 |

## 설계

- **데이터 흐름**: `b` 키 → App 플래그 `image_backend_cycle_requested` → host dispatch가 소비 → `next_image_protocol_setting(현재값, 베이스 피커 가용)`으로 다음 값 계산 → `user_settings.image_protocol` 저장 → `build_image_picker(베이스, 설정)`으로 유효 피커 재구성(강제 시 `base.clone()` 후 `set_protocol_type`, halfblocks는 `Picker::halfblocks()`) → `app.set_image_picker_mode(picker, manual)` → reset+refresh 재인코딩.
- **책임 경계**: 순환·피지빌리티·영속 논리는 host(apps/storytui, ADR-003상 FS 접근은 host만). story-tui App은 플래그·피커 보관·라벨(자동/수동 마커)만. 설정 화면은 렌더만.
- **시작 적용**: host 시작 시 베이스 피커를 clone으로 보관, 저장된 `image_protocol`을 같은 `build_image_picker`로 적용(AC-2). 무효 값 → 자동 취급, 시작 실패 없음.
- **라벨 표현**(settings.rs image 행): `켜짐 · kitty (자동)` / `켜짐 · sixel (수동)` / `꺼짐 · halfblocks (수동)` / `터미널 미지원`(자동+피커 없음). off 상태에서도 방식 마커를 보여 `b` 전환이 보이게 유지. 블록 높이 변화 없음(행 추가 아님).
- **오류·경계**: 저장 실패 시 기존 i/f/g/tts 토글과 동일하게 `?` 전파 → main에서 "StoryTUI 실행 실패" 출력 후 종료(오버레이 아님 — G3 F-1 정정). 저장 성공 전에 메모리 값을 먼저 바꾸는 순서는 현재 종료 경로에서 관측 가능한 불일치가 없으나, 향후 오류 복구 경로 도입 시 mutate→save→apply 순서 재검토 필요(G3 F-2). 전환 직전 프로토콜의 잔상(kitty/sixel은 화면에서 능동 지우기 불가) 가능 → KNOWN-LIMITATIONS 기록.
- **UI 정보 구조**: 단축키 블록 Settings 그룹에 `b` 행 추가. 푸터는 추가하지 않기로 변경 — 80폭 최소 크기에서 `[B]` 추가 시 잘림이 발생하고, 이미지 on/off(`i`)도 푸터에 없다는 기존 관례와 일치. 대신 상태/샘플링 블록 분할을 40/60→50/50으로 균형 조정해 `(자동)/(수동)` 마커가 최악 행(`꺼짐 · halfblocks (수동)`, 37칸)에서도 감싸지지 않게 함.

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | 설정 영속 필드 추가 (AC-2) | `apps/storytui/src/config.rs` (`UserSettings.image_protocol`) | 없음 | V-1 | done |
| T-2 | App 플래그·키·피커 모드 (AC-1) | `crates/story-tui/src/app.rs` (핸들러 `b`/ㅠ, `set_image_picker(picker, manual)`, SettingsState 필드) | 없음 | V-2 | done |
| T-3 | 설정 화면 렌더·도움말 (AC-1, AC-5) | `crates/story-tui/src/screens/{settings.rs,keymap.rs}` (마커 라벨, keymap `b` 행, 50/50 분할) | T-2 | V-4 | done |
| T-4 | host 순환·적용·재인코딩 (AC-2~4) | `apps/storytui/src/host.rs` (베이스 피커 보관, 순환·피커 헬퍼, dispatch, 시작 적용) | T-1, T-2 | V-3 | done |
| T-5 | 스냅샷·문서 갱신 (AC-5) | `crates/story-tui/tests/snapshots/*settings*`, `docs/UI-NAVIGATION.md`, `docs/KNOWN-LIMITATIONS.md` | T-3 | V-4 | done |

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test --workspace` (저장소 관례 게이트) | 저장소 루트 | 전체 테스트 통과 | 전 스위트 ok — story-tui lib 148(신규 b키·렌더 마커 테스트 포함), storytui bin 36(순환·피커 헬퍼 테스트 포함), config 라운드트립·무효값 테스트 통과 (2026-10-22 실행) | PASS |
| V-2 | `App::handle_key` 레벨 테스트(실제 입력 체인 — settings visible + `b`/`ㅠ` → 플래그) + `set_image_picker` 라벨 테스트 | 저장소 루트 | 플래그 소비 확인, 라벨 "kitty"+manual 등 | `settings_b_key_requests_the_backend_cycle_once`(b·ㅠ·소비 1회) ok, `settings_screen_renders_the_forced_backend_marker`(TestBackend 실렌더에서 `halfblocks (수동)` 확인 — App→SettingsView 전파 고정) ok | PASS |
| V-3 | host 순환 헬퍼 테이블 테스트(피지빌리티 스킵 포함) + 저장된 설정이 시작에 적용되는 경로 빌드·테스트 확인 | 저장소 루트 | (auto,on)→kitty, (auto,off)→halfblocks, (halfblocks,·)→auto, 무효 값("kity")→자동(오류 없음) | `image_protocol_cycle_skips_unavailable_protocols` ok(전 케이스+무효값), `build_image_picker_forces_only_recognized_values` ok(강제는 베이스 필요, halfblocks는 항상 가능) | PASS |
| V-4 | 설정 스냅샷 갱신 + `cargo insta`/테스트로 라벨 확인, 80폭 잘림 검토 | 저장소 루트 | "…(자동)/(수동)" 라벨, 키 도움말 행 | 스냅샷 3건 수용 전 diff 검증: loopback("켜짐 · kitty (자동)"+50/50 분할), non_loopback(분할+미지원 행 한 줄 복원), 신규 manual("꺼짐 · halfblocks (수동)") — 모든 행 블록 안 유지, 잘림 없음. keymap `b` 행은 80×24 스냅샷 높이 제한상 미표시(더 큰 화면에서 표시) — 행 길이·그룹핑은 `every_row_is_grouped_and_one_line_short`, 존재는 `keymap_documents_every_story_shortcut` expected 목록에 "b" 추가로 고정(G5 F-1 반영) | PASS |
| V-5 | `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `./scripts/check-core-deps.sh`, `./scripts/check-public-config.sh` | 저장소 루트 | 전부 통과 | clippy 경고 1건(needless_range_loop) 수정 후 통과, fmt 적용 후 --check OK, `OK: story-core has no UI/HTTP dependencies`, `OK: public Git content contains no private model selections` | PASS |
| V-6 | G4 실관찰: 앱 실행(강제 halfblocks 설정) 후 설정 화면 관찰 | tmux 100×32 세션, TERM=xterm-256color, 격리 save-dir, 죽은 루프백 공급자 | 저장 설정이 시작 라벨에 반영, `b` 순환 실시간 반영 | 시작 시 `image (i) : 켜짐 · halfblocks (수동)` — 저장된 override 적용 확인. `b` 3회: halfblocks(수동)→(자동)→kitty(수동)→sixel(수동), 설정 화면 기준 TUI 프레임 붕괴 없음(이미지가 실제 렌더되는 스토리 화면 상태는 아니었음 — 미지원 강제 시 raw 출력 잔상 한계는 KNOWN-LIMITATIONS 유지, G3 F-3). 순환마다 `user-settings.toml` 실제 기록(최종 `"sixel"`). 자동 감지가 이 터미널에서 halfblocks로 폴백함도 함께 확인(수동 override 필요성 실증). kitty/sixel 실픽셀 렌더·스토리 화면 재인코딩 시각 관찰은 이 환경(그래픽 미지원·플레이 불가)에서 불가 — 단위 테스트+스냅샷이 대체 | PASS (범위 한계 명시) |

## 평가와 인계

- G1: 독립 평가자(validate-plan/plan) **PASS** — P3 3건(F-1 문구, F-2 AC-4↔V-6 매핑, F-3 무효값 사례) 모두 계획 문서에 반영. G2: self-review — 태스크별 증거는 V-1~V-5, 통합 확인은 workspace 테스트+실관찰로 수행. G3: 독립 평가자(review-changes) **PASS** — P3 3건(F-1 오류 경로 문구, F-2 mutate→save 순서 주의, F-3 V-6 서술 정밀화) 모두 문서 반영, 코드 결함 0건, simplify 불요(유지보수성 지적 없음). G4: self 관찰(V-6) — 실관찰 가능 범위 halfblocks 경로. G5: 독립 평가자(completion) **PASS** — AC-1~5 전부 PASS, 독립 재실행(test/clippy/fmt/scripts) exit 0, P3 1건(keymap 존재 고정 문구)을 expected 목록 "b" 추가로 해소. 잔여 한계: kitty/sixel/iterm2 실픽셀 렌더는 이 환경에서 관찰 불가(단위·스냅샷 증거로 대체), V-6 tmux 세션은 종료되어 재실행 불가(소스·로직 대조로 정합 확인).
- 커밋은 사용자가 요청할 때만.
