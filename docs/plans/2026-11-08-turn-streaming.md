# 계획: MVP 생성 경로 SSE 스트리밍 렌더링 (이슈 #2, P0-1)

작성: 2026-11-08 · 상태: 완료 (G1 PASS · G3 PASS · G4 N/A · G5 PASS, 2026-11-08) · PR 리뷰 후 수정(아래 "PR 리뷰 반영" 참조)

## 목표와 수용 기준 (G0)

- **AC-1** 생성 중 내레이션이 터미널에 점진 렌더링된다 (진행 표시 + 부분 텍스트).
- **AC-2** `c` 취소 키가 스트리밍 중에도 동작한다 (CancelGeneration 경로 유지).
- **AC-3** 스트리밍 실패/중단 시 기존 안정 체크포인트 복구 경로가 그대로 동작한다.
- **AC-4** Director/Writer 경로와 단일 경로 모두 동작이 정의된다 (스트리밍 불가 시 기존 동작 폴백 포함).
- **AC-5** `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` 통과.

제약: ADR-003 — 스트리밍 렌더 상태는 story-tui/host 계층 소유. story-core(`EngineCommand`/`EngineEffect`)는 무수정.

## 조사 결론 (근거)

- Writer(Director/Writer) 경로의 스트리밍 체인이 이미 완전하다:
  `write_stream`(`crates/story-provider/src/openai_compatible.rs:1067`) → `WriterEvent::Chunk` mpsc(32) → `HostMessage::WriterChunk`(`apps/storytui/src/host.rs:1103`) → `App::push_writer_chunk`(`crates/story-tui/src/app.rs:993`, generation-id 가드·late chunk 폐기) → `streaming_narration` 부분 렌더(`screens/story.rs:660`).
- MVP 경로는 `host.rs:1024-1031`의 `provider.generate_turn` spawn이 전부. 취소 토큰(`host.rs:281` 단일 슬롯), 체크포인트 복구(엔진 `reduce_provider_failed`), 로딩 오버레이는 이미 동작.
- `generate_turn` 응답은 완성 단일 JSON(narration+choices+state_patch+summary). `parse_generated_turn`(`recovery.rs:66`)은 완성 텍스트 전제 — 부분 JSON에서 narration 값만 점진 추출하는 별도 추출기 필요.
- 베이스라인: `cargo test --workspace` PASS. **clippy는 기존 실패** — `LlmProvider` 트레이트의 `#[async_trait]` × clippy 1.99 `double_must_use` 4건(`provider.rs:28`). AC-5를 위해 함께 수정.
- MockProvider는 `supports_streaming: false`(`mock.rs:79-83`) — 폴백 경로의 자연스러운 테스트 베이이자 기존 e2e(`apps/storytui/tests/mock_e2e.rs`) 무영향 보장.

## 설계 결정

