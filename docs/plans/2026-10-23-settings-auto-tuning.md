# 설정 자동 튜닝 — 샘플링 스타일 프리셋과 페르소나 편집

2026-10-23 · 요청: "다른 스토리형(SillyTavern, Risu)에 있는 기능들이 여기에는 많이 없어.
터미널의 한계일 수 있지만, 설정이 많은데 그것들이 전혀 없기도 해. 중요한 것은 사용자가
그것을 모르더라도 알아서 잘 설정되도록 하면 좋을 것 같아."

## 연구 (SDD discovery)

SillyTavern/Risu 대비 현재 갭(`docs/KNOWN-LIMITATIONS.md`, 설정 화면 `screens/settings.rs`,
`apps/storytui/src/config.rs` 기준):

1. **샘플링 편집 불가** — temperature/top_p/top_k/max_tokens는 설정 화면에 조회 전용으로
   표시되고 값 출처는 TOML뿐. KNOWN-LIMITATIONS가 "Editing those values in the TUI
   remains a future UX improvement"로 명시. ST의 AI Response Configuration / Risu 프리셋에
   해당하는 진입점이 아예 없다.
2. **페르소나 편집 불가** — `[player] name/persona`가 `pack.prompts.system`에 주입되는데
   (`host.rs:1088` `apply_player_profile`, 팩 로드·전환 시 1회 append), TOML 편집·재시작
   필요. ST/Risu의 페르소나 관리에 해당하는 UI가 없다.
3. 이미 자동 구성이 된 영역(로컬 서버 자동 발견·모델 자동 선택 `host/settings.rs::discover_session_provider`,
   로어북 자동 발동 T0+T1, 이미지 프로토콜 자동 감지)은 재설계 대상이 아니다.

## 목표와 범위

- **달라지는 결과**:
  1. 설정 화면에서 `x`로 스타일(샘플링 프리셋)을 자동 → 창의적 → 균형 → 정밀 순환한다.
     값이 즉시 반영되고 다음 턴부터 적용된다. 기본은 "자동" — 아무 것도 건드리지 않으면
     기존 TOML/프로필 튜닝값 그대로(알아서 잘 설정).
  2. 스타일 선택은 `user-settings.toml`에 영속되고 재시작·프로바이더 전환 후에도 유지된다.
  3. 설정 화면에서 `a`로 페르소나 모달(이름·페르소나 2필드)을 열어 Enter 확정하면 같은
     세션의 다음 턴 프롬프트부터 반영된다. Esc는 취소.
  4. 페르소나도 user-settings에 영속된다. TOML `[player]` 값은 상속 기본 — 편집하지
     않으면 TOML값이 그대로 쓰인다. 빈 값 확정 = 그 필드 끔.
