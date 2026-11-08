# 즉시 실행·원스텝 자동화와 약한 모델 자동 방어

2026-10-23 · 요청: "현재 중요한 것으로, 결국 사용자가 바로 즐기거나, 한번에 가능하도록
하는것이 주요된 기능인데 그것을 잘 극복하고, 부족한 ai성능을 극복할 수 있는지도 모두
연구해줘." — 이 문서는 연구(SDD discovery)+계획(G1) 산출물이다. 구현은 아직 시작하지
않았다(태스크 상태 전부 계획). 미커밋 변경(2026-10-23 패리티·자동튜닝 사이클) 위에서
보존하며 진행한다.

## 연구 (SDD discovery — 읽기 전용 조사 2건 + 진입점 직접 검증)

### 축 A: 바로 즐기기·원스텝의 현재 상태

**이미 자동인 것** — 첫 실행 설정 무수정(엔드포인트 `127.0.0.1:8080/:1234/:11434` 자동
프로브·비챗 모델 필터·모델만 영속, `host/settings.rs:230-328`), 저장 모델 소실 시 자동
수리(`settings.rs:212-224`), 역할 모델 자동 수리(`host.rs:156-171`), autosave 자동·기동
시 검증 복원(`engine.rs:119-121`, `host.rs:223`), 서버 만다도 홈 메뉴는 열림(`host.rs:290-299`).
서버만 떠 있으면 첫 내레이션까지 키 1개(`n`)다.

**남은 마찰 6가지**:

1. 기동 시 홈 메뉴 무조건 1단계(`host.rs:301`) — autosave 복원은 준비돼 있으나(`host.rs:223`,
   `host.rs:908-922`가 실제 적용) 재개는 항상 키. 자동 재개·`auto_resume` 류 설정 필드 없음.
2. **되돌아온 유저의 홈 기본 커서가 "새 게임"행(0행)** — Enter 한 번이 확인 모달 없이 새
   게임을 시작한다(`app.rs:3059-3071`). 진행 중인 세션이 있는 유저에게 1키 풋건.
3. 서버 다운 재프로브가 "설정 화면을 열 때"만 발동(`host.rs:776-792` → `settings.rs:355-400`)
   — 복구 감지에 2키. 주기적 백그라운드 재시도 없음.
4. 생성 실패 후 재시도 전부 수동 `r`(`event.rs:34-41`) — 엔진은 `retry_pending`에 원 요청
   보관(`engine.rs:103-104, 977`). UI 자동 재시도 없음.
5. 팩 기억 없음 — 기본 팩 하드코딩 `stories/rainy-reunion`(`config.rs:810-813`), 피커 수동.
   팩 0개는 TUI 밖 하드 종료(`config.rs:814-819`).
6. 문서 오류 — INSTALL.md가 7 멤버를 "six crates"로 기술·`rust-toolchain.toml` 버전 고정
   주장(실제 `channel = "stable"`)·홈 메뉴 흐름과 모순된 기동 서술, import/export 서브커맨드
   미기술. UI-NAVIGATION에 옛 설정 화면 `v` 잔재.

### 축 B: 약한 모델 보완의 현재 상태

**엔진·프로바이더가 이미 막는 것(16층)** — 선택지 역할 배정·risk 정규화·13종 품질 검사
(`choice_quality.rs`), 상태 패치 8단계 검증(`validator.rs:70-102`), 이벤트·엔딩 결정론 재판정
(`reducer.rs:361-380`), Director 플랜 검증+드라이런(`director.rs:65-103`), Writer 무권한화
(`writer.rs:57-118`), 요약 출처 검증(`summary.rs:39-71`), JSON 수복 파이프라인(`recovery.rs:66-193`
— 언래핑·think 제거·locate·별칭 정규화·1회 수리 패스), 계약 재시도는 온도 0.2로 낮춰 1회
(`openai_compatible.rs:885-920`), 5xx 백오프 재시도(`openai_compatible.rs:393-434`),
내레이션 반복·정체 5종 게이트+1회 narration-only 자동 수리(`narration_quality.rs:87-128`,
`engine.rs:927-937, 992-1051`), 로어북 게이팅·활성 원장(`lorebook.rs`).

