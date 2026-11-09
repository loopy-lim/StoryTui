# StoryTui 기능 갭 분석 — SillyTavern · RisuAI 대비

작성일: 2026-11-07 · 갱신: 2026-11-09 · 기준: StoryTui `main`(+ History 편집·삭제) · SillyTavern 1.19.0 · RisuAI 2026-10-294
관점: TUI 사용자 우선. "없는 것"을 나열하는 게 아니라 **진짜 결핍**(P0–P2)과 **문서화된 의도적 제외**를 구분한다.

## 요약

- 비교 37개 항목: 예전 P0 채팅 조작 축(스트리밍·편집·분기·되돌리기·수출)은 2026-10~11에 충족/부분 충족으로 올라갔다. 남는 갭은 **커스터마이징 노출**(토큰 가시화·샘플링 TUI 등 P1)과 의도적 제외에 집중.
- StoryTui는 캐릭터챗 앱의 하위 호환이 아니라 **다른 장르**(선택지 기반 인터랙티브 픽션 게임 엔진)다. 웹 고유 기능(CSS·스프라이트·허브·동기화)은 N/A.
- 로어북 트리거 엔진·카드 임포트·커넥션 프로필·세이브는 대등 이상.

## P0 — 플레이 경험을 가르는 결핍 (TUI 관점)

