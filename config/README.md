# Provider 설정

StoryTUI는 OMLX에 종속되지 않고 OpenAI-compatible HTTP provider를 사용한다.
`[provider]`의 `base_url`, `chat_path`, `health_path`, `model`만 맞으면
MLX-LM, MLX-VLM, OMLX 또는 다른 호환 서버를 선택할 수 있다.

## 기본 실행

```bash
cargo run -p storytui
```

실제 모델 ID는 Git에 추적되지 않는 `storytui.local.toml`에 둔다. 이 파일이
있으면 별도 인자 없이 자동으로 우선 사용하고, 없을 때만 공개 템플릿인
`storytui.toml`을 읽는다. 시작할 때 `/v1/models`를 짧게 조회하며, 저장된
모델이 서버에서 사라졌으면 로컬 프로필 모델 또는 서버의 첫 모델로 자동
복구하고 `saves/user-settings.toml`에 반영한다.

```bash
cp config/storytui.toml config/storytui.local.toml
# storytui.local.toml의 provider.model과 선택 단계 모델을 로컬 값으로 설정
cargo run -p storytui
```

`config/*.local.toml`과 `saves/`는 `.gitignore` 대상이다. 실제 모델 ID를
공개 템플릿, 문서 또는 커밋 메시지에 복사하지 않는다.

TUI에서는 `o` → `p`로 모델 목록을 열 수 있다. `Enter`는 메인 모델,
`d`는 Director 모델, `w`는 Writer 모델로 선택하며 선택 결과는 현재
`save_dir` 아래의 `user-settings.toml`에 저장된다.

## OpenAI 호환 서버 (클라우드·게이트웨이)

`base_url`만 바꾸면 어떤 OpenAI 호환 서버든 그대로 쓸 수 있다. 호스티드
엔드포인트는 관례상 base_url이 `/v1`으로 끝나는데, 이 경우 요청 경로
(`chat_path`, `models_path`)가 아직 기본값일 때 자동으로 맞춰진다 —
프리셋이나 경로 재정의 없이 URL 교체만으로 충분하다.

```toml
[provider]
# 예: OpenRouter — Groq·LM Studio·vLLM·OpenAI 등도 같은 형태
base_url = "https://openrouter.ai/api/v1"
model = "사용하려는-모델-ID"
api_key_env = "MY_COMPATIBLE_KEY"
```

```bash
export MY_COMPATIBLE_KEY="..."
cargo run -p storytui
```

참고 사항:

- `/health`가 없는 서버(클라우드 대부분)는 모델 목록(`/models`)으로
  상태를 대신 확인한다. `/health`를 제공하는 로컬 서버는 기존대로
  동작하고, `/health`가 없는 로컬 서버도 같은 폴백의 혜택을 받는다.
- OpenAI 계열 추론 모델처럼 `max_tokens`를 거부하는 서버는
  `max_tokens_field = "max_completion_tokens"`로 바꾼다.
- `structured_output = "auto"` 상태에서 스키마를 거부하는 서버(OpenAI
  strict 모드 등)는 첫 400에서 response_format 없이 재시도하고 세션
  동안 스키마를 끈다. 처음부터 끄려면 `structured_output = "off"`.
- `top_k`를 거부하는 서버는 `top_k = 0`(전송 생략)으로 둔다.
- 비밀값 관리는 아래 로컬 서버 규칙과 같다. `api_key_env` 외에
  `api_key_file`도 사용할 수 있다.

## 여러 프로바이더 구성과 TUI 전환

`[provider]`(기본) 아래에 `[[providers]]` 테이블을 나열하면 여러 프로파일을
한 번에 정의할 수 있다. 각 항목은 `[provider]`와 같은 필드를 쓰고 `name`으로
구별한다. 이름이 중복되면 시작이 거부된다.

```toml
[provider]
name = "omlx-local"
base_url = "http://127.0.0.1:8000"
model = "로컬-모델-ID"

[[providers]]
name = "openrouter"
base_url = "https://openrouter.ai/api/v1"
model = "호환-모델-ID"
api_key_env = "OPENROUTER_KEY"

[[providers]]
name = "groq"
base_url = "https://api.groq.com/openai/v1"
model = "호환-모델-ID"
api_key_file = "groq-key"        # 설정 파일 디렉터리 기준 상대 경로
```

TUI에서 `o`(설정) → `e`로 프로바이더 선택기를 열고 Enter로 전환한다.
전환은 재시작 없이 즉시 적용되며 — 새 엔드포인트·키로 공급자를 다시 만들고,
상태와 모델 목록을 새 서버 기준으로 갱신하고, 새 서버에 없는 저장 모델은
활성 모델로 자동 수리한다. 선택은 `user-settings.toml`의 `provider`에
유지되어 다음 실행에서 자동 선택된다. 모델 선택은 프로필 간 공유되지 않는다 —
프로필을 바꾸면 그 프로필의 모델로 저장되므로, 자주 쓰는 모델은 각
프로필의 `model`에 두는 것이 좋다.

## 인증이 필요한 서버

비밀값을 TOML에 직접 쓰지 않고 다음 중 하나를 선택한다.

환경변수:

```toml
[provider]
api_key_env = "MY_LOCAL_LLM_KEY"
```

```bash
export MY_LOCAL_LLM_KEY="..."
cargo run -p storytui
```