**자동 방어가 없는 빈틈(우선순위 순)**:

- **P1 — Writer 경로 빈/공백 내레이션 커밋 가능**: 단일 패스는 recovery가 빈 narration을
  거부(`recovery.rs:176-180`)하지만 2단계 Writer는 trim/빈 검사 전무(`writer.rs:57-64`) —
  패치에 진전만 있으면 빈 산문도 커밋. `NarrationQualityIssue`에 길이 항목 자체가 없음
  (`narration_quality.rs:19-33`).
- **P2 — max_tokens 잘림 무감지**: `finish_reason`은 파싱되지만 로그 전용
  (`envelope.rs:190-196, 243-247` — 직접 검증). "완결 JSON이지만 산문이 문장 중간에 끝남"은
  무감지. Writer 스트리밍은 최종 청크의 finish_reason조차 추출하지 않음
  (`parse_stream_delta`, `openai_compatible.rs:1199-1207`). provider엔 이미
  `length_repair_request`(짧은 산문 확장 재요청)가 있으나 `min_narration_chars` 기본 0이라
  발동 조건이 사실상 없음(`openai_compatible.rs:828-859`, `config.rs:96-98`).
- **P4 — Continue/장면 생성의 빈 결과 무음 묵살**: `engine.rs:1485-1489` — 사용자 피드백 0,
  재시도 0.
- **P6 — Director 파싱 재시도 온도 불변**(`openai_compatible.rs:1048-1051`): generate_turn
  계열은 0.2로 낮추는데 Director만 동일 바디 반복 — 비대칭.
- 기각·후순위(가정 로그): 주권·화자·언어 lint(P5 — 오탐 리스크 큼), penalty/seed 전송
  (P7 — 로컬 서버 호환 미확인), 계층 인지 예산 절단(P8 — 정상 플레이는 한도 하회 전제),
  예산 자동 산출(`/v1/models` 신뢰 불가 — `FRONTEND-COMPARISON.md:44` 기각 유지).

### 축 C: 방어 기제의 체감 비용 — 검증 (2026-10-23 사용자 피드백 "자꾸 막아서 느려지고 흐름이 끊긴다")

지적은 사실로 확인됐다. 두 경로가 있다.

1. **연결 차단(실측 로그)** — `saves/storytui.log` 최근 5회 실행(2026-10-22)이 전부
   `provider unavailable: connect` 실패. 재프로브는 설정을 열 때만 발동하므로 서버가
   돌아와도 앱이 모른다 → 플레이 진입 자체가 수동 대기로 끊긴다. AC-3·AC-4가 이 경로의
   해법이다.
2. **방어 재호출 스택(코드 + 과거 실측)** — `reduce_accept`에서 내레이션 게이트 발동 시
   수리 재호출(`engine.rs:927-937`)이 기본 호출과 겹치고, 게이트 통과 후 선택지 갱신이
   또 발동될 수 있다(`engine.rs:939-952`). 주 생성 모델 기준 worst-case 3배 호출(기본+
   수리+잘림 이어쓰기, 각각 계약 재시도 ±1). **동일 문제의 실측 전례**: 과거 1200자
   길이 하한이 "거의 매턴 전체 재생성을 유발(~2x 레이턴시)"해 `min_narration_chars = 0`
   으로 꺼둔 기록이 `config/storytui.toml:48-54`에 있다. → 길이 기반 게이트(MinLen)는
   폐기하고, 턴당 방어 재호출 예산(AC-8)으로 대체한다.
3. **흐름 차단 표면(오버레이)** — 서버 오류·계약 실패(`invalid_generation`)·수리 실패
   (`narration_repair_failed`)·패치 실패가 전부 수동 `r`/`Esc` 대기 오버레이다. 반면
   선택지 갱신은 커밋 후 즉시 뷰를 발행하는 비차단 구조(`engine.rs:1119-1135` — 장면·
   autosave가 갱신 요청과 함께 먼저 나감)로 확인됐다. 자동 재시도(AC-3)로 오버레이
   진입을 줄이고, 수리 실패 오버레이는 빈 내레이션 한정(희귀)으로만 남긴다.
