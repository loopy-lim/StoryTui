# RisuAI .charx + Character.AI 내보내기 임포트 계획

2026-10-18 · 요청: "다른 것들(Character.AI, Risu)에서 가지고 오지 못해서 어떻게 하면 가지고 올 수 있을까?"

## 목표와 범위

- **달라지는 결과**: RisuAI `.charx` 파일과 Character.AI 커뮤니티 도구가 내보내는 JSON을
  `storytui import`(CLI)와 TUI 홈 메뉴 "카드 가져오기"로 임포트할 수 있게 한다.
- **제외 범위**:
  - c.ai 서버에서 앱이 직접 내려받기(로그인 토큰·약관·안티봇 문제) — 파일 기반만 지원.
    c.ai는 공식 캐릭터 내보내기가 없고, 커뮤니티 확장(CAI Tools 등)이 만든 JSON/PNG 파일을
    받는 것이 생태계 표준 경로다. 조사 근거: `irsat000/CAI-Tools` content.js의
    `cai_character_hybrid`(표준 V1 필드 + pygmalion 별칭 동시 포함)와
    `cai_character_card`(PNG tEXt)는 **현재 파서가 이미 수용**한다. 남는 갭은 raw 덤프 별칭.
  - RisuAI 채팅 이력 임포트 — 캐릭터 임포트가 본 요구. RisuAI의 캐릭터 내보내기 형식은
    charx/PNG/JSON뿐임(`kwaroran/Risuai` `src/ts/characterCards.ts:594`)을 확인.
  - RisuAI `.jpg`(charxJpeg 확장자 위장) — 희귀 변형, 미지원(명시적 오류 유지).
- **현재 구조(확인한 근거)**:
  - `crates/story-import/src/card.rs:62-72` — 확장자 라우팅이 `png`/`json`만 허용.
  - `parse_card_json`은 `data` 래퍼만 보고(permissive `#[serde(default)]`), 별칭 필드는 무시.
  - TUI는 경로를 그대로 넘김(`apps/storytui/src/host.rs:689`), 확장자 검사는 `load_card` 한 곳.
  - CHARX 스펙: zip 컨테이너 + 루트 `card.json`(CCv2/v3) + `embeded://` 자산 URI
    (`kwaroran/character-card-spec-v3` SPEC_V3.md 36-59행).
  - c.ai raw 덤프 필드(CAI Tools `character_copy` 페이로드): `greeting`, `definition`,
    `title`, `description`, `name` (+ `char_greeting`/`char_persona`/`world_scenario`/
    `example_dialogue`는 pygmalion 하이브리드). `{"character": {...}}` API 래퍼 변형 존재.
- **기준 상태**: main `1d5704f`, 트래킹 안 되는 파일만 존재(.scratch/, stories/mira-smoke/).
- **도구·환경**: cargo(rust 1.88 MSRV, rust-toolchain.toml), CI 없음 — 검증은 cargo test/
  clippy/fmt. 평가자는 위임 에이전트(일반 목적) 사용 가능.
- **미해결 결정**: 없음 — 별칭 매핑은 "표준 필드가 비어 있을 때만 채우고 리포트에 기록"
  정책으로 설계 확정(기존 카드와 충돌 없음, compile형 "무손실 원본 보존" 정책과 일치).

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | RisuAI `.charx`를 CLI/TUI 임포트하면 팩이 생성되고 PackLoader 검증 통과; 이름·설명·오프닝·로어북이 팩에 반영 | T-1 | V-1, V-4 |
| AC-2 | `.charx`의 `embeded://` 아바타가 zip 엔트리에서 복원되어 `images/characters/`에 저장 | T-1 | V-1 |
| AC-3 | Character.AI raw 덤프 JSON(`greeting`/`definition`/`title`/pygmalion 별칭, `{"character":{}}` 래퍼)이 greeting→오프닝, definition→dialogue_examples 등으로 매핑되고 별칭 사용이 import-report.md에 기록됨 | T-2 | V-1 |
| AC-4 | 기존 V1/V2/V3 PNG/JSON 임포트 회귀 없음(기존 테스트 전부 통과) | T-4 | V-2 |
| AC-5 | zip이 아니거나 card.json이 없는 .charx, 기타 미지원 확장자는 한국어 안내 오류 | T-1 | V-1 |
| AC-6 | CLI usage와 TUI 임포트 모달 안내가 새 지원 형식을 반영 | T-3 | V-3 |

## 설계

