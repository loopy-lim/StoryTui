# StoryTUI 아키텍처 결정 기록(ADR)

문서 버전: 1.0-draft  
작성 기준일: 2026-07-31  
상태 값: Proposed, Accepted, Superseded, Deprecated

## ADR-001 - Rust와 Ratatui를 제품의 첫 실행 기반으로 선택

- 상태: Accepted
- 결정일: 2026-07-31

### 맥락

첫 제품은 이미지 없는 터미널 애플리케이션이며, 장기적으로 규칙·저장·이벤트·UI 비동기성을 안정적으로 관리해야 한다. 로컬 LLM 추론 자체는 별도 Python 서버가 담당하므로 클라이언트는 추론 프레임워크보다 도메인 안전성과 터미널 경험에 집중한다.

### 결정

코어와 앱은 Rust로 작성하고 첫 렌더러는 Ratatui + Crossterm을 사용한다. Tokio를 비동기 runtime으로 사용한다.

### 이유

- Rust의 타입과 소유권은 `GameState`·`StatePatch`·세이브 migration처럼 잘못된 부분 변경이 치명적인 영역에 유리하다.
- 단일 native binary는 로컬 도구의 배포와 시작이 단순하다.
- Ratatui는 모델-뷰 방식의 터미널 렌더링과 테스트 가능한 buffer를 제공한다.
- Crossterm은 macOS 터미널 이벤트·raw mode·alternate screen을 다룰 수 있다.
- Tokio와 channel을 사용하면 모델 생성 중에도 TUI loop를 계속 실행할 수 있다.

### 검토한 대안

1. **Python Textual**: MLX와 같은 언어라 빠르게 만들 수 있으나, 추론 서버와 앱 수명·의존성이 결합되고 장기 코어 API의 정적 경계가 약해진다.
2. **TypeScript/Ink**: 개발 속도는 좋지만 터미널 폭·raw mode와 배포 runtime 의존성을 추가한다.
3. **Swift TUI**: Apple 플랫폼 통합은 좋으나 TUI 생태계와 향후 다른 렌더러에서 얻는 이점이 작다.
4. **처음부터 GUI**: 한국어 생성·선택·상태·기억의 핵심 위험을 검증하기 전에 범위를 넓힌다.

### 결과

긍정:

- 코어 불변식을 compile-time 타입과 테스트로 강화한다.
- UI를 버리더라도 core 투자가 남는다.
- release binary가 가볍고 원격 텔레메트리 없이 운용 가능하다.

부정:

- 한글 IME와 다중 줄 편집은 GUI보다 어렵고 실제 터미널 검증이 필요하다.
- Python 모델 서버와 프로세스·문서 경계가 생긴다.
- Rust 학습·컴파일 비용이 초기 prototype 속도를 늦출 수 있다.

### 검증

M0에서 Terminal.app·iTerm2 IME, panic 복원, 생성 중 반응성을 확인한다. 실패하면 코어 언어는 유지하되 입력 UX에 `$EDITOR` fallback을 검토한다.

## ADR-002 - 모델 서버를 Rust 프로세스에서 분리

- 상태: Accepted
- 결정일: 2026-07-31

### 맥락

초기 모델은 Apple Silicon용 MLX 형식이며 Python MLX 생태계가 가장 직접적인 실행 경로다. Rust 프로세스에 Python 또는 MLX binding을 내장하면 빌드·메모리·crash·업데이트 경계가 흐려진다.

### 결정

모델은 별도 로컬 Python 프로세스에서 실행하고 Rust는 `http://127.0.0.1:8080/v1/chat/completions` 같은 OpenAI-compatible HTTP 계약으로 호출한다.

### 이유

- 모델 다운로드·로딩·양자화와 앱 release를 독립적으로 변경할 수 있다.
- 모델 server crash가 TUI 상태를 직접 파괴하지 않는다.
- llama.cpp, Ollama와 다른 OpenAI-compatible 서버를 같은 client adapter로 연결할 수 있다.
- 서버를 command line에서 독립적으로 smoke·benchmark할 수 있다.

### 검토한 대안

1. **Rust에서 MLX 직접 binding**: 프로세스 간 overhead는 줄지만 유지보수·ABI·Python model code 호환 위험이 크다.
2. **Python sidecar를 앱이 항상 자동 실행**: 사용자 경험은 단순해질 수 있으나 MVP에서 environment·모델 다운로드·로그 수명 관리가 범위를 키운다.
3. **클라우드 API**: 로컬성·프라이버시·오프라인 목표에 어긋난다.