4. 실측 한계: 현재 로그에는 성공한 생성이 없어(전부 connect 실패) 턴당 호출 수 실측은
   불가했다. provider는 호출별 토큰 사용량을 로깅하므로(`log_token_usage`) 구현 후
   사용자 실세션 로그 분석으로 방어 발동 빈도를 검증할 수 있다.

### 결론

둘 다 극복 가능하다. 단 방향이 하나 추가된다 — **방어는 흐름을 막아서는 안 된다**:
연결 복구는 자동으로, 방어 재호출은 턴당 1회로, 방어 실패의 흐름 차단은 최소화한다. 필요한 프리미티브(`retry_pending` 슬롯·`RegenerateCurrentTurn` 명령,
ContinueNarration의 narration append 구조, `finish_reason` 파싱, 복원 바디, 재프로브 훅,
수리 인프라)이 모두 존재하며 남은 것은 배선이다. 단, **도움말 예산이 포화**(게임 키 25행 =
20행×2, `2026-10-23-parity-followup.md` G4)하므로 새 키 추가는 불가 — 본 계획은 **키 추가
0, 자동화로만** 해결한다(사용성 피드백 메모리와도 일치).

## 목표와 범위

- **달라지는 결과**:
  1. autosave가 있는 채 앱을 시작하면 홈을 거치지 않고 바로 이전 세션 스토리 화면이
     뜬다. 상태줄에 "이전 세션을 이어요" 한 줄을 표시한다. `user-settings.toml`의
     `auto_resume = false`(기본 true)로 끄면 기존처럼 홈이 먼저 뜬다.
  2. autosave가 있으면 홈 메뉴 기본 커서가 "이어하기"행이다(auto_resume 꺼짐 포함).
  3. 생성 실패 시 오버레이 전에 1회 자동 재시도(짧은 지연 후, 상태 표시 동반)한다.
     자동 재시도도 실패하면 기존 [R]/[Esc] 오버레이가 뜬다. 사용자 취소(c)에는
     재시도하지 않는다.
  4. 모델 서버가 꺼져 있으면 앱이 주기적으로(설정 열기 없이) 스스로 재탐색해 서버가
     돌아오면 상태줄이 연결됨으로 바뀐다.
  5. 생성이 max_tokens로 잘려 산문이 문장 중간에 끝났다면 앱이 감지해 이어쓰기를 한 번에
     수행한다(단일 패스는 기존 length repair 자동 연결, Writer는 finish_reason 전파 +
     자동 ContinueNarration 1회 — 턴당 캡 1, 배지 표시).
  6. 빈 내레이션은 기존 narration-only 수리 경로로 1회 자동 수리된다(단일·Writer 양
     경로 공통 — 짧음 게이트는 없음, 재호출 유발 방지). Continue/장면 생성의 빈 결과는
     무음 묵살되지 않고 1회 재시도 뒤 안내가 보인다. Director 재시도도 계약 재시도와
     같이 온도를 0.2로 낮춘다.
  7. 표현층 회귀 없음 — 새 키 0개(도움말·키맵 예산 무증가), 80×24 스냅숏 의도된 변화만.
  8. 한 턴에서 주 모델의 방어 재호출(수리·이어쓰기)은 최대 1회다 — 약한 모델에서도 한
     턴이 2배 이상 느려지지 않는다(선택지 갱신은 경량 모델·비차단이라 예산 밖).
- **제외 (가정 로그)**: 주권·화자·언어 lint(P5), penalty/seed 전송(P7), 계층 인지 예산
  절단(P8), 예산 자동 산출(기각 유지), 팩 0개 하드 종료 완화·마지막 팩 기억(별도 사이클),
  INSTALL.md 이외 문서 대개정. 오버레이 상태에서의 재자동 재시도(플레이어 주권 — [R] 유지).

## 수락 기준

