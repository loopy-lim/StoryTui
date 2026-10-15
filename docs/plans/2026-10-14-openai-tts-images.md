# 계획: OpenAI 호환 프리셋 + TTS 내레이션 낭독 + 팩 이미지 표시

## 목표와 범위

- **사용자에게 달라지는 결과**
  1. `preset = "openai"` 한 줄로 OpenAI 클라우드 API(api.openai.com)를 채팅 모델 서버로 사용한다.
  2. `v` 키로 내레이션을 소리로 읽게 하고, 새 턴이 완료될 때마다 자동 낭독한다(macOS `say` 기본, OpenAI TTS 선택).
  3. 스토리 팩에 `images/` 폴더로 이미지를 넣으면 스토리 화면에 표시한다(kitty 그래픽 프로토콜 — Ghostty/kitty/WezTerm).
- **순서 요구**: 사용자 지시 "가장 하기 쉬운 것부터" → A(프리셋) → B(TTS) → C(이미지) 순 구현.
- **제외 범위**: AI 이미지 생성(다음 단계), 음성 입력(STT), PRD에 명시된 다른 비목표.
- **현재 구조와 근거** (조사 완료, 리비전 `1193602` 클린 트리):
  - 프로바이더는 이미 `openai_compatible` + rustls TLS (`crates/story-provider/src/openai_compatible.rs`). 남은 격차: `/v1/models` 경로 하드코딩(openai_compatible.rs:156), `health()`가 정확히 200만 Ready(openai_compatible.rs:670-696), `max_tokens` 키 고정(envelope.rs:79), 프리셋 없음, `kind` 미검증.
  - 턴 완료 시 내레이션은 `EngineEffect::PublishView`(engine.rs:682-684, 1347-1348) → host.rs:500 `app.set_view`로 도착. 비동기/프로세스 스폰은 전부 `apps/storytui`(tokio::main select 루프).
  - `AppViewModel`은 location id 미포함(view.rs:33, 표시명만). 호스트는 `EngineState`+`StoryPack.pack_root`를 보유(host.rs:495) → 이미지 경로 해석은 host 층이 맡는다.
  - `UserSettings`는 Option 필드 + `deny_unknown_fields` 없음 → 필드 추가 시 마이그레이션 불필요(apps/storytui/src/config.rs:47-63).
  - 팩 로더는 텍스트 전용(loader.rs, UTF-8 강제) → 이미지는 로더 우회, `pack_root/images/` 컨벤션.
  - `ratatui-image` 11.1.0이 ratatui ^0.30.1과 호환(crates.io 확인 2026-10-14).
  - `generated_turn` JSON 스키마는 이미 `additionalProperties:false` + 전체 required(engine.rs:2118+) → OpenAI strict 응답 형식과 호환 예상. 실패해도 `structured_output="auto"`가 세션 폴백 제공.