### 결과

긍정:

- provider 교체와 독립 업데이트가 쉽다.
- Rust의 상태와 모델 runtime memory를 분리해 오류 복구가 명확하다.

부정:

- 사용자가 서버를 먼저 준비해야 한다.
- health, loading, timeout과 프로세스 중단 UX를 구현해야 한다.
- HTTP API의 “OpenAI-compatible” 세부 차이를 adapter가 감수한다.

### 보안 결과

기본 bind와 URL은 숫자 loopback만 사용한다. 외부 bind는 경고하고 제품의 지원 운영 방식으로 보지 않는다. 텔레메트리는 추가하지 않는다.

## ADR-003 - UI와 `story-core`를 엄격히 분리

- 상태: Accepted
- 결정일: 2026-07-31

### 맥락

최초 UI는 TUI지만 최종 목표는 재사용 가능한 스토리 엔진이다. Ratatui widget 상태나 key code가 도메인 타입에 스며들면 웹·미연시 UI와 headless test가 같은 코어를 사용하기 어렵다.

### 결정

`story-core`는 순수 `EngineCommand -> Transition` 경계를 갖고 Ratatui, Crossterm, Reqwest와 파일 시스템에 의존하지 않는다. UI는 key를 domain command로 변환하고 `AppViewModel`을 렌더한다.

### 검토한 대안

1. **단일 app crate의 모듈 분리**: MVP는 단순하지만 dependency 경계를 tooling으로 보장하기 어렵다.
2. **각 화면마다 도메인 로직 소유**: 구현이 직관적이나 저장·재생성·다른 UI에서 규칙이 중복된다.
3. **완전한 ECS**: 텍스트 스토리 상태에 과도한 일반화다.

### 결과

긍정:

- reducer와 100턴 soak를 터미널 없이 실행한다.
- 향후 웹 renderer와 CLI batch benchmark가 같은 core를 사용한다.
- 이벤트·저장·provider를 Mock으로 대체하기 쉽다.

부정:

- command/effect/view model 변환 코드가 추가된다.
- 작은 MVP에 여러 crate가 과해 보일 수 있다.

### 경계 검사

CI에서 `cargo tree -p story-core`를 검사하고 금지 dependency가 들어오면 실패시킨다. UI 전용 `KeyEvent`, `Rect`, `Widget`은 core public API에 나타나면 안 된다.

## ADR-004 - MVP 저장소로 버전이 있는 JSON 파일을 선택

- 상태: Accepted
- 결정일: 2026-07-31

### 맥락

첫 버전은 single-user 로컬 앱이고 자동 저장 1개·수동 슬롯 3개가 필요하다. 상태·memory·history는 구조화되어 있지만 복잡한 동시 query나 다중 process 접근은 필요하지 않다.

### 결정

MVP는 `SaveEnvelope` 안에 버전·checksum·metadata와 payload를 담은 JSON 파일을 사용한다. same-directory temp write, sync, rename과 backup으로 내구성을 확보한다.

### 이유

- 사람이 진단·복구하기 쉽고 fixture와 snapshot을 만들기 좋다.
- Serde 도메인 타입과 직접 연결된다.
- SQLite schema와 migration 도구를 도입하기 전에 실제 데이터 크기와 접근 패턴을 측정할 수 있다.

### 검토한 대안

1. **SQLite**: transaction·부분 query·대용량 history에 좋지만 MVP 복잡도가 증가한다.
2. **RON/MessagePack/CBOR**: Rust 친화적이거나 작지만 사용자 진단성과 다른 도구 호환이 낮다.
3. **이벤트 로그만 저장**: 완전한 replay는 매력적이나 migration·성능·모델 비결정성의 의미가 복잡하다.

### 결과

긍정:

- 구현·fixture·사용자 지원이 단순하다.
- 손상 여부를 checksum과 backup으로 분명히 알 수 있다.

부정:

- history가 커질수록 파일 전체를 다시 쓴다.
- 민감 본문이 평문 로컬 파일에 남는다.
- 동시 write를 지원하지 않는다.

### 재검토 조건

100턴 save가 10MB 이상으로 커지고 write latency가 앱 overhead 목표를 반복해서 넘거나, history 검색·부분 로드가 제품 요구가 되면 SQLite ADR을 작성한다. JSON을 선택했다고 원자성·migration을 생략하지 않는다.