| ID | 관찰할 결과와 적용 조건 | 담당 태스크 | 검증 ID |
|---|---|---|---|
| AC-1 | autosave가 있는 상태로 앱을 시작하면 홈 메뉴 없이 이전 세션 스토리 화면이 뜨고 상태줄에 이어하기 안내가 보인다. `auto_resume = false`면 홈이 먼저 뜬다 | T-1, T-2 | V-1, V-3 |
| AC-2 | autosave가 있을 때 홈 메뉴 기본 커서는 "이어하기"행이고, 새 게임은 여전히 명시 선택이다 | T-2 | V-2 |
| AC-3 | 생성 실패 시 자동 1회 재시도가 상태 표시와 함께 일어나고, 그것도 실패해야 [R]/[Esc] 오버레이가 뜬다. `c` 취소에는 자동 재시도하지 않는다 | T-2 | V-3 |
| AC-4 | 서버 꺼짐 상태에서 서버가 돌아오면 설정을 열지 않아도 상태줄이 연결됨으로 갱신된다 | T-2 | V-3 |
| AC-5 | max_tokens 잘림(finish_reason "length")이 감지되면 이어쓰기가 키 0회로 1회 자동 수행되고 배지가 남는다. 턴당 1회를 넘지 않는다 | T-3, T-4, T-2 | V-1 |
| AC-6 | 빈 내레이션(Writer 경로 포함)은 1회 자동 수리로 방어되고, Continue/장면의 빈 결과는 무음 묵살되지 않고 1회 재시도 뒤 안내가 보인다. Director 재시도는 온도 0.2로 다양화된다. 길이 기반 게이트는 넣지 않는다(재호출 유발 — `config/storytui.toml:48-54` 전례) | T-3, T-4 | V-1 |
| AC-7 | 표현층 회귀 없음 — 새 키 0개, 도움말·키맵·80×24 스냅숏 의도된 변화만, 게이트 5종 통과 | T-1–T-5 | V-2, V-4 |
| AC-8 | 커밋된 턴 기준으로 주 생성 모델의 방어 재호출(내레이션 수리·잘림 이어쓰기)은 합계 최대 1회다 — 수리가 이미 발동한 턴의 잘림은 자동 이어쓰기 대신 배지+수동 `w` 안내로 마친다. 자동 재시도는 실패 시도마다 별도 1회 캡이고(가정 로그 — 낭비 1회 수용), 커밋 시 예산이 리필된다. 예외: 빈 continuation의 1회 재디스패치(AC-6)는 호스트 캡 1회로 별도 제한된다. 선택지 갱신(경량 모델·비차단)은 기존대로 예산 밖이다 | T-4 | V-1 |

## 태스크