별도 파일:

```toml
[provider]
api_key_file = "provider-key"
```

위 경로는 이 TOML 파일이 있는 디렉터리 기준이다. 예를 들어
`config/storytui.local.toml`에서 지정하면 `config/provider-key`를 읽는다.

```bash
printf '%s\n' '...' > config/provider-key
chmod 600 config/provider-key
cargo run -p storytui -- --provider-config config/storytui.local.toml
```

`api_key_file`과 `api_key_env`는 상호 배타적으로 쓰는 것을 권장한다.
둘 다 있으면 파일 값을 우선한다. 비밀값은 요청 시 Bearer token으로만
사용하고 세이브·진단·Settings 화면에는 넣지 않는다.

## 내레이션 낭독(TTS)

`v` 키로 낭독을 켜고 끈다. 상태는 `save_dir/user-settings.toml`에 유지되고
Settings 화면(`o`)의 "상태" 블록에 표시된다. 기본 백엔드는 macOS 내장
`say`(무료·오프라인)이며, 한국어 보이스가 설치돼 있으면 자동으로 고른다.

```toml
[tts]
# 시작 시 기본값. v 토글이 이 값을 덮어쓴다.
enabled = false
# "say" (기본) 또는 "openai"
backend = "say"
# 백엔드 보이스. 미설정 시 say는 한국어 보이스를 자동 감지하고,
# openai는 서버 기본값을 쓴다.
# voice = "Yuna"
# backend = "openai"일 때:
# openai_model = "gpt-4o-mini-tts"
# base_url = "https://api.openai.com/v1"
# api_key_env = "OPENAI_API_KEY"     # 또는 api_key_file = "tts-key"
```

`backend = "openai"`는 OpenAI TTS API(`/audio/speech`)로 mp3를 받아
`afplay`로 재생한다. 키는 자체 `api_key_file`/`api_key_env` 순으로 해석하고,
둘 다 없으면 provider가 이미 해석한 키(예: 클라우드 프로필의 `api_key_file`)를
재사용한다. 비밀값은 로그·세이브·설정 화면에 never 기록되지 않는다.

스키마를 거부하는 서버(OpenAI strict 모드 등)에서는 제약 없는 JSON
프롬프트+복구 파서가 기본 전략이다. `structured_output = "auto"` 상태에서
스키마 관련 400을 받으면 response_format 없이 한 번 재시도하고 세션 동안
스키마를 끈다. 처음부터 끄려면 `structured_output = "off"`.

## 팩 이미지

스토리 팩 디렉터리에 `images/` 폴더를 두면 스토리 화면에 표시된다.
이미지가 없는 팩은 기존과 동일하게 동작한다.

```
stories/<pack>/
  images/
    locations/<location-id>.png   # 현재 장소 이미지 (우선)
    characters/<character-id>.png # 없으면 주요 캐릭터 초상
```

- 지원 확장자: `.png`, `.jpg`, `.jpeg`, `.webp` (이 순서로 탐색)
- 장소 이미지가 손상돼 있으면 캐릭터 초상으로 자동 폴백한다
- kitty 그래픽 프로토콜(Ghostty/kitty/WezTerm)에서 선명하게 렌더링되고,
  미지원 터미널에서는 halfblock으로 표시되거나 생략된다
- 설정 화면(`o`)의 "상태" 블록에서 감지된 프로토콜을 확인할 수 있고,
  `i` 키로 이미지 표시를 켜고 끌 수 있다(선택은 user-settings에 유지)

## 일회성 provider/model 변경

```bash
cargo run -p storytui -- \
  --base-url http://127.0.0.1:8080 \
  --model mlx-community/your-model \
  --pack ./stories/rainy-reunion
```

전체 옵션은 다음으로 확인한다.

```bash
cargo run -p storytui -- --help
```

## 캐릭터 카드 임포트

SillyTavern/RisuAI 캐릭터 카드(V1/V2/V3, PNG `chara`/`ccv3` 또는 JSON)를 네이티브
팩으로 컴파일한다.

```bash
# 기본: stories/<카드명-slug> 에 팩 생성
cargo run -p storytui -- import card.png

# 옵션: 출력 위치, World Info 병합, {{user}} 치환, Director 정규화
cargo run -p storytui -- import card.json \
  -o stories/yuna \
  --lorebook world-info.json \
  --player-name 하늘 \
  --normalize --provider omlx-local
```

- 원본 카드는 `sources/card.json`에 그대로 보존되고, 매핑/무시/근사 내역은
  `import-report.md`에 기록된다. 임포트가 끝나면 PackLoader 검증을 통과한 팩만
  남는다.
- 로어북(`character_book`, World Info 단독 파일 모두)은 `lorebook.toml`로
  저장되어 턴마다 키워드 매칭으로 프롬프트에 주입된다(예산·우선순위·재귀 지원).
- `--normalize`는 설명 본문을 로컬 모델로 구조화 필드(personality, 말투, 목표,
  알고/모르는 사실)에 옮기는 생성 1회짜리 후속 단계다. 실패해도 결정적 임포트
  결과는 그대로 남는다.
- TUI에서도 시작 메뉴의 `카드 가져오기`로 같은 임포트를 실행할 수 있다(옵션은
  CLI만 제공).
