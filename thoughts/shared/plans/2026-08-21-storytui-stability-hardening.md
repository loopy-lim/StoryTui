# StoryTUI 안정화 경계 보강 구현 계획

## 개요

현재 더티 워크트리에 이미 반영된 서사 품질·장기 문맥 변경을 보존하면서, 점검에서 확인된 로그 무노출, 파일 경로 격리, Story Pack 불변식, 요약 provenance, 요청 문맥 상한, 체크포인트 일관성, 저장 슬롯 경로 문제를 단계적으로 수정한다. 각 단계는 회귀 테스트와 전체 워크스페이스 게이트로 검증한다.

## 현재 상태 분석

- `apps/storytui/src/main.rs`는 런타임 로그를 크기 제한 없이 append한다.
- `crates/story-provider/src/openai_compatible.rs`는 Director 파싱 실패 시 모델 응답 앞부분을 원문으로 기록하며, 다중 메시지 요청에서 마지막 메시지만 잘라 전체 문맥 상한을 보장하지 못한다.
- `crates/story-persistence/src/pack/loader.rs`는 문자열 `..` 검사는 하지만 canonical child가 Pack root 안에 남는지 확인하지 않는다.
- `crates/story-persistence/src/pack/dto.rs`, `migrate.rs`, `validate.rs`는 엔진 최소 버전, 선택지 개수, 중복 ID, 범위, 위치 도달성 불변식을 완전히 검증하지 않는다.
- `crates/story-core/src/summary.rs`의 검증기는 실제 턴 수락 경로와 연결되지 않았고, 모델 interpretation이 장기 문맥에 직접 저장된다.
- `crates/story-core/src/engine.rs`의 선택지 갱신은 현재 history만 바꾸고 같은 상태를 보관한 checkpoint를 동기화하지 않는다.
- `crates/story-persistence/src/json_store.rs`는 외부에서 전달받은 slot id를 파일명에 그대로 사용한다.
- `config/storytui.toml`의 선택지 보조 모델이 Writer와 같은 27B라서 주석에 적힌 4B 최적화가 적용되지 않는다.

## 목표 상태

- 로그에는 모델 응답, prompt, player free input이 기록되지 않으며 런타임 로그 파일 크기는 세션 중에도 제한된다.
- 모든 Pack 파일은 canonical Pack root 내부 파일만 읽고, escape symlink는 `UnsafePath`로 거부한다.
- 지원 엔진 버전과 팩 데이터 불변식이 로드 시점에 fail-closed로 검증된다.
- 유효한 source history에 연결되지 않은 모델 요약은 장기 문맥에서 폐기되고 narration 기반 digest로 대체된다.
- Turn/Director/Writer 요청의 모든 메시지 합계가 설정된 입력 예산을 넘지 않는다.
- 선택지 갱신 후 history, autosave 대상, checkpoint가 같은 선택지 집합을 가진다.
- 저장 슬롯은 제한된 안전한 identifier만 허용한다.
- 선택지 재생성은 현재 OMLX가 광고하는 4B 모델을 사용한다.

## 범위 제한

- 실제 사람 3인의 한국어 품질 평가는 자동화하지 않는다.
- 별도 clean-machine과 별도 Apple Silicon 플레이어 머신을 현재 호스트로 대체하지 않는다.
- 실제 100턴 플레이와 서버 kill/restart 증빙은 코드 수정 완료와 분리된 runtime release gate로 유지한다.
- Pack 조건 DSL 전체에 대한 완전한 정적 실행 가능성 증명은 하지 않는다. 초기 위치와 인접 그래프로 확실히 판정 가능한 location 도달성만 검사한다.

## 구현 접근 방식

기존 공개 동작은 가능한 한 유지하되 보안 경계는 fail-closed로 바꾼다. 모델 출력 품질 오류는 이미 수락 가능한 턴을 잃지 않도록 해당 파생 데이터만 폐기한다. 파일·slot identifier·Pack metadata처럼 실행 전에 확정할 수 있는 오류는 로드/저장 전에 거부한다.

## Phase 1: 로그와 파일시스템 경계

### 필요한 변경사항

#### 런타임 로그

**파일**: `apps/storytui/src/main.rs`, `crates/story-provider/src/openai_compatible.rs`

- Director 실패 로그에서 raw content head를 제거하고 구조·길이 metadata만 남긴다.
- 제한 크기를 넘으면 파일을 truncate/restart하는 bounded writer를 도입한다.

#### Pack 파일 경계

**파일**: `crates/story-persistence/src/pack/loader.rs`, `crates/story-persistence/tests/loader_basic.rs`

- Pack root canonicalize 실패를 오류로 처리한다.
- 각 실제 읽기 경로를 canonicalize하고 canonical root prefix를 벗어나면 거부한다.
- 원래 symlink 경로가 교체되더라도 escape 대상을 읽지 않도록 canonical target을 읽는다.

#### 저장 슬롯 경계

**파일**: `crates/story-persistence/src/json_store.rs`, 관련 persistence tests

- slot id를 ASCII 영숫자, `-`, `_`, 1~64자로 제한한다.
- save/load/delete/slot_path 모두 동일 validator를 사용한다.

### 성공 기준

#### 자동 검증

- [x] raw Director response를 기록하는 필드가 코드에 없음
- [x] escape symlink Pack 통합 테스트 통과
- [x] traversal/빈 값/과대 slot id 거부 테스트 통과
- [x] `cargo test -p story-persistence` 통과

#### 수동 검증

- [ ] 로그 메시지에 모델 원문이 보이지 않음
- [ ] 로그 파일이 설정된 최대 크기를 넘지 않음

## Phase 2: Pack 호환성과 불변식