| ID | 목적·AC | 수정 파일/경계 | 선행 | 검증 | 상태 |
|---|---|---|---|---|---|
| T-1 | `UserSettings`에 `auto_resume`(기본 true) 추가, 파싱·라운드트립·기본값 테스트 (AC-1) | apps/storytui/src/config.rs | 없음 | V-1 | done (위임) — `auto_resume` 필드·수동 Default(신규 설치 기본 true 보장), 라운드트립·부재→true·잘못된 타입 에러 테스트 |
| T-3 | provider: 단일 패스 — 파싱 성공+finish_reason "length"면 `length_repair_request` 자동 연결(1회). Writer — `parse_stream_delta`가 최종 finish_reason을 추출해 `WriterFinish`로 전파. Director 파싱 재시도 온도 0.2 (AC-5, 6) | crates/story-provider/src/{openai_compatible.rs, envelope.rs, lib.rs 타입} | 없음 | V-1 | done (위임) — length 수리 1회 캡·재귀 없음, 스트림 finish_reason 전파(`WriterEvent::Done`/`WriterFinish`), Director 재시도 온도 0.2. 편차: 실제 타입은 story-model/director.rs(envelope.rs 불요), story-core 테스트 리터럴 5행 기계 수정 |
| T-4 | engine: `narration_quality`에 Empty 이슈 추가(양 경로 수리 인프라 재사용 — MinLen 없음), 잘림 플래그·빈 사이드 결과 플래그를 뷰/상태로 노출, 턴당 방어 재호출 예산(수리·이어쓰기 합계 1, AC-8) 강제 (AC-5, 6, 8) | crates/story-core/src/{narration_quality.rs, engine.rs}, crates/story-model/src/view.rs | T-3 | V-1 | done (위임) — `TruncationStatus{None,Auto,ManualOnly}`+`empty_side_result` 뷰 노출(세션 메모리), Empty 이슈·Writer 1회 수리→오버레이, 정규 턴·되감기에서 해제, Continue append 시 Auto→ManualOnly 강등(엔진 측 루프 방지) |
| T-2 | 호스트·앱: 기동 시 auto_resume 복원 적용+홈 스킵·상태줄, 홈 커서 기본(이어하기), Generation Err 자동 1회 재시도(취소 제외·짧은 지연·턴당 1), frame_tick 주기 재프로브(미가용 시), 잘림·빈 사이드의 자동 Continue 재디스패치+배지/토스트, `App::handle_key`·초기화 레벨 테스트 (AC-1, 2, 3, 4, 5) | apps/storytui/src/{host.rs, host/settings.rs 재프로브 추출}, crates/story-tui/src/app.rs | T-1, T-4 | V-2, V-3 | done (위임) — `TurnAutomation` 턴당 예산(재시도 1회·1.5s 지연·취소 제외), 기동 자동재개·홈 커서 기본, `post_accept_automation`(잘림 Auto 1회 디스패치·ManualOnly 수동 안내·사이드 빈 1회 재디스패치), `reprobe_availability` 추출+15s 주기 재프로브, story_notice 표면(키 0개). 편차: lib.rs host 표면화·wiremock dev-dep, choice-refresh 진행 중 자동 이어쓰기 지연 |
| T-5 | 문서: INSTALL.md 정정(크레이트 7·toolchain·기동 흐름·import/export), UI-NAVIGATION 자동 재개·상태줄·v 잔재 제거, KNOWN-LIMITATIONS 갱신(자동 재시도·이어쓰기 캡·남는 한계) (AC-7) | docs/ | T-2 | V-4 | done — INSTALL.md 정정(7 크레이트·toolchain stable·기동 흐름·import/export·자동 재개/재시도/재프로브), UI-NAVIGATION(자동 재개 엣지·자동 보조 단락·요약 표 3행·옛 설정 `v` 잔재 제거·Alt+Home 실동작 정정), KNOWN-LIMITATIONS 자동 보조 한계 섹션 신설 |

의존성 요약: T-1·T-3 병행 → T-4 → T-2 → T-5. 겹치는 파일(app.rs·engine.rs)은 순차.

## 설계 메모

- **자동 재개는 "복원 바디의 초기화 이동"**: `HomeRequest::Continue` 바디(`host.rs:908-922`)를
  기동 복원(`host.rs:223`) 뒤 조건 실행으로 재사용 — 새 경로를 만들지 않는다. 조건:
  `can_continue && settings.auto_resume`. 홈은 Alt+Home(기존 `app.rs:2247`)과 Esc 경로로
  계속 도달. 신규 유저(autosave 없음)는 홈 그대로 — 학습 경로 보존.
- **홈 커서 기본**: `can_continue`면 이어하기 행 — 풋건(마침 2) 제거. auto_resume와 무관하게
  적용(홈을 연 사람에게는 선택지 정렬이 올은 것).
- **자동 재시도 규칙**: 턴당 1회, 사용자 취소 제외, ~1.5s 지연 후 `RegenerateCurrentTurn`
  재디스패치(명령·retry_pending 슬롯 기존 사용). 서버 다운 낭비 호출 1회는 수용 — 대신
  주기 재프로브가 복구를 알려주고 오버레이의 [R]은 플레이어 주권으로 남긴다.
- **주기 재프로브**: `frame_tick` 분기(`host.rs:479-498`)에 미가용(unconfigured/unavailable)
  시 15s 간격 재프로브 — `settings.rs:355-400` 로직을 추출해 공유. 상태줄 갱신만, 생성
  중에는 동작하지 않음.
