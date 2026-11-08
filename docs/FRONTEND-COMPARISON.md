# 스토리 프론트엔드 기능 비교 (2026-10-23)

SillyTavern, RisuAI, TavernAI, Agnai, Backyard AI, Chub, NovelAI(+AI Dungeon·KoboldAI 참고)의
사용자 대면 기능을 조사해 StoryTui 현재 상태와 대조한 결과물. 조사 방법: 웹 공식 문서·GitHub
기반 에이전트 조사 3건(SillyTavern 전수, 기타 프론트엔드, 로컬 저장소 재고 — 작업 트리
미커밋 포함 현재 기준) + 로컬 파일 대조 검증. 요청: "다른 프로젝트에서는 어떻게
이루어지는지 모두 확인하고 개선 사항들을 가져올 것."

## 1. 이미 구현돼 있는 것 (외부 후보였으나 갭이 아님)

| 기능 | 외부 기준점 | StoryTui 구현 |
|---|---|---|
| 로어북 고급 게이팅(확률·sticky/cooldown/delay·재귀·예산·보조키/논리) | ST World Info, Risu 로어북, NovelAI Lorebook | `crates/story-core/src/lorebook.rs` — T2 재귀 3패스, (turn,id) 결정 롤, priority→order admission 전부 구현. **UI 노출만 없음** |
| 자동 요약 장기 기억 | ST Summarize, Risu SupaMemory | 이벤트 기반 MemoryEntry + recent-arc digest(턴당 1줄 FIFO) + earlier_recap. 요약 출처 검증(`summary.rs`)은 외부에 없는 강점 |
| 표정 스프라이트 | ST Expressions | 키워드 분류기(무추가 호출) — `classify_emotion` |
| TTS 자동 낭독 | ST/Risu/Backyard | 백엔드 2종(say/openai), `v` 토글 |
| 역할별 모델 배정·프로필 | ST Connection Profiles | Director/Choice/Writer 3역할, 프로필 귀속·복원 |
| 캐릭터 카드 임포트 | ST/Risu/Chub 생태계 | PNG chara/ccv3·JSON·charx·CAI, V1/V2/V3, ST 채팅 시드(CLI) |
| undo/retry/새게임 가드, greeting·pack 피커 | AI Dungeon rewind, TavernAI swipe 원형 | `u`/`r`/확인 모달/오프닝 선택 |
| 샘플링 프리셋·페르소나 편집 | ST 샘플러 프리셋·페르소나 | `x` 스타일 사이클, `a` 페르소나 (2026-10-23 사이클) |

## 2. 실제 갭 (우선순위·크기 순)

| # | 갭 | 외부 근거 | 크기 | 비고 |
|---|---|---|---|---|
| G1 | **로어북 뷰어/편집기** — 엔진은 완성, UI는 임포트 리포트 행뿐 | ST/Risu/NovelAI 공통 핵심 저작 UI | 中 | 2026-10-23 사이클이 "별도 사이클"로 명시 유보 |
| G2 | **Author's Note(고정 조향 노트)** — 장면·방향·문체 지시를 매 턴 프롬프트에 주입 | ST Author's Note(@Depth), NovelAI, Backyard | 소 | 프롬프트 조립에 슬롯 부재(`prompt_context.rs` 확인). 저비용·고효과 |
| G3 | **Continue(이어쓰기)** — 끊긴 내레이션을 재생성과 다르게 자연 이어 생성 | ST Continue/Auto-continue, AI Dungeon | 中 | 2단계 Writer 경로 재사용 가능 |
| G4 | **분기 히스토리 트리·변형 보관** — undo/retry가 분기를 파기하지 않고 보관·전환 | NovelAI Editor V2 트리, TavernAI 2.x | 大 | 체크포인트 링(메모리 8턴)+세이브 확장 필요. 양 조사 모두 TOP 꼽음 |
| G5 | **Impersonate(대필 제안)** — AI가 플레이어 행동을 제안·작성해 입력에 주입 | ST Impersonate | 소 | 프롬프트 변형 + 입력 버퍼 주입 |
| G6 | **최종 프롬프트 미리보기** — 전송 프롬프트를 역할·출처와 함께 열람 | TavernAI Final Prompt Preview, ST Prompt Inspector | 소 | 조립기가 순수함수라 렌더만 추가 |
| G7 | **채팅/세이브 내보내기** — jsonl/txt 백업·타 앱 이식 | ST 채팅 파일 관리, Backyard Import/Export | 소 | 임포트는 있고 익스포트 없음 |
| G8 | **Regex 후처리(표시/프롬프트 분리)** | ST Regex 확장, Risu Regex Script | 中 | 사전 정의 몇 개로 시작 가능. 인지 부하 주의 |
| G9 | **Quick Replies식 커스텀 동작 키** | ST Quick Replies | 中 | TUI 키맵 문화와 궁합은 좋으나 키 예산 부담 |
| G10 | **그룹/멀티 캐릭터 회화** | ST 그룹 채팅, TavernAI 2.x MultiChat | 大 | 팩 단일 주인공 구조와 충돌 — 구조 결정 선행 |

## 3. 기각·비대상 (터미널 부적합 또는 프로젝트 비목표)

- Data Bank/RAG·벡터 검색(임베딩 인프라), 채팅 자동 번역(외부 API), 토큰 확률 뷰(logprobs
  백엔드+셀 렌더 부담), Live2D/VRM/VN 레이아웃·BGM, 테마 시스템(색상 수준만 가능),
  멀티유저/계정, 런타임 플러그인 로딩(Risu iframe 플러그인, ST 원클릭 확장 — 바이너리
  모델과 상충), STscript 전체(파서 비용 대비 수요 낮음), 클라우드 전용 모델 모듈(NovelAI
  Modules — 비 OpenAI 호환 비목표).
- 컨텍스트 예산 자동 산출은 2026-10-23 사이클에서 `/v1/models`로 신뢰 불가로 기각 유지.

## 4. 제안 로드맵 → 2026-10-23 사이클 결과

1. ~~이번 사이클(권장): G1+G2~~ → **확정 범위 "모두"로 G1–G9 전부 구현 완료**
   (계획·증거: docs/plans/2026-10-23-frontend-parity.md). G4는 최소 완성판(되돌리기 링
   영속 — 로드 직후 `u` 가능, 분기는 undo→재선택→저장), G8/G9는 키 없는 TOML 구성.
2. 후속 과제로 남음: G4 변형 순환 UI(스와이프식), G10 그룹 채팅(팩 스키마·엔진 턴 주체
   재설계 선행). 추가로 조사 중 발견된 기존 엔진 결함 후보(로어북 활성 원장이 accept을
   살아남지 못함 — sticky/cooldown 무효화 의심)는 plan 문서 가정 로그에 기록.

## 5. 조사 출처

- SillyTavern: https://docs.sillytavern.app/usage/ 및 하위(worldinfo, authors-note, personas,
  groupchats, chatfilemanagement, macros, advancedformatting, data-bank, prompt-manager,
  st-script, characters, user-settings, api-connections, extensions/*), GitHub README
- RisuAI: https://github.com/kwaroran/RisuAI , plugins.md, wiki/Lorebook, wiki/SupaMemory,
  서드파티 소스 분석(2026-08 기준 — 세부 수치는 버전별 불일치 가능)
- TavernAI: https://github.com/TavernAI/TavernAI
- Agnai: https://github.com/agnaistic/agnai
- Backyard AI: https://backyard.ai/docs
- Chub: https://docs.chub.ai/docs/advanced-setups/lorebooks
- NovelAI: https://docs.novelai.net/en/text/editor , /en/text/lorebook/
