# 표정 스프라이트 — 감정 분류·인라인 삽입과 로컬 성능 대응

2026-10-22 · 요청: "다른 이런 앱들은 어떻게 이루어져 있는지 연구하고 … 특히 표현이나
이미지 넣는 방법들에 대해서 연구 … 로칼모델이라서 성능이 안좋을꺼라 그것들을 같이 대응"
(전체 SDD 사이클 2 — 1차는 생성 방식 토글, `2026-10-22-generation-mode-settings.md`)

## 연구 (SDD discovery)

**외부 유사 앱의 표현·이미지 삽입 방식**:

1. **SillyTavern Expressions** — 감정별 스프라이트 폴더 관례(28종 감정, 캐릭터별 디렉터리),
   채팅마다 소형 분류 모델이 최신 응답의 감정을 분류해 스프라이트 교체. VN 모드로 캐릭터
   옆 표시. 폴백 스프라이트 개념(expressions-plus).
2. **SillyTavern 이미지 생성 확장** — 메시지 단위 SD 생성(캐릭터 요약+대화를 프롬프트로
   재구성). 서버형 SD 백엔드 필요 → 로컬 TUI 단일 프로세스 범위 밖.
3. **KoboldCpp** — 내장 이미지 생성 탭, "현재 장면을 SDXL 프롬프트로 기술" 모드 버튼.
   역시 별도 생성 모델 필요.
4. **ratatui-image** — kitty/sixel/iterm2/halfblocks 통합, `sliced` 변형으로 부분 스크롤
   슬라이싱. StoryTui가 이미 채택(커밋 8e4ec11).

**공통 구조**: 유사 앱 모두 "감정/장면 판정"에 **추가 모델 호출**을 쓴다(분류기 모델 또는
이미지 프롬프트 재생성). 로컬 저사양 환경에서 이 비용은 체감 품질을 해친다.

**저장소 갭** (근거: `apps/storytui/src/host/images.rs`, `crates/story-tui/src/app.rs:1417`,
`crates/story-tui/src/screens/story.rs:43`):

- 팩 정적 이미지(`images/locations|characters/`)는 장소 경계에만 삽입됨 — 캐릭터
  **표정/감정 표현은 전무**.
- `story-model::Emotion`(10종)이 존재하지만 `StatePatch`에 감정 필드가 없어
  `CharacterState.current_emotion`은 항상 `Neutral` — 엔진 상태만으로는 스프라이트 원천 불가.
- 스프라이트 원천 후보: (a) `StatePatch.next_emotion` 추가 — 모델 출력 의존, 약한 로컬 모델은
  필드를 생략해 기능이 무용화 + 모델/골든/세이브 파장. (b) ST식 분류 모델 호출 — 턴마다
  추가 추론(저사양 정반대). (c) **키워드 룰 분류 — 추론 0회, 결정적, 기존 세이브/팩에 즉시
  작동**. → (c) 채택.

## 목표와 범위

- **달라지는 결과**:
  1. 팩이 `images/expressions/<캐릭터id>/<감정>.png`를 제공하면, 각 턴 내레이션에서
     추정한 감정의 스프라이트가 대화 흐름에 **감정이 바뀐 턴 앞에만** 인라인 삽입된다.
  2. 감정 추정은 **모델 호출 없이** 순수 키워드 분류로 이뤄진다(로컬 성능 대응의 핵심).
  3. 설정에서 `f`(한글 ㄹ)로 표정 스프라이트를 끄고, 선택이 `user-settings.toml`에 영속된다.
  4. 스프라이트 부재 시 캐릭터 초상으로 폴백하고, 직전 삽입과 같은 소스면 생략한다 —
     트랜스크립트 블록 수가 감정 횟수에 비례해 폭증하지 않는다.
- **제외**: 이미지 생성 모델 연동(별도 SD 백엔드 필요), `StatePatch` 감정 필드 추가,
  다중 캐릭터 동시 표정(관례상 primary 1명 — 기존 `location_image_candidates`와 동일).