- **charx 파서** — 새 모듈 `crates/story-import/src/charx.rs`:
  - `parse_charx(bytes) -> Result<ParsedCard, String>`. zip central directory를 읽어
    `card.json`을 찾고(`zip` 크레이트 9.x, `default-features=false,
    features=["deflate-flate2-zlib-rs"]` — 스펙상 "SHOULD NOT be encrypted"이므로
    crypto/bzip2/zstd 스택 제외), 기존 `card::parse_card_json`에 위임해 정책·정규화 재사용.
  - `embeded://<path>` 자산: `card_data.assets` 중 avatar/icon 종류에서 경로를 뽑아
    zip 엔트리와 대조(정확 일치 → 선행 `./`,`/` 정규화 → 대소문자 무시 폴백). 성공 시
    `canonical.avatar` 채우고 `extract_asset_avatar`가 남긴 "외부 URI 미지원" 노트를 제거.
    실패 시 노트 유지. 아바타 외 자산 개수를 근사 노트로 기록.
  - 원본 보존: `sources/card.json`에는 card.json 원본(`parsed.raw`)이 그대로 감 — 기존
    정책 그대로. 다른 zip 엔트리는 미지원 명시(리포트).
  - 오류: zip 아님/손상 → "CHARX zip을 열지 못했습니다…", card.json 부재 → 안내문,
    암호화 → 지원 안내.
- **c.ai 별칭 매핑** — `card.rs::parse_card_json` 안, `card_data_view` 파싱 직전:
  1. 루트에 `data`/`name`/`spec`이 없고 `character`가 객체(내부에 name)면 언랩.
  2. 대상이 비어 있고 원본이 비어 있지 않을 때만 채움: `greeting`/`char_greeting`→
     `first_mes`, `definition`/`example_dialogue`→`mes_example`, `char_persona`→
     `description`, `world_scenario`→`scenario`, `title`→`personality`.
  3. 적용된 별칭을 parse-time notes로 기록 → 리포트 근사 섹션에 "(c.ai 덤프)" 표기.
  - 원본 `raw`는 별칭 적용 전 값을 유지하지 않는다 — **별칭 적용은 view 복제에만** 하고
    `raw`는 파일 원본 그대로 직렬화(무손실 보존 정책 유지).
- **UI 텍스트**: TUI 모달 안내 문구와 CLI usage에 `.charx`·c.ai 덤프 반영. 상태 전이·복구
  흐름은 기존 임포트 다이얼로그(입력→Running→Report/에러) 그대로 — 정보 구조 변화 없음.
- **위험**: zip 의존성 추가(빌드 시간·바이너리 크기) — 최소 feature로 완화. 별칭 오탐은
  "빈 표준 필드만 채움 + 리포트 기록"으로 차단.

## 태스크

| ID | 목적·AC | 수정할 파일/경계 | 선행 태스크 | 검증 ID | 상태 |
|---|---|---|---|---|---|
| T-1 | charx 파서 + 라우팅 (AC-1,2,5) | Cargo.toml(워크스페이스/story-import), crates/story-import/src/{charx.rs(신규),card.rs,lib.rs} | 없음 | V-1, V-4 | done |
| T-2 | c.ai 별칭 매핑 + 리포트 노트 (AC-3) | crates/story-import/src/card.rs | 없음 (T-1과 같은 파일 — T-1 후 순차) | V-1 | done |
| T-3 | CLI/TUI 텍스트·모듈 문서 (AC-6) | apps/storytui/src/import_cmd.rs, crates/story-tui/src/screens/import.rs (+ app.rs paste 스니핑 확인) | 없음 | V-3 | done |
| T-4 | 통합 검증·회귀 (AC-1..6) | 없음(검증만) | T-1,2,3 | V-2, V-4 | done |

G1 평가(위임, validate-plan/plan 모드): **PASS** — 저장소 사실·외부 포맷 인용 전부 원문 대조 확인,
P3 3건(인용 정밀도·기준 리비전 표기·기존 텍스트 관찰)은 조치 불필요.

G3 평가(위임, review-changes): **PASS** — 독립 재실행(테스트·clippy·실제 CLI 재현·외부 스펙 원문 대조)으로
AC-1..6 확인. P3 결함 5건 보고 → 전부 수정·재검증:
- F-1 별칭 패스가 루트에서 동작해 표준 V2/V3 카드에 허위 리포트 노트 → 카드 데이터 뷰(`data` 내부)에
  적용하도록 수정. 재관찰: 루트에 무관한 `title`이 있는 V3 카드 임포트 시 c.ai 노트 0건.
- F-2 암호화 charx가 raw zip 오류 노출 → password 오류를 전용 한국어 안내로 매핑.
  재관찰: `zip -P` 암호화 charx → "암호화된 CHARX는 지원하지 않습니다…".
- F-3 read_entry가 첫 읽기 불가 엔트리에서 스캔 중단 → per-entry continue로 수정.
- F-4 embeded:// 아바타가 data-URI 아바타를 무조건 대체 + `name:"main"` 무시 → data-URI 우선 유지,
  `main` 이름 자산 우선 선택. 단위 테스트 2건 추가(단위 31→34).