- **제외 (가정 로그)**: raw 슬라이더 5개 편집(인지 부하 — 프리셋이 필요을 충족),
  로어북 뷰어·편집(별도 사이클), 스와이프 대안 캐시(히스토리 트리 필요 — 대형),
  컨텍스트 예산 자동 산출(`/v1/models`로 신뢰 불가), 새 게임 외 세션 도중 [player] TOML
  동기화(단방향: user-settings가 이기면 충분).

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | 설정에서 `x`를 누를 때마다 스타일이 자동→창의적→균형→정밀→자동으로 순환하고 샘플링 블록의 temperature/top_p/top_k 표시가 즉시 바뀐다 | T-2, T-3 | V-2 |
| AC-2 | 프리셋이 활성인 동안 다음 턴 요청이 프리셋 값으로 생성되고, "자동"으로 돌리면 활성 프로필의 기본 샘플링으로 복원된다. 선택은 재시작·프로바이더 전환 후에도 유지된다 | T-1, T-3 | V-1 |
| AC-3 | `a`로 연 모달에서 이름·페르소나를 입력해 Enter로 확정하면 같은 세션의 다음 턴부터 시스템 프롬프트에 반영된다(기존 절의 중복 없음). Esc는 아무 것도 바꾸지 않는다 | T-2, T-3 | V-1, V-2 |
| AC-4 | 편집하지 않으면 TOML `[player]` 상속이 그대로고, 빈 값 확정은 그 필드를 끈다. 저장은 user-settings에 영속된다 | T-1, T-3 | V-1 |
| AC-5 | 표현층 회귀 없음 — 설정 화면 80×24·`?` 도움말 두 컬럼·키맵에 새 행이 잘림 없이 들어가고, 미설정 기본값에서 기존 동작(값·스냅숏)은 의도된 행 추가 외 동일하다 | T-2 | V-2, V-4 |

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 태스크 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | `UserSettings`에 sampling_preset/player_name/player_persona 추가, `SamplingStyle` 파싱·라벨·값 트리플, `resolve_sampling`·`effective_player_profile` 순수함수, 라운드트립·우선순위 테스트 (AC-2,4) | apps/storytui/src/config.rs | 없음 | V-1 | done |
| T-2 | App 설정 분기에 `x` 사이클 플래그·`a` 페르소나 모달(2필드·Tab 포커스·붙여넣기), SettingsView/State 표시 필드, 설정 화면 행 추가, 키맵 행 2건, `App::handle_key` 레벨 테스트 (AC-1,3,5) | crates/story-tui/src/app.rs, screens/settings.rs, screens/keymap.rs | 없음 | V-2, V-4 | done |
| T-3 | 호스트 소비: `RuntimeConfig.base_sampling` 스냅샷(load·switch), 스타일 사이클 → 프리셋 적용·프로바이더 재빌드·영속·프로젝션 패치, 페르소나 확정 → 멱등 `set_player_profile_section`·pack 재적용·영속, `switch_provider` 스타일 재적용 (AC-2,3) | apps/storytui/src/host.rs, host/settings.rs | T-1 | V-1 | done |
| T-4 | 문서: UI-NAVIGATION.md 설정 키·변경 가능 항목, KNOWN-LIMITATIONS.md 샘플링 편집 한계 문구 갱신 (AC-5) | docs/ | T-2, T-3 | V-3 | done |

## 설계 메모

- **프리셋 테이블** (`SamplingStyle`): 창의적 (1.00/0.95/60), 균형 (0.85/0.92/40 = Tech
  Spec §10.2 기본), 정밀 (0.40/0.80/20). user-settings에는 스타일 id("creative" 등)만
  저장 — 알 수 없는 값·부재는 "자동". 순환 순서: 자동 → 창의적 → 균형 → 정밀 → 자동.
- **샘플링 우선순위**: 프리셋 활성 시 temperature/top_p/top_k 전체를 프리셋이 결정한다
  (user-settings.temperature 레거시 오버라이드·TOML 모두 덮음 — 더 거친 최근 노브가
  이긴다). max_tokens/context_budget는 기존 `apply_sampling_overrides` 그대로 별도.
  "자동" 복원원은 `RuntimeConfig.base_sampling` — 오버라이드 적용 전의 활성 프로필 원본
  트리플(load()에서 스냅샷, switch_provider는 clone 직후 갱신).
- **세션 적용 경로**: 샘플링은 요청 시 프로바이더가 읽으므로 기존 `apply_model` 패턴대로
  provider_config 갱신 → `OpenAiCompatibleProvider` 재빌드. 엔진 상태 무관.
- **페르소나 반영**: `apply_player_profile`의 append를 멱등 `set_player_profile_section`
  (기존 절 절단 후 재부여) 순수함수로 교체 — 절단 마커는 `\n\n# Player profile (StoryTUI)\n`로
  앱 고유 접미사를 붙여, 임포트 카드가 우연히 같은 제목의 절을 가져도 절단되지 않게 한다.
  기존 호출 지점(팩 로드 host.rs:104, 팩 전환 :528)이 같은 함수를 쓰므로 새 게임·전환
  경로도 자동 일관. 세션 중 확정 시에는 호스트가 pack을 다시 통과시켜 다음 턴부터 반영.
- **페르소나 우선순위**: user-settings 필드가 Some이면 `[player]`를 필드별로 이긴다.
  None = 상속, Some("") = 끔(명시적 해제). trim·공백 필터는 기존 apply_player_profile 규칙.
- **키 배정**: 설정 분기에서 `x/ㅌ`(스타일), `a/ㅁ`(페르소나) — 기존 소비 키(Esc,p,e,i,g,f,b,q)와
  충돌 없음. 페르소나 모달은 ImportDialog::Input의 버퍼·붙여넣기 처리를 재사용.