| # | 기능 | 현황 | 근거 |
|---|---|---|---|
| 1 | 스트리밍 렌더링 | **충족** (MVP SSE 경로, 이슈 #2) | `docs/plans/2026-11-08-turn-streaming.md` |
| 2 | 턴 편집·삭제 | **충족** (History 내레이션 편집·접미 삭제, 이슈 #3) | `EditHistoryNarration` / `DeleteHistoryFrom`, `docs/KNOWN-LIMITATIONS.md` |
| 3 | 분기·스와이프 | **부분** (undo 시드 변형 `[`/`]` 순환 · 세션 메모리) | `CycleTurnVariant`, 풀 브랜치 트리는 의도적 후속 |
| 4 | 턴 되돌리기 | **충족** (`u` + 세이브 영속 링, 이슈 #4) | `RestoreCheckpoint`, `SavePayload.undo_stack` |
| 5 | 대화 내보내기 | **충족** (`storytui export` txt/jsonl, 이슈 #6) | `story-persistence/export.rs` |

## P1 — 노출만 부족한 것들

| 기능 | 현황 | 비고 |
|---|---|---|
| 토큰·컨텍스트 가시화 | 없음 | 6티어 복합 컨텍스트라 ST Prompt Itemization보다 가치가 큼. 상태줄 + Memory 화면 확장 |
| 샘플링·예산·타임아웃 TUI 편집 | TOML만, TUI는 `max_tokens` 표시뿐 | `KNOWN-LIMITATIONS.md:99` 자체 인정 |
| 작가 노트(플레이어 스티어링) | 없음 | 엔진의 동적 스티어링 티어 위에 사용자 노트만 있으면 됨. 전면 프롬프트 편집과 달리 엔진 권위와 양립 |
| 페르소나 TUI 편집·전환 | TOML 1개 고정, 편집 불가 | ST는 다중+캐릭터 매핑 |
| Director/Writer 토글 | 코드 있음, 설정 UI 노출 없음 | `KNOWN-LIMITATIONS.md:65` |

## P2 — 가치 있으나 범위 큼

그룹챗(엔진 재설계 수준) · 카드 편집기(원본 보존+import-report가 부분 대체) · 로어북 편집 UI(팩 파일 관행과 충돌 적음) · 이미지 생성(PRD Later 명시) · Continue · 메시지 단위 컨텍스트 제외 · @Depth/역할 주입(before/after char만 있음).

## 의도적 제외 — 유지 권장

클라우드 전용 프로바이더(non-goal 문서화) · 벡터 RAG 우선 도입(`CONTEXT-MANAGEMENT.md`의 결정론 우선 결정) · 플러그인·STscript·Lua/Python 생태계(Story Pack이 확장 단위) · 동기화·멀티유저·임페르소네이트(플레이어 주권 원칙 상충) · 표정 스프라이트·STT(매체 부적합).

## StoryTui가 앞서는 지점

1. **엔진 권위 + 규칙 검증** — 존재하지 않는 캐릭터·플래그 거부(FR-013), 선택지 자동 품질 게이트. ST/Risu는 프롬프트만 믿는 구조.
2. **세이브 신뢰성** — 체크섬·마이그레이션·원자적 쓰기·100회 라운드트립. ST/Risu 채팅 파일에 없는 계층.
3. **프리픽스 안정성** — turn-invariant 캐논 프리픽스로 로컬 추론 캐시 극대화. ST는 벡터화 캐시 미스를 스스로 경고.
4. **임포트 투명성** — 카드 원본 바이트 보존 + 미매핑 필드 리포트 + ST 채팅 시딩. 양쪽에 없는 고유 기능.
5. **터미널 미디어** — kitty/sixel 씬 이미지 인라인, macOS 무료 TTS(say).

## 전체 매트릭스

### A. 대화·턴 조작

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| 스트리밍 렌더링 | 충족(MVP SSE) | 기본+FPS 조절 | 기본 | 충족 |
| 턴 편집·삭제 | 충족(History 내레이션·접미 삭제) | 편집·삭제·이동·제외 | 편집+표시/전송 분리 | 충족(이동·제외는 N/A) |
| 분기·스와이프 | 부분(변형 순환, 세션 메모리) | Swipe Picker+Branch | 브랜치 트리 GUI | 부분 |
| 턴 되돌리기 | 충족(`u`+영속 링) | 체크포인트 | 브랜치 복귀 | 충족 |
| Continue | 없음 | Continue Nudge | 지원 | P2 |
| 임페르소네이트 | 없음 | 표준 | Auto Suggest | N/A(플레이어 주권 상충) |
| 메시지 단위 제외 | 없음 | Included/Excluded | editRequest/editDisplay | P2 |

### B. 프롬프트·파라미터·가시성

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| 시스템 프롬프트/프리셋 UI | 없음(엔진 소유) | Prompt Manager | 프롬프트 순서 편집 | 의도적 + P1(작가 노트 최소형) |
| Author's Note류 스티어링 | 없음 | Author's Note | Global Note | **P1** |
| 샘플링 파라미터 편집 | 부분(TOML만) | 전 파라미터+프리셋 | 전 파라미터+Bias·seed | **P1** |
| 컨텍스트 예산 조절 | 부분(TOML) | Context% | per-book 버짓 | **P1** |
| 토큰 카운터·가시화 | 없음 | Prompt Itemization | 토크나이저+Playground | **P1** |
| Director/Writer 토글 | 부분(UI 노출 없음) | — | — | **P1** |
| 런타임 매크로 | 없음(임포트 시간 한정) | Macros 2.0 | CBS 100여 개 | 의도적 P3 |

### C. 캐릭터·페르소나

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| 카드 임포트 포맷 | 있음(V1/V2/V3·PNG/JSON·charx·c.ai) | 대등 | 대등(V3 주도) | 충족 |
| 임포트 투명성(원본 보존+리포트) | 있음 | — | — | 강점(고유) |
| 카드 편집기 | 없음 | 전 필드 | 기본+고급 | P2 |
| 그룹챗 | 없음(group_only_greetings만 기록) | 턴 오더 4종 | 지원 | P2 |
| 페르소나 관리 | 부분(TOML 1개) | 다중+매핑+락 | 지원 | **P1** |

### D. 로어북/World Info

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| 트리거 엔진 | 있음(키·selective·재귀 T2·예산·확률) | 대등(+Timed Effects) | 대등(+@@ 데코레이터) | 충족(미지원 필드는 리포트 고지) |
| 편집 UI | 없음(파일+사이드카) | 전용 편집기 | 전용 편집기 | P2 |
| @Depth·역할 주입 | 부분(before/after char만) | @Depth+role+outlet | @@depth/@@role | P2 |

### E. 컨텍스트·메모리

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| 장기 기억 | 있음(결정론 6티어) | Summarize+Vector RAG | HypaMemory V1–V3 등 5종 | 충족(임베딩은 의도적 보류) |
| 프리픽스 안정성 | 있음(캐논 프리픽스 설계) | 경고 사항(캐시 미스) | — | 강점 |

### F. 프로바이더

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| 백엔드 다양성 | 부분(OpenAI 호환 단일) | 30+ | 20+ + WebGPU | 의도적(non-goal) |
| 커넥션 프로필 | 있음(TOML 복수+TUI 전환) | Connection Profile | 프리셋+fallback | 충족 |
| 역할별 모델 분리 | 있음(메인/기획/선택지/본문) | 보조 모델 | auxiliary | 강점(더 세분) |

### G. 미디어

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| TTS | 있음(say+OpenAI) | 17종 | 9종+카드 임베드 | 충족 |
| STT | 없음 | Whisper·Vosk·API | 음성 입력 | P3 |
| 이미지 생성 | 없음(표시 전용) | SD/ComfyUI/DALL-E | SD/Gemini/인라이 | 의도적 Later P2 |
| 씬 이미지 인라인 | 있음(kitty/sixel) | — | — | 강점(TUI 고유) |
| 표정 스프라이트 | 없음 | 감정 자동 전환 | Emotion Images | N/A |

### H. 데이터·세션

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| 세이브 시스템 | 있음(오토+3슬롯+체크섬+마이그레이션) | 채팅 파일 | 채팅+콜드 스토리지 | 강점 |
| 대화 내보내기 | 충족(`storytui export`) | JSONL/TXT | JSON/TXT/HTML | 충족 |
| 대화 임포트 | 있음(ST 채팅→imported-chat) | 6개 앱 마이그레이션 | 상호 변환 | 충족 |
| 동기화·멀티유저 | 없음 | 멀티유저 | Drive/Kei/P2P | N/A |

### I. 확장·자동화

| 기능 | StoryTui | SillyTavern 1.19 | RisuAI | 판정 |
|---|---|---|---|---|
| 플러그인·스크립팅 | 없음 | 확장 3계층+STscript | Lua/Python+모듈 | 의도적 |
| 정규식·퀵리플라이 | 없음 | Regex+QR | Regex Script | 의도적 P3 |

## 조사 정정 · 주의

- **risuai.org는 존재하지 않는 도메인**(NXDOMAIN). 실제는 `risuai.net`, 문서는 `kwaroran.github.io/docs`.
- **"RiS 컴파일러(Automerge, Recursive compiler)"라는 명칭은 공식 문서·코드에 없다.** 실제 대응물은 로어북 `@@recursive` 계열 데코레이터 + CBS 파서 + Lua/Python 트리거. 이후 논의에서 명칭 재정의 필요.
- StoryTui의 여러 미구현은 `KNOWN-LIMITATIONS.md`·PRD 9.2에 이미 자체 기록돼 있다. 2026-11-09 갱신 기준, 문서화되지 않았던 P0 결핍(스트리밍·편집·분기·되돌리기·수출)은 해소됐고, 남는 노출 갭은 토큰 가시화·샘플링 TUI 세밀 편집 등 P1이다.

## 근거

- StoryTui: `main @ e748044` — `docs/{KNOWN-LIMITATIONS,CONTEXT-MANAGEMENT,UI-NAVIGATION}.md`, `docs/StoryTUI_{PRD,Backlog}_KO.md`, `crates/story-{import,core,provider,tts,tui}` 코드.
- SillyTavern: docs.sillytavern.app (core-concepts, prompts, worldinfo, personas, groupchats, data-bank, st-script) · 1.19.0 릴리즈노트.
- RisuAI: github.com/kwaroran/Risuai · kwaroran.github.io/docs · github.com/kwaroran/character-card-spec-v3 · realm.risuai.net.