- **호환**: 팩 스키마 변경 없음(폴더 관례만 추가), 세이브 포맷 변경 없음
  (`StoryStackEntry`는 런타임 재구성), `UserSettings`는 `Option<bool>` 하위호환.
  ADR-003 유지 — 팩 로더는 텍스트 전용, 파일 접근은 호스트 전담.

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | 표정 스프라이트가 감정이 바뀐 턴 앞에 인라인 삽입되고, 같은 감정 연속·초상 폴백 중복에서는 생략된다 | T-2, T-3 | V-1 |
| AC-2 | 감정 분류가 내레이션 키워드만으로 결정적으로 동작한다(추론 0회) — 분류 유닛 테스트로 입증 | T-1 | V-1 |
| AC-3 | 설정 `f` 토글로 표정 표시 on/off, `user-settings.toml` 영속, 설정 상태 블록 `표정 (f)` 행과 키맵 행 표시 | T-4, T-5 | V-1, V-2 |
| AC-4 | 스프라이트 해상은 세션당 감정당 1회 디코드 캐시이고, 부재·손상은 초상→무이미지로 우아한 폴백이다 | T-3 | V-2 |
| AC-5 | 기존 흐름(장면 이미지, i/g/v 토글, 복원·스왑)에 회귀가 없다 | T-6 | V-3 |

## 설계

- **감정 분류** (`crates/story-tui/src/app.rs`, `location_timeline`과 같은 표시 전용 순수
  함수 계열): `classify_emotion(narration: &str) -> Emotion` — 고정 순서 키워드 테이블
  `[(Emotion, &[&str])]`에 대해 포함 히트 수를 세어 최다 감정 선택, 타이브레이크는 테이블
  순서, 0 히트는 `Neutral`. `emotion_timeline(&[HistoryEntry]) -> Vec<Emotion>`로 턴별
  분류. 한국어 서사 어휘 중심, 영어 소량(lowercase 비교).
- **스프라이트 해상** (`apps/storytui/src/host/images.rs` — `LocationImageStore` 미러):
  후보 `images/expressions/<primary>/<emotion>.{png,jpg,jpeg,webp}` → 캐릭터 초상 폴백.
  `ExpressionImageStore`가 감정별 승자 경로 캐시(세션당 1회 디코드, authoritative
  absence). primary는 `engine.game.characters.keys().next()` 기존 관례.
- **삽입** (`App::story_blocks`): 장면 이미지와 **하나의 `previous_source`를 공유**하는
  dedupe — 초상 폴백 표정이 같은 초상의 장면 이미지와 겹치지 않게 한다. 표정 셀 예산은
  별도 상한(초상형 14×14)으로 `SceneImage::encode`에 예산 파라미터화.
- **설정 흐름**: `UserSettings.expressions_enabled: Option<bool>`(None=기본 켬) → 호스트가
  App 미러 반영 → `f` 토글 시 세션값+영속 동시 갱신(`i` 토글과 동일 패턴). 끄면 호스트는
  스프라이트 로드를 생략하고 app은 표정 블록을 생략한다.
- **UI 설계** (review-design 설계 모드): transcript는 시간순 서사 — 표정은 "턴 텍스트
  직전"의 감정 맥락 블록(장면 이미지와 동일 위치 규약, 스페이서 1행 재사용). 설정 상태
  블록은 토글 나열(tts/image/gen 다음 `표정 (f) : 켬/끔`) — 상태 블록 높이 8→9,
  80×24 최소 크기에서 `단축키 Min(3)`이 흡수(2+7+9+3+2=23≤24). 디자인 출처는 기존
  `image (i)`/`gen (g)` 행 — 새 패턴 도입 없음. 색상 없는 라벨 텍스트라 색각 의존 없음.