### 필요한 변경사항

**파일**: `crates/story-persistence/src/error.rs`, `pack/migrate.rs`, `pack/dto.rs`, `pack/validate.rs`, loader integration tests

- `min_engine_version`과 현재 crate 버전을 비교하고 미래 엔진 요구 Pack을 거부한다.
- `default_choice_count`를 provider 계약과 같은 `1..=6`으로 제한한다.
- location/flag/number/string/item/route/ending/event ID 중복과 빈 ID를 거부한다.
- delta/stat/number range와 default 범위를 검증한다.
- 초기 location에서 인접 그래프로 도달 불가능한 location을 보고한다.

### 성공 기준

#### 자동 검증

- [x] 미래 engine version Pack 거부 테스트 통과
- [x] 선택지 범위, 중복 ID, 역전 범위, 범위 밖 default 테스트 통과
- [x] 도달 불가능 location 테스트 통과
- [x] 두 checked-in Pack loader 테스트 통과

#### 수동 검증

- [x] 오류 메시지가 Pack 작성자가 수정할 field/id를 식별함

## Phase 3: 모델 파생 상태와 요청 예산

### 필요한 변경사항

#### 요약 provenance

**파일**: `crates/story-core/src/engine.rs`, `crates/story-core/src/summary.rs`, core tests

- 현재 생성된 history entry를 source 후보에 포함해 `validate_summary`를 실행한다.
- source가 비어 있으면 엔진이 현재 history id를 부여한다.
- 존재하지 않는 source가 있으면 model interpretation을 폐기하고 narration 첫 절 digest로 폴백한다.

#### Context hard bound

**파일**: `crates/story-provider/src/openai_compatible.rs`

- 모든 메시지에 비례 예산을 배분하고 각 메시지를 UTF-8 안전하게 truncate한다.
- 첫 system과 마지막 instruction에 높은 가중치를 주고 총합을 hard bound한다.
- Turn뿐 아니라 Director와 Writer 요청에도 같은 함수를 적용한다.

#### 체크포인트 동기화

**파일**: `crates/story-core/src/engine.rs`, reducer tests

- 선택지 갱신 성공 후 동일 session/turn/generation checkpoint를 교체한다.
- 일치 checkpoint가 없으면 현재 stable state를 ring cap 안에 추가한다.

### 성공 기준

#### 자동 검증

- [x] 잘못된 source summary가 recent arc를 오염시키지 않는 테스트 통과
- [x] 2개/4개 메시지와 작은 budget에서도 총합 상한 테스트 통과
- [x] choice refresh 후 checkpoint restore 일관성 테스트 통과
- [x] `cargo test -p story-core -p story-provider` 통과

#### 수동 검증

- [x] 실제 OMLX smoke에서 한국어/JSON/선택지 gate 유지

## Phase 4: 설정·문서·전체 게이트

### 필요한 변경사항

**파일**: `config/storytui.toml`, `docs/KNOWN-LIMITATIONS.md`, `docs/RELEASE-RUNTIME-GATE.md`, 본 계획

- 현재 OMLX 목록의 4B 모델을 `choice_model`에 지정하고 주석을 실제 설정과 맞춘다.
- 닫힌 코드 문제와 여전히 runtime-only인 gate를 문서에서 분리한다.
- 구현 완료 항목을 본 계획에서 체크한다.

### 성공 기준

#### 자동 검증

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [x] `./scripts/check-core-deps.sh`
- [x] `git diff --check`
- [x] `cargo build --release -p storytui`
- [x] ignored OMLX smoke 3개 gate 통과

#### 수동 검증

- [ ] G1 100턴+server kill/restart, G2 clean machine, G3 사람 평가, G4 배포 checksum, G5 second-Pack 10턴은 별도 증빙 없이는 완료로 표시하지 않음

## 테스트 전략

### 단위 테스트

- 로그 writer의 크기 상한과 truncate 동작
- slot id validator의 허용/거부 집합
- 메시지 budget allocator의 hard cap
- summary digest의 valid/invalid source 처리
- location reachability와 Pack range validator

### 통합 테스트

- 임시 Pack과 외부 파일 symlink를 사용한 root escape 재현
- loader를 통한 engine version/duplicate/range/reachability 오류 확인
- 실제 reducer choice refresh 후 checkpoint restore
- 두 checked-in Pack 로드와 전체 workspace 회귀

### 수동 테스트

1. 현재 OMLX `/health`, `/v1/models`를 확인한다.
2. Writer와 choice model을 지정해 ignored smoke를 실행한다.
3. 코드로 대체할 수 없는 release runtime gate는 문서의 OPEN 상태로 유지한다.

## 성능 고려사항

- Pack canonicalize는 시작 시 최대 10개 필수 파일에만 수행한다.
- Context bounding은 메시지와 문자열 길이에 선형이며 네트워크 호출 전에 한 번 실행한다.
- 로그 writer는 파일 크기를 메모리 counter로 유지해 매 로그마다 metadata syscall을 하지 않는다.

## 마이그레이션 참고사항

- 기존 정상 슬롯명(`autosave`, `slot-1`~`slot-3`)은 계속 유효하다.
- 명시적 `default_choice_count = 0`이나 잘못된 range/중복 ID를 가진 Pack은 이전과 달리 로드가 거부된다.
- `min_engine_version`이 현재 `story-persistence` crate version보다 높으면 앱 업데이트 전까지 로드할 수 없다.

## 참고 자료

- 기존 릴리스 경계: `docs/RELEASE-RUNTIME-GATE.md`
- Pack 설계: `docs/StoryTUI_Technical_Spec_KO.md`
- 기존 장기 문맥 설명: `docs/CONTEXT-MANAGEMENT.md`