- **잘림 이어쓰기**: 감지원은 finish_reason("length")뿐 — 종결 어미 휴리스틱은 오탐 리스크로
  미채택. 단일 패스는 provider 내부에서 기존 `length_repair_request`로 소화(엔진 무변경).
  Writer는 커밋 후 엔진이 잘림 플래그를 세우면 호스트가 `ContinueNarration`을 1회 자동
  디스패치 — continue는 선택지·상태 불변 append(`engine.rs:882-887`)라 턴 주권 훼손 없음.
  배지 "✎ 이어씀" 1행(본문 꼬리 subtle 관례). 턴당 캡 1 — 비용 폭주 방지.
- **길이 게이트 없음 / 방어 예산**: 짧은 산문을 재생성으로 고치는 길이 기반 게이트는
  재도입하지 않는다 — 1200자 플로어가 "거의 매턴 재생성(~2x 레이턴시)"을 유발한 실측
  전례(`config/storytui.toml:48-54`). 대신 턴당 방어 재호출 예산: 내레이션 수리와 잘림
  자동 이어쓰기의 합계 ≤ 1회(주 모델). 수리 발동 턴의 잘림은 배지+수동 `w`로 마친다.
  선택지 갱신은 경량 모델·커밋 후 비차단(`engine.rs:1119-1135`)이라 예산 밖.
- **빈 사이드 결과**: 엔진이 빈 accept를 무음 스킵하는 대신 플래그를 남기고(`engine.rs:1485-1489`
  변경), 호스트가 같은 사이드 명령을 1회 재디스패치 — 재시도 후에도 비면 기존 토스트 표면으로
  안내. 상태·턴 불변은 유지. 수리 실패 오버레이(`narration_repair_failed`)는 빈 내레이션
  한정으로 남긴다(빈 장면 커밋이 오버레이보다 나쁨 — 희귀 경로).
- **표현층 원칙**: 키 0개 — 푸터·키맵·도움말 무증가(포화 예산 회피). 안내는 기존 상태줄·
  배지·토스트 표면만 재사용, 라벨 최단.

## 검증 계획과 결과

| ID | 방법·정의 근거 | cwd·환경 | 기대 결과 | 실제 결과·증거 | 판정 |
|---|---|---|---|---|---|
| V-1 | `cargo test -p story-core -p story-provider` — MinLen/Empty 게이트(양 경로), 잘림 플래그·이어쓰기 캡, 사이드 빈 결과 플래그, finish_reason 전파, length repair 자동 연결, Director 재시도 온도, 기존 회귀 | repo root | 신규+기존 전부 통과 | story-provider 131 passed+doctest 1(신규 7 — length 수리 정확히 1회·stop 미발생·스트림 전파 2·Director 온도 2·stop 온도 유지) + story-core·story-model 413 passed(신규 10 — Empty 수리→오버레이, 잘림 Auto/ManualOnly·커밋 해제, 사이드 빈 플래그, Continue 강등), clippy·fmt·check-core-deps OK (2026-10-23) | PASS |
| V-2 | `cargo test -p story-tui` — `App::handle_key`·초기 상태 레벨: 홈 커서 기본, auto_resume 스킵 진입, 배지 렌더, 80×24 스냅숏·도움말 예산 무증가 | repo root | 신규+기존 전부 통과 | story-tui lib 178 passed — `handle_key` 레벨 5건(자동저장 시 홈 커서=이어하기·Enter→Continue, 재개 없으면 새 게임 기본, 재개 상실 시 커서 강등, story_notice 80×24 잘림 없음 5종 문구, 변형 배지 공존·새 턴 해제). 스냅숏 무변화(notice 기본 None), 키맵·도움말 행 수 불변 — 예산 테스트 통과 (2026-10-23) | PASS |
| V-3 | `cargo test -p storytui` — 호스트 배선: auto_resume 초기화, Err 자동 재시도 1회·취소 제외, frame_tick 재프로브, 사이드 재디스패치, Mock E2E | repo root | 신규+기존 전부 통과 | host_automation 11건 신규 통과 — 실패→자동 1회 재시도→성공(HTTP 2회), 2회째 실패→[R]/[Esc] 오버레이, 취소 미재시도, 재프로브 15s 게이트·복구 시 홈 안내 제거·다운 시 정적, auto_resume 설정×autosave 조합, 잘림 Auto 턴당 1회 디스패치·ManualOnly 미디스패치, 사이드 빈 1회 재디스패치·2회째 없음. storytui 전체 105 passed — lib 74·main 2·bench 5·diagnostic 6·host_automation 14(F-1 수리로 2단계 3건 추가)·mock_e2e 4 (2026-10-23) | PASS |
| V-4 | 게이트 5종 — `cargo test --workspace` + `clippy -D warnings` + `fmt --check` + `./scripts/check-core-deps.sh` + `./scripts/check-public-config.sh` | repo root | 회귀 0 | 최종 코드 상태(T-2 완료 시점) 기준 전부 통과 — `cargo test --workspace` 실패 0(story-tui 178·host_automation 11·provider 132·core+model 413 포함), clippy -D warnings 0, fmt --check OK, check-core-deps OK, check-public-config OK. 이후 변경은 docs 4파일뿐 — cargo 게이트 대상 외라 증거 유지 (2026-10-23) | PASS |