- **표현층 원칙** (피드백 메모리): 푸터에 키를 더하지 않고 — 설정 키맵 블록·`?` 도움말에만
  추가, 라벨 최단("스타일", "페르소나"), 설정 화면 80×24·도움말 두 컬럼 수용을 렌더 테스트로 확인.

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p storytui` — SamplingStyle 파싱(알 수 없는 id → 자동), resolve_sampling 우선순위(프리셋 > 레거시 temp > base), 자동 복원원(base_sampling), effective_player_profile 상속·해제, UserSettings 라운드트립, 페르소나 절 멱등 교체, switch_provider 스타일 재적용 배선 | repo root | 신규+기존 전부 통과 | 6 스위트 전부 ok — lib 28(+6: 사이클·리졸브·페르소나 상속·taste·라운드트립·기존 2건 갱신) + bin 42(+5: persona_section_is_idempotent_and_clears, persona_status_label, style_activation_uses_preset_then_restores_base_taste, sampling_style_survives_provider_switch — b 프로필 전환 후 0.4/0.8/20 재적용·base_sampling (0.85,0.92,40) 스냅샷 단언, persona_submit_persists_and_rewrites_the_pack_section — user-settings 우선·절 1회·빈 값 해제 단언) + 통합 15. `cargo test --workspace`에서도 동일 스위트 재통과 (2026-10-23) | PASS |
| V-2 | `cargo test -p story-tui` — `App::handle_key` 레벨: 설정에서 `x` 사이클 플래그·`a` 모달 진입·모달 내 입력/Tab/Esc/Enter 요청, 스냅숏 갱신, 키맵 두 컬럼 수용 테스트 | repo root | 신규+기존 전부 통과 | 5 스위트 전부 ok — lib 153(+6: settings_x_cycles_the_sampling_style_request, settings_a_opens_the_persona_dialog_prefilled, persona_dialog_edits_focuses_and_submits_raw_text, persona_dialog_absorbs_other_settings_keys_and_paste, persona_modal_renders_over_the_settings_screen, 기존 렌더 테스트 갱신) + settings_snapshot 3(갱신 수용) + 기존 통합 전부. 1차 실행에서 모달이 프리필을 유지·문자를 필드에 흡수하는 설계와 테스트 기대 불일치 2건을 발견 → 기대값 정정 후 통과 (2026-10-23) | PASS |
| V-3 | 게이트 5종 — `cargo test --workspace` + `cargo clippy --workspace -- -D warnings` + `cargo fmt --check` + `./scripts/check-core-deps.sh` + `./scripts/check-public-config.sh` | repo root | 회귀 0 | 최종 상태 기준 전부 통과 — workspace 63 스위트 ok(실패 0, 신규 스냅숏 포함), clippy 경고 0, fmt 정렬(cargo fmt가 신규 코드 정렬 5파일 적용 후 --check 통과), check-core-deps OK(ADR-003), check-public-config OK. 참고: gen 라벨 축소("2단계 스트리밍")·마커 접미사는 V-4/G3 결함 수정 후 재실행한 최종 증거 (2026-10-23) | PASS |
| V-4 | G4 사용성 — `cargo test -p story-tui settings_snapshot` 렌더 문자열 검토: 80×24에서 새 행(스타일·페르소나) 잘림 없음, 자동/없음 기본 표시, 도움말 두 컬럼에 새 키 수용 | repo root, TestBackend 80×24 | 새 행 가시·레이아웃 붕괴 없음 | 1차 렌더 검토에서 결함 1건: gen 라벨이 80컬럼에서 줄바꿈되어 persona 행이 상태 블록에서 잘림 → 라벨을 "2단계 (Director → Writer 스트리밍)"에서 "2단계 스트리밍"으로 축소(최단 라벨 원칙) 후 스냅숏 재수용 — loopback·manual 변환 모두 상태 7행(provider/tts/voice/image/gen/표정/persona)·샘플링 6행(style/temperature/top_p/top_k/max_tokens/theme) 손실 없이 확인. 페르소나 모달 80×24 렌더(app 테스트)에서 라벨·커서 ▌ 가시 확인. 실단말 육안 확인은 터미널 앱 특성상 사용자 실행(잔여) | PASS |

## 평가와 인계

- **G1 계획 평가**: PASS (self, plan 모드) — AC↔T↔V 전결합, 순수함수 경계 분리(config에서
  resolve·상속, host에서 적용·영속), 실패 상태(알 수 없는 스타일 id→자동, 빈 값 확정→해제,
  모달 Esc→무변경) 명시. 실행 중 설계 보강 2건: 절단 마커에 `(StoryTUI)` 접미사 추가(G3),
  gen 라벨 축소(V-4).
- **G3 코드 리뷰**: PASS (self-review — 이번 세션에 위임 에이전트 도구 없음, 스킬 규칙에
  따라 동일 루브릭 자체 검토·독립성 한계 명시). 검토 방법: 최종 diff 전체(dual 경계
  config/host/app/screen 6파일)를 plan AC·설계 메모와 대조. 발견 결함 1건:
  - F-1(P2, 수정 완료): 절단 마커가 일반 문자열(`\n\n# Player profile\n`)이라 임포트 카드가
    우연히 같은 제목 절을 가지면 카드 본문을 절단할 수 있었다(AC-3 위반 소지). 수정:
    `\n\n# Player profile (StoryTUI)\n` 고유 접미사 — 이 코드가 쓴 절만 매칭된다. 수정 후
    storytui 전 스위트·fmt·clippy 재통과(2026-10-23).
  - 비차단 P3 1건(후속 제안): `cycle_sampling_style`이 provider 재빌드 실패 시 in-memory
    sampling_preset만 갱신되고 영속·프로젝션은 이전 값 — 재빌드 실패는 클라이언트 빌드
    오류로 극히 드물고 다음 사이클에서 자기수렴하므로 조치 불요.
