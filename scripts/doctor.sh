#!/usr/bin/env bash
# storytui doctor — 설정·프로바이더 서버·모델·팩 상태를 한 번에 점검한다.
#
# 사용법: scripts/doctor.sh
#
# 점검 항목:
#   1. 활성 설정 TOML 파싱 (storytui.local.toml 우선, 없으면 storytui.toml)
#   2. 각 프로바이더 서버: /v1/models 도달 여부 + 설정된 model ID가 목록에 있는지
#   3. stories/*/story.toml 최소 필드 점검
#
# 종료 코드: 활성 프로바이더가 정상이면 0, 다운/모델 불일치면 1.
# 대체 프로바이더([[providers]]) 문제는 경고로만 표시한다.

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

config="config/storytui.local.toml"
[[ -f "$config" ]] || config="config/storytui.toml"
echo "● 설정: $config"

# TOML에서 프로바이더 목록을 읽어 TSV(provider|name|base_url|model)로 출력한다.
provider_tsv="$(python3 - "$config" <<'PY'
import sys, tomllib

with open(sys.argv[1], "rb") as f:
    data = tomllib.load(f)

rows = []
if "provider" in data:
    p = data["provider"]
    rows.append(("active", p.get("name", "?"), p.get("base_url", "?"), p.get("model", "")))
for p in data.get("providers", []):
    rows.append(("alt", p.get("name", "?"), p.get("base_url", "?"), p.get("model", "")))
for role, name, url, model in rows:
    print(f"{role}\t{name}\t{url}\t{model}")
PY
)"

if [[ -z "$provider_tsv" ]]; then
  echo "✗ 프로바이더 설정이 없습니다 — [provider] 또는 [[providers]]를 추가하세요" >&2
  exit 1
fi

exit_code=0
while IFS=$'\t' read -r role name url model; do
  [[ -n "$model" ]] || model="(미설정)"
  # 앱과 동일한 경로 정규화: /v1으로 끝나지 않는 base_url에는 /v1을 붙인다.
  api_url="$url"
  [[ "$api_url" != */v1 ]] && api_url="$api_url/v1"
  if ! models_json="$(curl -sS -m 5 "$api_url/models" 2>/dev/null)"; then
    models_json=""
  fi
  if [[ -z "$models_json" ]]; then
    if [[ "$role" == "active" ]]; then
      echo "✗ [$name] 서버 응답 없음 ($url) — 활성 프로바이더가 다운 상태"
      exit_code=1
      continue
    fi
    echo "△ [$name] 서버 응답 없음 ($url) — 대체 프로바이더 (참고용 경고)"
    continue
  fi

  if ! server_models="$(printf '%s' "$models_json" | python3 -c '
import json, sys
try:
    data = json.load(sys.stdin)
    print("\n".join(m.get("id", "?") for m in data.get("data", [])))
except Exception as e:
    print(f"parse-error: {e}", file=sys.stderr)
    sys.exit(1)
' 2>/dev/null)"; then
    echo "△ [$name] /v1/models 응답을 파싱하지 못했습니다 ($url)"
    continue
  fi

  model_count="$(printf '%s\n' "$server_models" | grep -c . || true)"
  if [[ "$model" == "("* ]]; then
    echo "△ [$name] 서버 정상 ($url, 모델 ${model_count}개) — model 미설정: ${model_count}개 중 선택 필요"
    [[ "$role" == "active" ]] && exit_code=1
    printf '%s\n' "$server_models" | sed 's/^/    - /'
  elif printf '%s\n' "$server_models" | grep -Fxq -- "$model"; then
    echo "✓ [$name] 서버 정상, 모델 확인: $model"
  else
    echo "✗ [$name] 서버는 정상이나 모델 미존재: $model ($url)"
    printf '%s\n' "$server_models" | sed 's/^/    - /'
    [[ "$role" == "active" ]] && exit_code=1
  fi
done <<<"$provider_tsv"

echo "● 팩 점검 (stories/*/story.toml)"
pack_count=0
for story_toml in stories/*/story.toml; do
  [[ -f "$story_toml" ]] || continue
  pack_count=$((pack_count + 1))
  pack_dir="$(dirname "$story_toml")"
  if python3 - "$story_toml" <<'PY'
import sys, tomllib

with open(sys.argv[1], "rb") as f:
    data = tomllib.load(f)
missing = [k for k in ("schema_version", "story_id", "title", "language") if not data.get(k)]
if missing:
    print("missing: " + ", ".join(missing))
    sys.exit(1)
PY
  then
    echo "  ✓ $pack_dir"
  else
    echo "  ✗ $pack_dir — story.toml 필드 누락/파싱 실패"
    [[ "$exit_code" -eq 0 ]] && exit_code=1
  fi
done
[[ "$pack_count" -eq 0 ]] && echo "  (stories/에 팩이 없습니다)"

exit "$exit_code"