## ADR-005 - MVP 단일 호출 후 Director/Writer를 단계적으로 분리

- 상태: Accepted
- 결정일: 2026-07-31

### 맥락

최종 설계는 장면 계획·규칙 제안과 한국어 표현을 분리하는 것이 안정적이다. 그러나 처음부터 두 호출과 스트리밍을 구현하면 모델 호환, JSON, 상태와 UI 문제를 동시에 디버깅해야 한다.

### 결정

MVP는 한 번의 비스트리밍 호출에서 `narration`, `choices`, `state_patch`, `summary`를 받는다. M7에서 비스트리밍 Director JSON과 스트리밍 Writer 본문으로 분리한다.

### 이유

- M3에서 실제 재미와 한국어 품질을 빠르게 측정할 수 있다.
- 동일 fixture로 단일/2단계 품질과 latency를 비교할 수 있다.
- reducer·검증·저장 경계를 먼저 안정화한 뒤 스트리밍을 추가한다.

### 검토한 대안

1. **처음부터 2단계**: 구조는 깨끗하지만 호출 지연·오류 경우의 수가 배가된다.
2. **항상 단일 호출**: 빠르지만 장기적으로 plan·patch·문체를 한 응답에서 통제하기 어렵다.
3. **Director만 규칙 기반, Writer만 LLM**: 핵심 event는 가능하지만 자유 행동에서 장면 계획의 유연성이 줄어든다.

### 결과

긍정:

- MVP 범위가 작고 end-to-end 검증이 빠르다.
- 최종 architecture의 검증기와 provider trait를 미리 준비할 수 있다.

부정:

- MVP prompt가 복잡하고 JSON truncation 가능성이 있다.
- M7에서 prompt와 API를 리팩터링해야 한다.

### 전환 게이트

MVP 품질을 먼저 통과한다. 2단계는 실제 20턴 A/B에서 규칙 위반률이 개선되고 품질이 악화되지 않으며 latency가 수용 가능할 때 기본값이 된다.

## ADR-006 - 초기 기억 검색에 벡터 데이터베이스를 사용하지 않음

- 상태: Accepted
- 결정일: 2026-07-31

### 맥락

장기 플레이에는 오래된 사건 회상이 필요하지만, 중요한 정보의 대부분은 인물·장소·route·event·미해결 상태처럼 명시적 키를 가진다. 벡터 검색은 임베딩 모델, index lifecycle, 추가 메모리와 설명하기 어려운 회수 실패를 도입한다.

### 결정

v1.0 초기 기억 선택은 고정·상태·최근·요약·사건 계층과 규칙 기반 score를 사용한다. 벡터 검색은 후보 검색 보조로만 향후 검토한다.

### 검토한 대안

1. **처음부터 embedded vector DB**: 비정형 유사성에는 강하지만 설치·평가·migration이 커진다.
2. **모든 history 전송**: 단순하지만 context·latency·비용과 주의 분산이 커진다.
3. **요약 하나만 유지**: 작지만 약속·지식 소유자·관계 원인을 잃기 쉽다.

### 결과

긍정:

- 선택 이유를 trace로 설명하고 결정적으로 테스트할 수 있다.
- 별도 모델·DB 없이 완전 로컬 경로가 단순하다.
- canonical state와 사건 기록의 중요성을 유지한다.

부정:

- 플레이어가 오래전에 말한 비정형 표현을 의미적으로 찾는 능력이 제한된다.
- Pack이 매우 커지면 수동 tag와 규칙 유지 비용이 증가한다.

### 재검토 조건

100턴 중요 기억 회수율 90% 미만, fact 500개 이상, 인물 10명·장소 30개 이상, 비정형 자유 입력 회상 실패가 반복되는 조건 중 둘 이상이 충족되고 blind 평가에서 개선 가능성이 보일 때 새 ADR을 작성한다.

## ADR-007 - Story Pack을 TOML과 Markdown으로 구성

- 상태: Accepted
- 결정일: 2026-07-31

### 맥락

Story Pack은 작가가 편집하는 긴 세계관·문체·prompt와 엔진이 엄격히 검증하는 ID·규칙·변수를 함께 담아야 한다. 한 포맷으로 모두 표현하면 either prose 작성성이 나쁘거나 구조 검증이 약해진다.

### 결정

ID, 버전, 캐릭터 필드, 규칙, event는 TOML을 사용하고 세계관·문체·오프닝·prompt는 Markdown을 사용한다.