- **G4 사용성·디자인**: PASS (self, 구현 모드 — V-4 렌더 증거). 새 행 2건 최단 라벨,
  푸터 키 무증가(설정 키맵 블록·`?` 도움말로만 안내 — 인지 부하 원칙), 80×24에서 상태
  7행·샘플링 6행 손실 없음, 모달 라벨·커서 가시. 실단말 육안 확인 잔여는 사용자 실행.
- **G5 완료 판정**: PASS (self, completion 모드) — AC-1 사이클·즉시 표시(handle_key 레벨
  6건), AC-2 프리셋 우선·자동 복원·전환 생존(apply_style_to_config + switch 배선 테스트;
  실 요청 전송은 Mock E2E 범위 밖 — 잔여 한계), AC-3 세션 반영·멱등·Esc 무변경(절 교체 +
  모달 키 테스트), AC-4 상속·해제·영속(effective_player_profile + 라운드트립), AC-5
  스냅숏 3건·키맵 테스트·게이트 5종.
- **잔여 한계**: (1) 실단말 육안 확인(사용자 실행), (2) 프리셋 값의 실모델 품질 A/B 미실시
  (균형=기존 기본이므로 기본 경로 회귀 없음 — 창의적/정밀은 사용자 피드백으로 조정 가능),
  (3) 스타일 사이클의 app.update_settings_sampling 패치 4줄·run() 초기 refresh_view 인자
  배선은 직접 테스트 없음(호스트 통합 경로 특성 — 기존 사이클과 동일한 한계).
- 커밋은 사용자 요청 시에만.

## 가정 로그

- **범위 선택**: ST/Risu 갭 중 "설정이 아예 없는 것 + 자동 기본이 가능한 것"으로
  샘플링 프리셋·페르소나를 골랐다. raw 슬라이더는 인지 부하 피드백(2026-10-22)으로
  기각, 로어북 뷰어·스와이프 캐시는 크기 때문에 별도 사이클로 분리.
- **프리셋 값 근거**: 균형 = 기존 기본값(회귀 없음 보장), 창의적/정밀 = 로컬 내러티브
  모델 통상 범위의 중심값. 실모델 A/B는 잔여 한계로 남긴다 — 사용자 피드백으로 조정.
- **레거시 temperature 충돌**: user-settings.temperature가 있는 머신에서 프리셋을
  켜면 프리셋이 이긴다. "더 최근에 설정한 노브가 이긴다"는 기존 오버라이드 규칙과
  같은 방향이고, 해제(자동)하면 레거시 값이 복원된다.