- **D1 트레이트 확장**: `LlmProvider`에 `generate_turn_stream(&self, request: TurnRequest, sink: Sender<WriterEvent>, cancel: CancellationToken) -> Result<GeneratedTurnDraft, ProviderError>` 추가. 기본 구현 `Err(UnsupportedCapability("turn_streaming"))` — `direct`/`write_stream` 선례(provider.rs:52-72). 청크 DTO는 기존 `WriterEvent::Chunk`/`Done` 재사용(story-model 무수정). 청크는 **디코딩된 narration 산문만** 담는다.
- **D2 OpenAI 구현**(`openai_compatible.rs`): `do_generate`와 동일한 요청 조립(temperature/top_p/top_k/max_tokens 클램프, `bound_context`, 모델 해석, `structured_enabled`+`json_schema` 기반 response_format) 후 `body.stream = true`. `write_stream` 선례에 따라 **단일 시도**, cancel-select + `SseParser` 루프, 전체 텍스트 누적 + 점진 추출기 청크 전달. `[DONE]` 후 누적 전문을 기존 `parse_generated_turn`/`parse_generated_choices`(`is_choice_only_request` 분기)로 최종 파싱해 `GeneratedTurnDraft` 반환 — 최종 커밋 계약은 기존 파서를 그대로 통과한다. 청크 노출 후에는 재시도 없음(실패 → `Err` → 엔진 체크포인트 복구, AC-3).
- **D3 점진 narration 추출기**(신규 `crates/story-provider/src/narration_stream.rs`): 상태 머신(Scanning → AwaitingValue → InString → Done). `"narration"` 토큰 직후 문자열 값만 이스케이프(`\"` `\\` `\n` `\uXXXX` 서로게이트 페어 포함)를 디코딩해 증분 반환. 이스케이프가 청크 경계로 잘리는 경우 버퍼링. 파싱 실패·키 미발견 시 청크 없음(로딩 오버레이는 유지) — 부분 텍스트는 최종 수락 시점에 `streaming_narration` 해제로 자동 교체되므로 과도 표시는 자가 치유된다.
- **D4 provider 내부 폴백**: `generate_turn_stream`에서 **첫 청크 전송 전** 실패(연결/4xx/5xx/디코드)면 `self.generate_turn(request, cancel)`으로 위임 — 기존 재시도 사다리(400 auto-fallback, envelope 실패 schema-free 재시도 등)가 그대로 재사용된다. 청크 노출 후 실패는 `Err` 그대로 전파. 폴백이 provider 내부에 있으므로 wiremock으로 직접 검증 가능. 4xx 포함 근거: `stream:true` 거부는 스트리밍 고유 실패일 수 있어 비스트리밍 1회 재시도는 ADR-002(4xx 무재시도)의 예외가 아니라 다른 요청 형태로의 폴백이며, 재시도는 폴백 내부에서 1회로 한정된다. 참고: length-expansion 재시도(`min_narration_chars`, openai_compatible.rs:828-857)는 스트리밍 경로에서 생략된다(현재 프로필값 0으로 동면; KNOWN-LIMITATIONS에 기록).
- **D5 호스트 배선**(`host.rs` RequestGeneration arm): `config.provider.stream_turns && provider.capabilities().supports_streaming`일 때만 `generate_turn_stream` 사용, 아니면 기존 `generate_turn`. 스트리밍 spawn은 Writer arm(host.rs:1095-1118) 구조 복제 — mpsc(32) select 루프에서 `Chunk`를 기존 `HostMessage::WriterChunk`로 전달(앱 렌더 경로·generation-id 가드 무수정). 완료는 기존 `HostMessage::Generation` → `AcceptGeneration`. 취소 토큰 단일 슬롯 구조 무수정(AC-2).
- **D6 설정**: `ProviderConfig`에 `stream_turns: bool`(serde 기본값 `true`) 추가. `config/storytui.toml` `[provider]`에 주석과 함께 명시. 설정 UI 노출은 이번 범위 외(KNOWN-LIMITATIONS에 기록).
- **D7 clippy 기존 결함 수정**: `LlmProvider` 트레이트에 `#[allow(clippy::double_must_use)]` (async_trait 매크로 유발, 러스트 1.99 신규 엄격화).
- **D8 문서**: `docs/KNOWN-LIMITATIONS.md`의 "No streaming in the MVP binary path" 단락을 새 동작(스트리밍, 단일 시도, 첫 청크 전 실패 시 비스트리밍 폴백)으로 교체.

## 태스크 (담당: Backend/Core Engineer, 순차 실행 — 파일 겹침)

| ID | 내용 | 파일 | AC | 검증(V) |
|---|---|---|---|---|
| T1 | 점진 narration 추출기 + 단위 테스트 | `crates/story-provider/src/narration_stream.rs`, `lib.rs` | AC-1 | V1 단위 테스트(증분 한국어, 이스케이프, 청크 경계 분할 \uXXXX, 키 미발견) |
| T2 | 트레이트 메서드 + OpenAI 구현 + provider 내부 폴백 + clippy allow | `provider.rs`, `openai_compatible.rs`, `envelope.rs`(필요 시) | AC-1,2,3,4 | V2 계약 테스트, V3 취소, V5 폴백 |
| T3 | 호스트 배선 + 설정 플래그 | `host.rs`, `crates/story-provider/src/config.rs`, `apps/storytui/src/config.rs`(기본값 전파 확인), `config/storytui.toml` | AC-1,2,4 | V6 `cargo test/clippy`, 기존 mock_e2e 무영향 |
| T4 | wiremock 계약 테스트 | `crates/story-provider/tests/turn_streaming_contract.rs` | AC-1,2,4 | V2·V3·V5 실행 |
| T4b | app 레벨 단위 테스트: `push_writer_chunk` → story_blocks 스트리밍 엔트리 포함, `set_view` 완료 시 해제 (검토 권고 1) | `crates/story-tui/src/app.rs` 내 테스트 모듈 또는 기존 테스트 파일 | AC-1 | V7 |
| T5 | 문서 갱신 | `docs/KNOWN-LIMITATIONS.md` | AC-4 | 문서 검토 |