- **실패·복구 경계**: 손상 스프라이트는 디코드 실패 시 다음 후보(초상)로 폴백
  (`load_location_image`의 fall-through와 동일). 팩 전환 시 `reset_scene_images`가
  표정 스토어도 lockstep 클리어( stale-portrait 불변식 유지).

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 태스크 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | 감정 키워드 분류기 + 타임라인 fold, 유닛 테스트 (AC-2) | crates/story-tui/src/app.rs, lib.rs | 없음 | V-1 | done |
| T-2 | `StoryStackEntry.emotion` + `set_lists` fold + `story_blocks` 표정 삽입·공유 dedupe·예산 파라미터화 + 렌더 테스트 (AC-1) | crates/story-tui/src/app.rs, screens/story.rs | T-1 | V-1 | done |
| T-3 | `ExpressionImageStore`·후보/폴백/캐시 + 테스트 (AC-4) | apps/storytui/src/host/images.rs | 없음 | V-2 | done |
| T-4 | 호스트 연결 — refresh/reset/toggle 영속/초기 프로젝션 (AC-3,4,5) | apps/storytui/src/host.rs, apps/storytui/src/config.rs | T-2, T-3 | V-2 | done |
| T-5 | 설정 렌더 `표정 (f)` 행 + 키맵 행 + 스냅샷 재기록 (AC-3) | crates/story-tui/src/screens/{settings.rs,keymap.rs}, tests/snapshots | T-4 | V-1 | done |
| T-6 | 문서 — UI-NAVIGATION 이미지 규칙·변경 항목 표, KNOWN-LIMITATIONS 분류기 한계, 본 계획 | docs/ | T-5 | — | done |

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-tui` — 분류기 유닛, 타임라인 fold, story_blocks 삽입·dedupe(halfblocks TestBackend 렌더), f 키, 설정 스냅샷 2종 | repo root | 신규+기존 전부 통과 | lib 132/132 통과(신규 7: 분류 3·삽입/dedupe 3·f키 1 — F-2 수정 후 영어 큐 단언 2건 포함), settings 스냅샷 2종 재기록(INSTA_UPDATE, F-4 푸터 반영) 후 통과 — `표정 (f) : 켜짐/꺼짐` 양분기 렌더 확인, 80×24 레이아웃 유지 (2026-10-22, 결함 수정 후 재실행) | PASS |
| V-2 | `cargo test -p storytui` — ExpressionImageStore 경로/폴백/캐시, UserSettings 라운드트립, 콜드 토글-ON 갱신 | repo root | 전부 통과 | lib 19 + bin 29 + 통합 15 포함 전 스위트 ok(신규 스토어 테스트 5, 표정 토글 라운드트립 1, F-1 회귀 테스트 1) (2026-10-22, 결함 수정 후 재실행) | PASS |
| V-3 | `cargo test --workspace` + clippy(-D warnings) + fmt --check + check-core-deps + check-public-config | repo root | 회귀 0 | 63 스위트 전부 ok, clippy 경고 0, fmt 정렬, ADR-003·public config 양호 (2026-10-22, 결함 수정 후 재실행) | PASS |

## 리뷰 결함 처리 (G3/G4 — reviewer: independent, 판정 PASS)

독립 평가자가 AC-1~5 충족과 V-1~V-3 재현을 확인하고 F-1~F-5를 보고했다.

| ID | 심각도 | 내용 | 처리 |
|---|---|---|---|
| F-1 | P2 | 꺼짐으로 시작한 세션에서 `f` ON 시 다음 publish까지 스프라이트 미로딩 — "재활성 즉시" 주장 위배 | 수정: 토글 ON 분기에서 `refresh_scene_images` 즉시 호출 + 회귀 테스트 `enabling_expressions_mid_session_resolves_sprites_immediately`(bin 29번째). V-1~V-3 재실행 |
| F-2 | P3 | "영어 큐 소량" 주장에 코드가 반함(한국어뿐) | 수정: 9감정에 영어 큐 1개씩 추가(smiles/tears/snaps/restless/gasps/hugs/coldly/blushes/apolog) + 분류 테스트 영어 단언 2건 |
| F-3 | P3 | 80×24 설정 화면에서 단축키 블록 내용 행 3→2행(1..9·l 행 가려짐) — 레이아웃 산술이 내용 손실을 놓침 | 유지+문서화: 가려진 행은 설정 화면에서 기능하지 않는 스토리 키이고 전체 키맵은 `?` 오버레이에 존재. 이 절로 명시 |
| F-4 | P3 | 푸터 힌트에 g는 있고 f는 없음(선택적 노출) | 수정: 푸터에 `f 표정` 추가(g 도입 시 선례와 동일), 스냅샷 재기록 |
| F-5 | P3(관찰) | 오프닝 아님에도 view.narration 분류 중복 계산 | 조치 불필요 — HashSet dedup으로 무해(평가자 합의) |

## 평가와 인계

- **분류기 한계** (`KNOWN-LIMITATIONS.md` 명시): 키워드 룰은 문면 감정만 잡고 반어·문맥은
  못 읽는다 — ST의 분류 모델 대비 정밀도는 낮지만 추론 비용 0이고, 오분류의 최악은
  "스프라이트 미표시 또는 중립"이라 정보 파괴가 없다. 한국어 주도 + 영어 큐 소량.
- **F-3 잔여**: 설정 화면 80×24에서 단축키 블록이 2행만 보인다(1..9·l 행 생략) —
  스토리 전용 키이며 `?` 오버레이에 전체 키맵 존재. 후속 여지: 설정 관련 키만 필터 렌더.
- 대화형 실단말 확인(스프라이트 삽입 체감)은 터미널 앱 특성상 사용자 실행으로 남긴다 —
  halfblocks TestBackend 렌더 테스트가 삽입·위치·dedupe를 자동 검증(기존 컨벤션과 동일).
- 커밋은 사용자 요청 시에만.