- **가정**: 사용자 터미널은 kitty 프로토콜 지원(Ghostty/kitty/WezTerm — G0에서 확인). macOS이므로 `say`/`afplay` 내장.
- **환경/평가자**: cargo 워크스페이스 테스트·clippy·fmt 사용 가능(README.md:22-24). 실시간 TUI 실행 검증은 로컬 omlx 서버(127.0.0.1:8000) 가용성에 의존 — 불가 시 해당 검증만 BLOCKED 표시.

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | (⑦로 수정) 호환 서버의 `base_url`(+키+모델)만으로 프로필이 완성된다 — `/v1` base_url의 경로 자동 정규화, `max_tokens_field` 등 일반 필드로 서버 차이를 흡수한다. | T-1, T-2 | V-1, V-2 |
| AC-2 | (⑦로 수정) `/health`가 없는 호환 서버에서 Settings 화면 공급자 상태가 Unavailable이 아닌 Ready로 표시된다(404 → 모델 목록 폴백). | T-1, T-2 | V-1 |
| AC-3 | (⑦로 수정) `max_tokens_field = "max_completion_tokens"` 설정 시 요청 본문이 해당 키를 사용한다(서버별 키 차이를 일반 설정으로 흡수). | T-1 | V-1 |
| AC-4 | 기존 로컬(omlx) 설정은 기존과 동일하게 동작한다 — `/v1`이 아닌 base_url에서 정규화는 발동하지 않고, 모든 기존 테스트가 회귀 없이 통과한다(`top_k=40`, `max_tokens` 키 유지). | T-1, T-2 | V-5 |
| AC-5 | `config/README.md`에 OpenAI 클라우드 설정 예시(비밀값 없음)가 있다. | T-3 | V-5 |
| AC-6 | `v` 키로 낭독 on/off 전환, 켜져 있으면 새 턴 완료 시 내레이션을 `say`로 읽는다(macOS). | T-4, T-5 | V-3, V-6 |
| AC-7 | TTS on/off 상태가 `user-settings.toml`에 유지되고 Settings 화면에 표시된다. | T-5 | V-2, V-6 |
| AC-8 | `[tts] backend = "openai"` 설정 시 OpenAI TTS API로 낭독한다(`api_key_env` 필요, mp3 → `afplay`). | T-4 | V-3 |
| AC-9 | 팩에 `images/locations/<location-id>.png`(폴백: `images/characters/<char-id>.png`)가 있고 터미널이 kitty 프로토콜을 지원하면 스토리 화면에 표시된다. | T-6, T-7 | V-4, V-6 |
| AC-10 | 이미지 파일 부재·미지원 터미널·좁은 폭에서는 기존과 동일한 텍스트 레이아웃으로 렌더링된다(회귀 없음). | T-7 | V-4, V-5 |
| AC-11 | `[[providers]]`로 여러 프로바이더 프로필을 정의할 수 있다(이름 중복 시 한국어 오류로 시작 거부; 기존 `[provider]`는 기본 프로필로 하위 호환). | T-9 | V-2 |
| AC-12 | Settings(`o` → `e`)에서 프로바이더를 고르면 재시작 없이 전환된다 — 새 엔드포인트·키로 공급자 재구성, health·모델 목록 갱신, 새 서버에 없는 저장 모델은 활성 모델로 수리. | T-10 | V-2 |
| AC-13 | 선택한 프로바이더는 `user-settings.toml`의 `provider`에 유지되어 다음 실행에서 자동 선택되고, 사라진 이름은 기본 프로필로 경고와 함께 폴백한다. | T-9 | V-2 |
| AC-14 | 생성 로딩 중 재그리기가 경과 초 변화(1Hz)와 실제 상태 변경 때만 발생한다 — 16ms 틱마다 풀 프레임을 그리지 않는다. | T-12 | V-8 |
| AC-15 | 스토리 스택 렌더용 라인 조립이 스택 변경 시 1회만 수행되고(버전 캐시), 스트리밍 중에는 신규 엔트리만 매 프레임 조립된다. | T-13 | V-7, V-8 |
| AC-16 | 마우스 이동만으로는 재그리기가 발생하지 않는다(클릭·휠 등 상태 변경 이벤트만). | T-12 | V-8 |
| AC-17 | 기존 렌더 출력이 회귀 없이 동일하다(스냅샷·스토리 화면 테스트 유지). | T-13 | V-5 |
| AC-18 | 전체 화면·전환 그래프가 문서로 존재하고 코드와 일치한다 (`docs/UI-NAVIGATION.md`). | T-15 | V-9 |
| AC-19 | 설정(`i`)에서 장면 이미지 표시를 켜고 끌 수 있고(즉시 적용·재활성 즉시·선택 유지), 설정 화면에 감지된 프로토콜과 상태가 표시된다. | T-16 | V-10 |
| AC-20 | 프로바이더 선택기 클릭 선택 등 마우스 상호작용이 테스트로 고정된다. | T-17 | V-10 |

## 설계

### A. OpenAI 호환 프리셋 (story-provider + apps/storytui)

- `ProviderConfig` 신규 필드(모두 serde default → 기존 TOML 무영향):
  - `models_path: String` (기본 `"/v1/models"`) — `list_models` URL이 하드코딩 대신 이 값을 사용.
  - `max_tokens_field: MaxTokensField` (enum: `MaxTokens` 기본 / `MaxCompletionTokens`) — envelope 직렬화 키 선택. 로컬 서버 호환 위해 기본은 기존 키.
  - `preset: String` (기본 `""`).
