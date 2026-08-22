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