## V-* 검증 정의

- **V1** 추출기 단위 테스트: 증분 feed가 디코딩된 narration만 순서대로 반환; 이스케이프(`\n` `\"` `😀`), 청크 경계로 이스케이프가 잘리는 입력, narration 키 없는 JSON(출력 없음), JSON 앞 텍스트(`<think>` 등) 통과.
- **V2** wiremock: GeneratedTurn JSON을 SSE delta로 분할(한글 멀티바이트 분할 포함) → sink는 narration 산문만 수신(JSON 토큰 노출 없음), 반환 draft는 전체 파싱 결과와 동치, usage 부착.
- **V3** wiremock: 스트림 중간 cancel → `ProviderError::Cancelled` (기존 `director_writer_contract.rs` 취소 테스트 패턴).
- **V4** AC-3: 기존 엔진 체크포인트 복구 테스트(`story-core` reduce_provider_failed 계열)가 통과하는 것으로 갱신 없음을 확인. 호스트 에러 경로 무수정.
- **V5** wiremock 폴백: 스트리밍 요청이 사전 실패(예: 첫 응답 5xx/프로토콜 오류) → 비스트리밍 재요청 성공 → 정상 draft 반환·청크 0건. MockProvider(기본 `UnsupportedCapability`, `supports_streaming:false`)는 기존 `mock_e2e`에서 무영향 통과.
- **V6** `cargo test --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` 통과(기존 clippy 실패 4건 포함 해소).
- **V7** story-tui 단위: 청크 push 후 `story_blocks`에 스트리밍 엔트리 존재, 완료 뷰(`set_view`, loading=None)에서 `streaming_narration` 해제 확인.

## 위험과 완화

- **부분 JSON 추출 정확성**(중첩 객체에 동명 키 등): 최종 커밋은 항상 기존 `parse_generated_turn`을 통과하므로 오염은 일시적 표시에 한정됨. 추출기는 첫 `"narration"` 문자열 값만 대상.
- **재시도 사다리 상실**(첫 청크 이후): 로컬 모델 정책상 timeout 재시도는 본래 무의미(openai_compatible.rs:411 주석). 실패 시 체크포인트 복구 + 재생성이라는 기존 UX 유지.
- **요청 타임아웃이 스트림 전체에 적용**(reqwest client timeout): 코드 기본값 180s, 배포 프로필 `config/storytui.toml`은 600s. 기존 `write_stream`과 동일 조건이므로 문서에만 기록한다. (timeout 재시도 무의미 주석은 openai_compatible.rs:412)

## PR 리뷰 반영 (2026-11-08, PR #7)

1. `[DONE]` 센티넬 없이 닫힌 스트림도 누적 전문이 엄격 파싱을 통과하면 수용(파싱 실패 시에만 Protocol 에러).
2. 청크 노출 후 `InvalidGeneration`(예: max_tokens 절단)은 비스트리밍 수리 1회 재시도 — 스트리밍 텍스트는 표시 전용이므로 중복 없음. 기존 기본 경로의 자가 치유 동작과 동등.
3. 값 종료 시점에 stranded high surrogate 즉시 flush(U+FFFD).
4. malformed `\u` 이스케이프 재생 시 버퍼링된 hex 자릿수까지 재생.
5. host의 청크 포워딩 루프를 `pump_writer_events` 공유 헬퍼로 통합(MVP·Writer 경로 동형 보장).
6. 턴 요청 조립 블록을 `assemble_turn_body` 공유 헬퍼로 통합(스트리밍·비스트리밍 샘플링 파라미터 일치 보장).
7. `trim_scan_buf`를 드레인 기반으로 변경(긴 prelude에서 글자당 힙 할당 제거).