- `ProviderConfig::apply_preset(&mut self)`: `"openai"`일 때 기본값과 동일한 필드만 교체(명시 커스텀 존중): name→`"openai"`, base_url→`https://api.openai.com/v1`, chat_path→`/chat/completions`, models_path→`/models`, health_path→`/models`, top_k 40→0(전송 생략), max_tokens_field→`MaxCompletionTokens`. 기본값 감지 방식의 한계(기본값과 같은 값을 의도적으로 쓴 경우 덮어짐)는 문서화.
- 적용 시점: `apps/storytui/src/config.rs::load`에서 파일 파싱 직후 — user-settings·CLI 오버라이드보다 먼저.
- `kind` 검증: `"openai_compatible"` 이외 값은 에러가 아니라 `tracing::warn` (하위호환 유지).
- health: 프리셋이 `health_path="/models"`로 만들어 인증된 GET 200 → `Ready`. 로컬은 기존 `/health` 그대로.
- structured_output: 스키마가 이미 strict 규칙 준수로 보이므로 프리셋이 건드리지 않음. OpenAI 400 시 `auto` 폴백이 처리.

### B. TTS 낭독 (신규 크레이트 story-tts + host/app/settings 연결)

- **crates/story-tts** (story-model에만 의존, reqwest+tokio):
  - `TtsConfig { enabled, backend(say|openai), voice, openai_model, base_url, api_key_env, api_key_file }` — serde default 완비.
  - `SayBackend`: `command_for(text, voice) -> std::process::Command` 순수 함수(테스트 가능) + spawn/kill 실행부.
  - `OpenAiTtsBackend`: `POST {base_url}/audio/speech` (model, voice, input, response_format=mp3) → temp mp3 → `afplay` spawn.
  - `TtsPlayer`: 현재 재생 child 1개 보관. `speak(text)` = 기존 kill → 새 spawn(논블로킹), `stop()`, `Drop`으로 잔여 프로세스 정리. 자식 stdout/stderr는 `Stdio::null()`(TUI 화면·로그 오염 방지). 재생 실패(예: `afplay` 부재, 보이스 없음)는 `tracing::warn` 이후 무시 — 낭독 실패가 앱을 죽이거나 턴 생성을 막지 않는다.
  - 텍스트 정리: `speakable_text()` — 마크다운 기호(`#`, `*`, `` ` ``, `>`, `[]()`), 스피커 라벨 유지(대사는 그대로 읽음).
- **설정 배선**: `FileConfig`에 `[tts]` 블록, `UserSettings`에 `tts_enabled: Option<bool>`(저장 유지). SettingsView에 tts 상태 필드 추가(비밀 없음 — 기존 no-secret 테스트 유지).
- **호스트 훅**: host select 루프의 `PublishView` 처리에서 턴 증가 && enabled일 때 `tts.speak(view.narration)`(say는 즉시 spawn, openai는 `tokio::spawn`). 선택지 재생성(refresh)은 턴이 안 늘어 자동 제외.
- **키 토글**: story 화면 `v` → App 플래그 → `take_tts_toggle_request()` → host `handle_ui_requests`에서 토글·`stop()`·user-settings 저장·설정 갱신. 로딩 중 가드는 기존 정책 유지(토글은 로딩 해제 후).
- Settings 화면에 TTS 섹션(상태·백엔드·보이스·`v` 안내). snapshot 테스트 갱신. `screens/keymap.rs`의 `KEYMAP` 정적 표에 `v` 낭독 항목 추가(Settings 키맵 블록 표시 동기화).

### C. 팩 이미지 표시 (host 경로 해석 + story-tui 렌더)

- **story-tui**에 `ratatui-image = "11.1"` + `image = "0.25"` 의존 추가(동작 feature 기본값: kitty/sixel/halfblocks 포함).
- **호스트 이미지 해석**(apps/storytui, FS 접근 허용 층): 매 `PublishView`마다
  `pack_root/images/locations/<location-id>.{png,jpg,jpeg,webp}` → 없으면 `pack_root/images/characters/<주요 캐릭터 id>.{...}` → 없으면 `None`. `App::set_scene_image(path)`로 전달. story-core/story-model/persistence 무변경.
- **렌더**: `render_story`에서 이미지 존재 && 터미널 폭이 텍스트 최소폭+이미지 열을 수용하면 내레이션 영역을 수평 `[Min(0) 텍스트, Length(이미지 열)]` 분할. ratatui-image `Picker`를 시작 시 1회 감지(`from_query_stdio` 계열, 실패 시 이미지 비활성+tracing 로그). 이미지 픽셀→셀 폭 계산은 ratatui-image API 사용.
- **일관성**: 선택지·상태 패널과 `choice_index_at`/`home_shortcut_at` 히트테스트는 내레이션 분할 영향 없음. 내레이션 스크롤바·줄 수 계산만 분할된 너비 사용. 이미지 `None`이면 기존 레이아웃 그대로.
- 테스트: `TestBackend` + halfblocks 프로토콜로 렌더 유닛 테스트(실제 터미널 쿼리 불필요).

### 위험·호환

- envelope 키 변경은 기본값 뒤에 숨음(로컬 서버는 기존 `max_tokens`) — 계약테스트로 이중 확인.
- `Picker` 터미널 쿼리는 raw mode에서 실패할 수 있음 → 실패 시 조용히 텍스트 전용(AC-10).
- TTS 자식 프로세스는 앱 종료·토글 off·새 턴 시작 시 kill — 방치 없음.

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | AC-1,2,3,4: `models_path`·`max_tokens_field`·envelope 키 선택 + 계약테스트 | crates/story-provider (config.rs, envelope.rs, openai_compatible.rs, tests/) | 없음 | V-1 | done |
| T-2 | AC-1,2,4: `preset` 필드 + `apply_preset` + config 로드 배선 + kind warn | crates/story-provider/src/config.rs, apps/storytui/src/config.rs | T-1 | V-2 | done |
| T-3 | AC-5: OpenAI 클라우드 문서 + 템플릿 예시 | config/README.md, config/storytui.toml | T-2 | V-5 | done |
| T-4 | AC-6,8: story-tts 크레이트(say+openai 백엔드, TtsPlayer) | crates/story-tts/** (신규), Cargo.toml workspace | 없음 | V-3 | done |
| T-5 | AC-6,7: `[tts]` 설정·user-settings 토글·`v` 키·턴 완료 훅·Settings 섹션 | apps/storytui (config.rs, host.rs, host/settings.rs), crates/story-tui (app.rs, screens/settings.rs, keymap.rs) | T-4 | V-2, V-6 | done |
| T-6 | AC-9: 호스트 이미지 경로 해석 + App 전달 | apps/storytui (host.rs, host/images.rs 신규), crates/story-tui/src/app.rs | 없음 | V-4 | done |
| T-7 | AC-9,10: ratatui-image 렌더 + 프로토콜 감지 + 폴백 + 테스트 | crates/story-tui (Cargo.toml, screens/story.rs), apps/storytui (TerminalGuard/Picker 초기화) | T-6 | V-4 | done |
| T-8 | 통합: 전체 테스트·clippy·fmt + (가능하면) 실서버 수동 확인 | 없음(검증만) | T-1..T-7 | V-5, V-6 | done |
| T-9 | AC-11,13: `[[providers]]` 복수 프로필(이름 검증·프로필별 키 파일 해석·기본 프로필 로스터 포함·user-settings 선택) | apps/storytui/src/config.rs | T-2 | V-2 | done |
| T-10 | AC-12: 프로바이더 선택기(ListMode::ProviderPicker, `e` 키) + `apply_provider` 런타임 전환 | crates/story-tui (app.rs, screens/story.rs, keymap.rs), apps/storytui (host.rs, host/settings.rs) | T-9 | V-2, V-4 | done |
| T-11 | AC-11,12 문서: README 다중 프로바이더 섹션 | config/README.md | T-10 | V-5 | done |
| T-12 | AC-14,16: 로딩 1Hz 그리기 게이트 + 마우스 이동 무재그리기 | apps/storytui/src/host.rs | 없음 | V-8 | done |
| T-13 | AC-15,17: 스택 라인 버전 캐시 + render_story 텍스트 사전 조립 시그니처 | crates/story-tui (app.rs, screens/story.rs, lib.rs), tests/render_perf.rs 신규 | 없음 | V-7, V-5 | done |
| T-14 | 성능 증분 통합 검증·리뷰 | 없음 | T-12, T-13 | V-8 | done |
| T-15 | AC-18: 전체 탐색 그래프 문서 | docs/UI-NAVIGATION.md (신규) | 없음 | V-9 | done |
| T-16 | AC-19: 이미지 토글(`i`)+프로토콜 상태 표시+user-settings 유지 | crates/story-tui (app.rs, screens/settings.rs, keymap.rs), apps/storytui (host.rs, host/settings.rs, config.rs) | 없음 | V-10 | done |
| T-17 | AC-20: 마우스 클릭 커버리지 테스트(프로바이더 선택기) | crates/story-tui/src/app.rs tests | T-16 | V-10 | done |

**계획 변경 기록**: ① `say -v ?` 신형 macOS 출력("Eddy (한국어(한국))  ko_KR")에서 보이스명에 공백·괄호가 포함되어 로케일 파서를 재작성(V-6a에서 발견). ② ratatui-image 기본 feature `chafa-dyn`이 시스템 libchafa를 요구해 `default-features = false, features = ["crossterm"]`로 사용(kitty/sixel/halfblocks는 내장). ③ Settings의 TTS 상태는 레이아웃 압박(MIN_HEIGHT 24)을 피하기 위해 Health 블록에 통합하고 블록명을 "상태 · Status"로 변경. ④ 앱 레벨 질문 없이 진행한 구현 선택: `TtsDisplay` 묶음 파라미터(clippy too_many_arguments), `SceneImageCache`(턴마다 재디코드 방지). ⑤ (G3 F-1) `openai` 프리셋이 `structured_output`을 기본 `auto`→`off`로 내린다 — OpenAI strict 모드가 게임 스키마의 동적 맵+`maxProperties`를 400으로 거부하기 때문. 명시 설정은 존중되며, `auto`에서 400이 오면 response_format 없이 1회 재시도 후 세션 비활성(신규 계약테스트). ⑥ (G3 메모) story-tts는 story-* 의존 0(계획의 "story-model 의존"보다 더 낮은 결합)이며, `from_query_stdio`는 쿼리 실패 시 Err 대신 halfblocks 폴백을 반환할 수 있어 미지원 터미널에서도 이미지가 텍스트 블록으로 표시될 수 있다. ⑦ (사용자 방향 수정 2026-10-15) `preset = "openai"` 브랜디드 프리셋은 제거하고 일반 URL 호환으로 재작업: `base_url`이 `/v1`으로 끝나면 아직 기본값인 요청 경로를 자동 정규화하는 `ProviderConfig::normalize_paths()`(호스티드 컨벤션 — OpenRouter/Groq/LM Studio/OpenAI 공통), `/health` 404 시 모델 목록으로 health 폴백. F-1 대응은 프리셋 선제 차단(F-1a) 대신 400 폴백 arm(F-1b, 유지) + 문서화가 담당한다. AC-1·AC-2는 이제 "URL 교체만으로 연결"로 재해석된다.

## 결함 로그 (G3 리뷰 → 처리)

| ID | 심각도 | 요지 | 처리 | 재검증 |
|---|---|---|---|---|
| F-1 | P1 | preset=openai 기본 경로에서 strict 스키마 400 → auto 폴백 미커버 | (a) 프리셋이 structured_output auto→off, (b) 400+schema+auto → disable+RF 없이 1회 재시도 arm 추가 | 신규 계약테스트 `auto_fallback_retries_without_response_format_on_http_400` PASS |
| F-2 | P3 | 매 낭독마다 블로킹 `say -v ?` 스폰 | `OnceLock` 프로세스 캐시 | story-tts 22 tests PASS |
| F-3 | P3 | 손상된 위치 이미지가 캐릭터 폴백을 영구 가림 | 후보별 decode 폴스루(`load_scene_image`) | `broken_location_image_falls_through_to_character_portrait` 등 PASS |
| F-4 | P3 | kitty transmit-once — 리사이즈 후 이미지 소실 가능 | 실기기 체크리스트로 이관(서버 기동 후 관찰 필요) | 미관찰(BLOCKED 항목에 병기) |
| F-5 | P3 | 이미지 26x14 셀 고정, 짧은 터미널에서 잘림 | 후속 개선(동적 재인코딩)으로 분리 | — |
| F-6 | P3 | TTS 호스트 동작 자동화 테스트 없음 | 후속 제안(mock provider e2e 회귀테스트) | — |
| F-7 | P3 | `[tts]`·images 컨벤션 문서 부재 | config/README.md에 두 섹션 추가 | 문서 존재 확인 |
| F-8 | P3 | (F-3 수정이 도입) Unchanged 경로에서 매 refresh 전체 재디코드 | `last_path`가 살아있으면 decode 없이 즉시 Unchanged인 고속 경로 + `unchanged_scene_does_not_redecode` 핀 테스트 | storytui 테스트 PASS |
| F-9 | P3 (수용) | 스키마 거부 서버 + `auto`에서 세션당 1회의 낭비 400 왕복(자가 치유, 문서화된 옵트아웃 있음) | 실기기 체크리스트에 관찰 항목 추가, 후속 티켓으로 트래킹 | 해당 없음(설계 수용) |
| F-10 | P3 (문서) | README `/health` 문구가 /health 없는 로컬 서버의 폴백 혜택을 누락, 계획 AC-3/AC-4가 제거된 프리셋 용어 참조 | README 문구 수정 + AC 본문 ⑦ 재범위로 갱신 | 문서 확인 |
| F-11 | P2 (증분) | 시작 시 활성 프로필이 픽커에 2행 표시(호스트 로스터 이중 계수 — 이전 치환이 fmt 재구조화에 매칭 실패해 잔존) | run()의 prepend 제거, `config.providers`만으로 목록 구성 | 코드 확인 + config 로스터 테스트(base 0번·3항목) |
| F-12 | P3 (증분) | apply_provider가 빌드 성공 전에 설정 상태 교체 | 로컬에 먼저 빌드 후 성공 시에만 상태 대입 | 코드 확인 |
| F-13 | P3 (증분) | 왕복 전환 시 프로필별 모델 선택 미보존(단일 슬롯 설계 귀결) | README에 "모델 선택은 프로필 간 공유되지 않는다" 명시 | 문서 확인 |
| F-14 | P3 (증분) | 신규 테스트 3건이 cwd 상대 `saves`를 읽어 분기를 시험하지 않거나 환경 의존 | 세 fixture 모두 save_dir를 tempdir로 고정 | storytui 16 테스트 PASS |
| F-15 | P1 (성능 증분) | 스택 라인 캐시 키를 별도 카운터와 비교해 절대 무효화되지 않음 — 첫 프레임 이후 내레이션 전사 동결 | 캐시 키를 `story_stack_version`과 직접 비교·저장, `stack_lines_cache_version` 삭제, `story_stack` pub(crate)화 + `replace_story_stack` 단일 쓰기 경로, 스택 변경 간 2프레임 회귀테스트(`stack_cache_invalidates_when_the_stack_changes`) 추가 | 신규 회귀테스트 PASS + 703/0 |
| F-16 | P2 (UX 증분) | 설정 화면에 이미지 상태·프로토콜 행이 실제로 렌더되지 않았음(replace 무음 실패 — 3번째) | `render_health_and_sampling`에 `image (i)` 행 추가(켜짐·백엔드/터미널 미지원/꺼짐) + 스냅샷 재생성 | 스냅샷에 `image (i) : 켜짐 · kitty` 렌더 확인 |
| F-17 | P3 (UX 증분) | 내비게이션 문서 미기재 5건(`n` 무확인 새게임·Alt+Home·[Home] 클릭·오류 q·입력 편집키) + `i` 가드 주석 | UI-NAVIGATION.md 보완, `n` 키는 위험 표기 | 문서 확인 |
| F-18 | P3 (UX 증분) | 이미지 끔 상태에서도 씬 변경마다 decode+encode(렌더만 게이트) | 후속 티켓: host가 disabled 시 refresh 건너뛰기 + 재활성 시 강제 refresh | — |

공유 파일(apps/storytui/src/config.rs, host.rs)은 T-2 → T-5 순차, story.rs는 T-7 단독. 병렬화 없음(단일 작업자).

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-provider` — wiremock 계약테스트: models_path, `max_completion_tokens` 직렬화, health `/models`, **⑦ `hosted_v1_base_url_adapts_paths_and_health_falls_back`**, 400 폴백 | 저장소 루트 | 신규+기존 테스트 전부 통과 | 계약 26 + lib 전부 ok (2026-10-15 재실행) | PASS |
| V-2 | `cargo test -p storytui` — 설정 단위테스트: 호환 URL, `[tts]`, 다중 프로바이더(파싱·중복 거부·persisted 선택·폴백), App 선택기 테스트 | 저장소 루트 | 통과 | config 16 + host 6 + bench 5 + story-tui picker 테스트 포함, 워크스페이스 701/0 (2026-10-15) | PASS |
| V-3 | `cargo test -p story-tts` — say 명령 구성, 로케일 파서(신형 출력 포함), openai 백엔드 wiremock(mp3 바이트·401), stage_audio 가드, speakable_text | 저장소 루트 | 통과 | 22 passed / 0 failed | PASS |
| V-4 | `cargo test -p story-tui` — TestBackend+halfblocks 체커보드 이미지 렌더(셀 색 검사), 이미지 None·협폭 폴백, Settings 스냅샷 갱신 | 저장소 루트 | 통과 | story 7 + snapshot 5 포함 전부 ok | PASS |
| V-5 | `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` (README.md:22-24) | 저장소 루트 | 전부 클린 | 691 passed / 0 failed, clippy 0 error, fmt clean (최종 상태) | PASS |
| V-6 | 실기기 확인: (a) `say -v ?` 한국어 보이스 + 파싱된 이름으로 `say -v "Eddy (한국어(한국))" -o file` 합성 77KB 성공(무음 검증), (b) 실서버 부트: LM Studio(localhost:1234) 기동 중 — 실제 바이너리 pty 부팅에서 `health=Ready` 로그 + curl 한국어 생성(finish=stop, 21초) 확인, omlx는 다운 → 전환 관찰만 대기 | macOS / LM Studio | 낭독·렌더·전환 관찰 | (a)(b) PASS, 대화형 관찰만 사용자 몫 | 잔여: 대화형 관찰 |
| V-7 | `cargo test -p story-tui --test render_perf -- --nocapture` — 120턴 스택 100x40 프레임 시간 측정(기선 6.89ms → 개선 3.61ms, -48%), 50ms 게이트 어설션 | 저장소 루트 | 수치 기록 + 게이트 통과 | 기선 413ms/60프레임 → 217ms/60프레임 (2026-10-15) | PASS |
| V-8 | 구조 검증: host.rs 틱 분기(로딩 1Hz 게이트 + drawn_loading_second), 마우스 분기(Down/Scroll만 재그리기), app.rs 스택 캐시(버전 무효화 + 스트리밍 부분 재조립) — 코드 리뷰 + 전체 스냅샷·렌더 테스트 회귀 없음 | 저장소 루트 | 게이트 로직 정확성 | 1차 리뷰에서 F-15(P1) 발견 → 수정 + 회귀테스트, 703/0 | PASS (리뷰어 재판정) |
| V-9 | `docs/UI-NAVIGATION.md`가 실제 전환 키·마우스·게임 중 변경 가능 항목을 코드와 대조해 전부 담는지 확인(리뷰어 코드 대조) | 저장소 루트 | 그래프=코드 일치 | 리뷰어 재판정 대기 | pending |
| V-10 | `cargo test -p story-tui --lib` — 이미지 토글(즉시 재활성·캐시 보존), 프로바이더 선택기 클릭, 스냅샷 갱신 | 저장소 루트 | 신규+기존 전부 통과 | story-tui lib 74 + 워크스페이스 705/0 | PASS |

**실기기 체크리스트 (서버 기동 후)**: ① `cargo run -p storytui` → `v` 토글·한국어 낭독 관찰. ② Ghostty에서 `stories/rainy-reunion/images/locations/cafe-namu.png` 넣고 kitty 렌더 관찰. ③ F-4: 터미널 리사이즈 후 이미지 생존 여부. ④ F-9: 스키마 거부 호환 서버에서 첫 턴의 400 폴백 지연(1회) 관찰(실 키 필요). ⑤ `o`→`e` 프로바이더 전환 → 상태·모델 목록 갱신, 재시작 후 선택 유지 관찰.

**성능 증분 기록 (2026-10-15, 사용자 추가 요구 — "TUI UI/UX 성능이 안 나온다")**: 진단 3원인 — ① 로딩 중 16ms 틱 무조건 풀 드로우(1Hz 라벨을 60fps로), ② 매 프레임 전체 대화 스택 마크다운 재조합, ③ 마우스 이동마다 재그리기. 측정 기선 `tests/render_perf.rs`(120턴 스택): 6.89ms/frame. 조치 — 로딩 1Hz 게이트(drawn_loading_second), 마우스 Down/Scroll만 재그리기, 스택 라인 캐시(키 = set_lists가 bump하는 story_stack_version, 스트리밍 엔트리만 실시간 조립, 쓰기는 replace_story_stack 단일 경로), render_story가 조립된 Text를 받도록 시그니처 변경. 결과 3.61ms/frame(-48%, 최종 3.98ms — 회귀테스트 추가 후 측정) + 로딩 60초 기준 드로우 CPU 24.8s→0.2s(~100x). 1차 리뷰가 캐시 무효화 결손(F-15, 첫 프레임 이후 전사 동결)을 잡아 수정 — 교훈: 단일 프레임 테스트만으로는 캐시 계약을 못 잡는다, 상태 변경 간 다중 프레임 테스트가 필수. 남은 3.61ms는 ratatui Paragraph의 전체 콘텐츠 랩 비용 — 창(windowing)은 후속 과제.

**UX 탐색 증분 기록 (2026-10-15, 사용자 추가 요구 — "그래프를 그려서 사용자가 어디로 가는지 모두 보자")**: 그래프 문서(docs/UI-NAVIGATION.md) 작성에서 드러난 갭 = 이미지 on/off 설정이 없었음 → 설정 `i` 토글(즉시 적용, 캐시 보존 재활성, user-settings 유지) + 상태 블록에 감지 프로토콜 표시(사용자가 "이미지가 제대로 보이나"를 스스로 확인 가능) + 프로바이더 클릭 테스트. 마우스 히트테스트는 last_render_area 기반이라 단위테스트에 명시적 영역 설정 필요(테스트가 계약을 문서화).

**증분 기록 (2026-10-15, 사용자 추가 요구)**: 복수 프로바이더 + TUI 전환(T-9~T-11, AC-11~13). 설계 결정: 로스터는 기본 `[provider]`를 0번으로 포함(전환 후 원래 프로필 복귀 가능 — 구현 중 발견한 결함 수정), 프로필별 `api_key_file`은 로드 시 전부 해석(전환 즉시성), env 키는 구성 시 지연 해석(기존 동작), 전환 시 스테일 모델 수리는 시작 로직과 동일한 규칙을 적용해 user-settings까지 저장. TUI에서 자유 입력으로 엔드포인트를 새로 만드는 것은 범위 밖(프로필은 TOML에 정의).

## 평가와 인계

- G3 코드 리뷰는 위임 평가자(Agent)로 원 요청·계획·diff·검증 기록을 전달해 독립 판정. 위임 불가 시 동일 루브릭 self-review 명시.
- V-6는 외부 환경(로컬 LLM 서버) 필요 — 부재 시 구현 완료와 별도로 BLOCKED 표기, 사용자 실행 안내 제공.
- 커밋은 사용자 요청 시 별도로.