잔여(사용자 실행): 실서버 잘림·재시도 A/B(실모델 품질은 릴리스 게이트 G3 영역), 실단말
육안 확인.

## 가정 로그

- **방향 수정(2026-10-23 사용자 피드백)**: "성능이 약한데 자꾸 막아서 더 느려지고 흐름이
  끊긴다" → 검증 결과 실제(축 C)로 판명. 원칙 추가: **연결 복구는 자동, 방어 재호출은
  턴당 1회, 흐름 차단 최소화.** MinLen 게이트는 이 피드백으로 폐기(1200자 플로어 전례와
  동일한 재발 리스크). 잘림 이어쓰기도 수리와 예산을 공유하게 조정.
- **auto_resume 기본값 = true**: 요청이 "바로 즐기기"를 주 기능으로 명시. 대안(옵트인)은
  신규 유저 학습 경로는 그대로고 되돌아온 유저만 0키가 되는 점에서 기각 — 홈 도달 경로
  (Alt+Home 등)가 남아 있어 되돌리기 쉬움.
- **자동 재시도 1회·이어쓰기 캡 1**: 무한 자동화는 토큰 비용 주권을 해친다. 2회째 실패부터는
  사람이 [R]로 결정.
- **감지원 = finish_reason 한정**: 종결 어미·길이 휴리스틱 단독 감지는 한국어 오탐 리스크 —
  provider가 이미 주는 신호만 쓴다. (`min_narration_chars` 기본값 변경은 하지 않고 엔진
  MinLen 게이트로 동일 목적을 달성 — `config/storytui.toml:50-58`의 기각 사유 존중.)
- **P5(주권 lint)·P7(penalty 전송)·P8(계층 절단) 제외**: 오탐·서버 호환·후순위 — 각각
  독립 사이클 후보.
- **키 0개 원칙**: 도움말 예산 포화(게임 키 25/40행) + 사용성 피드백 — 자동화는 설정 필드
  (TOML)로만 제어.

## 평가와 인계

- **G1 계획 평가**: PASS (self, plan 모드 — 조사는 위임 2건+진입점 직접 검증, 계획 평가는
  self). AC↔T↔V 전결합, 실패 상태 명시. 축 C(방어 비용 검증)로 방향 수정 1회 —
  사용자 피드백("자꾸 막아서 흐름이 끊긴다")을 원칙으로 편입.
- **G2 구현·통합**: PASS (위임 경계별 T-1+T-3 / T-4 / T-2 순차, 통합은 host_automation으로
  경계 간 연결 검증 — 개별 PASS를 통합 PASS로 대체하지 않음). 경계 외 최소 변경 기록:
  story-model/director.rs 타입 전파(T-3 — WriterFinish가 story-model 소속), story-core
  테스트 리터럴 5행 기계 수정(T-3), lib.rs host 표면화+wiremock dev-dep(T-2 — 호스트
  통합 테스트 요건), openai_compatible.rs 중복 `#[test]` 1행 제거(T-3 오타).