- F-5 zip 엔트리 무제한 읽기 → 압축 해제 상한 64MiB 도입(read_zip_text/read_entry).
- F-6 (코드 결함 아님, 게이트 관찰): 작업 트리에 타 워크스트림(팩 피커·프로바이더 자동설정) 변경이
  혼재 — 임포트 파일과 분리 커밋할 것.

수정 후 재검증(2026-10-18): `cargo test -p story-import` 단위 34 + 통합 11 전부 통과, clippy 경고 0,
`cargo fmt -p story-import` 잔여 diff 0. F-1/F-2는 실제 CLI 재관찰로 확인(위 기재).

## 실파일 검증 (2026-10-18, 사용자 요청)

RisuRealm(`realm.risuai.net`)의 실제 캐릭터로 검증 — "서울에서 부산까지!"(by kuihak,
`/character/f7fe93bd-b2cb-4458-9804-cb76235cf468`, 1.4MB `application/charx`):
- 다운로드: `realm.risuai.net/api/v1/download/dynamic/<id>?cors=true` + `x-risu-api-version: 4` 헤더
  (RisuAI 소스 `characterCards.ts:1880` 근거).
- 실파일 구조: card.json(chara_card_v3) + `module.risum`(Risu 모듈) + `x_meta/*.json` + `assets/`(webp 아이콘·배경, woff2 폰트).
- 임포트 결과: PackLoader 검증 통과, 한국어 오프닝·대체 인사 2개·로어북 23개(빈 엔트리 3개는
  사유와 함께 건너뜀 기록), embeded:// 아바타 복원 — zip 엔트리와 `cmp` 바이트 일치 확인.
- **신규 관찰 → 수정**: zip 수준의 컨테이너 파일(`module.risum`, `x_meta/*` 등 17개)이 리포트에
  기록되지 않음(모듈 문서의 "기타는 집계·보고" 미충족) → `note_container_files` 추가:
  기타 파일 개수+대표명(module.risum 우선)을 근사 노트로 기록. 단위 테스트 1건 추가(단위 34→35),
  전체 테스트·clippy·fmt 재통과, 실파일 재임포트로 노트 확인.
- 플레이 가능 산출물: `stories/seoul-busan` (`storytui --pack stories/seoul-busan`).

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-import` — 신규 charx/별칭 단위·e2e 테스트(synthetic zip 작성 → import_card → PackLoader 검증) | repo root | 전부 통과 | 최종: 단위 34 + 통합 11 전부 통과 (2026-10-18; G3 F-4 테스트 추가 후 재실행 — G3 섹션 참조) | PASS |
| V-2 | `cargo test --workspace` | repo root | 기존 테스트 회귀 없음 | 임포트 범위 회귀 없음. 관찰 당시 `mock_e2e::malformed_draft_surfaces_error_and_keeps_state_intact` 1건이 **HEAD(7e7da3b) 기준선에서도 동일 실패**(변경 stash 후 재실행, 3회 재현) → 기존 결함, 본 변경과 무관. 이후 타 워크스트림이 해당 테스트를 정리해 최종 트리에서는 mock_e2e 4건 전부 통과(G5 독립 재확인) | PASS |
| V-3 | `cargo clippy -p story-import -p storytui` + `cargo fmt`, TUI 모달 렌더 테스트 | repo root | 경고 없음/포맷 정렬 | clippy 경고 0. `cargo fmt -p story-import` 적용 후 story-import 잔여 diff 0. story-core 쪽 fmt 드리프트는 타 커밋 유래(범위 밖). story-tui 테스트 84개 전부 통과(갱신된 모달 문구 포함) | PASS |
| V-4 | 실제 관찰: synthetic `.charx`를 `cargo run -- import`로 임포트 → 산출물과 import-report.md 확인 | repo root, `/tmp/seraphine.charx`(zip CLI 생성, 디렉터리 엔트리 포함) | 팩 생성 + 리포트에 charx 노트 | "가져오기 완료", 로더 검증 통과, `images/characters/seraphine.png`가 embeded:// 엔트리 복원(바이트 일치), 로어북 1개, 리포트에 "CHARX embeded:// 자산에서 아바타를 복원" + "아바타 외 1개 미지원" 노트. 파손 charx → "CHARX zip을 열지 못했습니다 …EOCD" 한국어 오류. c.ai wrapped 덤프 → opening.md "You made it.", characters.toml "Sera: Glad to see you", 리포트에 c.ai 노트 | PASS |

## 평가와 인계

- G1 계획 평가: 위임 평가자(validate-plan/plan 모드). G3 코드 리뷰: 위임 평가자.
  G5 완료: 위임 평가자(completion 모드). G4: UI는 텍스트 변경만 — 렌더 테스트 증거로
  self-review 병기.
- 커밋은 사용자가 요청할 때만(commit 스킬).