### 이유

- TOML은 사람이 읽기 쉬우면서 typed field와 version을 검증하기 좋다.
- Markdown은 긴 한국어 문장, 예문과 prompt를 자연스럽게 편집할 수 있다.
- Git diff와 텍스트 editor workflow에 적합하다.
- 실행 가능한 스크립트가 아니므로 로컬 데이터 보안 경계가 단순하다.

### 검토한 대안

1. **모두 JSON**: schema tooling은 좋지만 긴 prose와 수동 편집성이 떨어진다.
2. **모두 YAML**: 표현력은 높지만 암시적 타입·들여쓰기와 parser 선택이 오류를 만들 수 있다.
3. **Ink/Twine 전용 포맷**: 분기 authoring은 강하지만 자유 생성·Rust state와 통합 범위를 키운다.
4. **Rust 코드로 Pack 작성**: 타입 안전하지만 비개발자가 코드를 수정해야 한다.

### 결과

긍정:

- 작가와 엔진의 관심사를 파일 수준에서 분리한다.
- Pack 추가에 compile이 필요 없다.
- schema version과 migration을 명시할 수 있다.

부정:

- 여러 파일 사이 참조 오류를 별도 validator가 찾아야 한다.
- Markdown prompt placeholder는 정적 schema만으로 완전히 검증하기 어렵다.

### 운영 규칙

Pack은 UTF-8, 제한된 파일 크기와 root-relative 경로만 허용한다. 임의 script를 실행하지 않는다. loader는 오류를 한 번에 모아 파일·필드·가능한 위치를 한국어로 표시한다.

## ADR-008 - 초기 후보 모델은 MLX-VLM 서버로 먼저 검증

- 상태: Accepted
- 결정일: 2026-07-31

### 맥락

제품 요구의 예시 구조는 MLX-LM 서버를 가정하지만, 후보 모델의 공식 모델 카드는 `mlx-vlm 0.6.3` 변환본이라고 명시한다. 서버 이름을 요구사항 그대로 고정하면 실제 아티팩트 형식과 processor를 무시할 위험이 있다.

### 결정

M0에서 후보는 `mlx_vlm.server`의 OpenAI-compatible `/v1/chat/completions`로 먼저 검증한다. `mlx_lm.server`는 호환되는 텍스트 모델 backend로 계속 지원한다. Rust 구현 이름은 `OpenAiCompatibleProvider`로 유지한다.

### 이유

- 모델 카드가 제시한 변환·로딩 도구를 존중한다.
- MLX-VLM 공식 서버는 text-only chat completions, health와 structured output을 제공한다.
- backend 차이를 Rust domain에 노출하지 않는다.

### 검토한 대안

1. **요구 문구대로 무조건 `mlx_lm.server`**: 후보가 지원되지 않을 수 있고 M0를 잘못 실패 처리한다.
2. **모델을 다른 MLX-LM 형식으로 재변환**: 품질·라이선스·재현성 검증 범위가 커진다.
3. **후보를 즉시 교체**: 실제 호환성을 확인하기 전에 사용자 선택을 버린다.

### 결과

긍정:

- 초기 모델을 실제 지원 경로에서 검증한다.
- 향후 MLX-LM·llama.cpp·Ollama로 바꿔도 core와 provider trait는 변하지 않는다.

부정:

- 문서에서 “MLX 계열 서버”와 정확한 실행 명령을 구분해야 한다.
- MLX-VLM의 기본 host가 버전에 따라 외부 bind일 수 있으므로 반드시 `--host 127.0.0.1`을 명시해야 한다.

### 검증

M0 결과 문서에 서버 package/version, 명령, health, request field, JSON 유효률과 모델 revision을 남긴다. 실패 시 추정이 아니라 실제 traceback과 지원 matrix로 대체 모델 결정을 한다.

## 결정 관계 요약

```text
ADR-001 Rust/Ratatui
  └─ ADR-003 UI/Core 분리
       ├─ ADR-004 JSON 저장
       └─ ADR-007 TOML+Markdown Pack

ADR-002 별도 모델 서버
  ├─ ADR-005 단일 호출 -> Director/Writer
  └─ ADR-008 후보는 MLX-VLM 우선

ADR-006 규칙 기반 기억은 모든 backend와 독립
```

ADR이 바뀌면 기존 문서를 조용히 수정하지 않고 새 ADR에서 이전 결정을 `Superseded`로 연결한다.