- **G3 코드 리뷰**: PASS (독립 위임 리뷰어 — 게이트 5종 자체 재실행 포함, 이전 사이클
  미커밋분과의 혼합 diff 한계 명시). 결함 처리:
  - F-1(P2, 수정 완료): 2단계 모드 Writer/Director 전송 실패가 자동 재시도 없이
    오버레이로 갔음 → 단일 패스와 같은 무장 패턴을 두 분기에 적용(host.rs), 같은
    TurnAutomation 예산 공유(스테이지 합계 턴당 1회). 2단계 테스트 3건 신설(재시도 성공
    커밋·2회째 오버레이·취소 미무장) — host_automation 11→14건.
  - F-4(P3, 수정 완료): V 표 수치 집계 기준 정정(131+doctest, storytui 105 내역).
  - F-5(P3, 수정 완료): KNOWN-LIMITATIONS에 스토리 화면 서버 상태 표면 부재·배지/플래그
    세션 메모리 보충.
  - F-6(P3, 수정 완료): AC-8 문언에 빈 continuation 재디스패치 1회 예외 명시(호스트 캡).
  - F-2(P3, 후속 백로그): 무장 창(1.5s) 내 수동 `r` 시 도래한 자동 재시도가 재생성을
    해체 — 희귀·generation_id 가드로 이중 커밋 없음. 발사 시 id 비교 스킵이 후속 조치.
  - F-3(P3, 후속 백로그): `Alt+Home` 스크롤 바인딩이 bare Home 개방 분기(app.rs:2181,
    modifier 무검사)에 가려진 기존 결함 — UI-NAVIGATION에 실동작 기재 완료, 후속
    사용성 사이클에서 조건 추가 또는 사익 삭제 택일.
  - F-7(P3, 수정 완료): G3 재평가에서 지적 — AC-8 개정 문언이 "자동 재시도"를 합산에
    넣어 구현(재시도는 실패 시도마다 별도 캡·커밋 시 리필)과 불일치. 합산에서 재시도를
    빼고 별도 캡임을 명시하는 1행 문언 정정으로 종결(동작 변경 없음). G3 최종 PASS
    (독립 재판정 — F-1 해소·F-4/F-5 반영 확인, storytui 105·story-tui 196 재실행 일치).
- **G4 사용성·디자인**: PASS (self, 구현 모드 — 렌더 테스트 근거). 새 키 0개(푸터·키맵·
  도움말 무증가, 예산 테스트 통과), story_notice 5종 80×24 잘림 없음·변형 배지 공존·
  새 턴 자동 해제, 홈 커서 기본 변경은 handle_key 레벨 3건으로 증명, 스냅숏 무변화
  (notice 기본 None). 실단말 육안 확인은 사용자 실행(잔여).
- **G5 완료 판정**: PASS (self, completion 모드) — AC-1 자동재개(설정×autosave 조합),
  AC-2 홈 커서, AC-3 자동 재시도(단일·2단계·취소·2회째 오버레이), AC-4 주기 재프로브,
  AC-5 잘림 이어쓰기(provider 수리 1회·Writer 전파·턴당 1 디스패치), AC-6 빈 내레이션
  수리·사이드 재시도·Director 온도, AC-7 표현층 무증가+게이트 5종, AC-8 예산(엔진
  강등+호스트 캡+예외 명시). 최종 코드 상태: workspace 테스트 실패 0·clippy 0·fmt OK·
  정책 스크립트 2종 OK(독립 리뷰어 재실행 포함, 2026-10-23).
- **잔여 한계**: (1) 실모델 잘림·재시도 A/B와 실단말 육안 — 사용자 실행(릴리스 게이트
  G3 영역), (2) 방어 발동 빈도 실측 — provider가 호출별 토큰 사용량을 로깅하므로
  실세션 `saves/storytui.log` 분석으로 가능, (3) F-2·F-3 후속 백로그, (4) 팩 0개 하드
  종료·마지막 팩 기억·주권 lint 등은 가정 로그의 별도 사이클 후보.
- 커밋은 사용자 요청 시에만 — 이전 사이클(패리티·자동튜닝)분과 이번 사이클분의 분리
  커밋 가능.
