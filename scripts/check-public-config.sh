#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

scan_mode="${1:---worktree}"
case "$scan_mode" in
  --worktree)
    public_config="$(<config/storytui.toml)"
    token_grep_args=(git grep -I -h -o -E)
    ;;
  --cached)
    public_config="$(git show :config/storytui.toml)"
    token_grep_args=(git grep --cached -I -h -o -E)
    ;;
  *)
    echo "usage: $0 [--worktree|--cached]" >&2
    exit 2
    ;;
esac

if rg -n '^[[:space:]]*(director_model|choice_model|writer_model|model)[[:space:]]*=' \
  <<<"$public_config"
then
  echo "error: config/storytui.toml must not contain machine-local model assignments" >&2
  exit 1
fi

tracked_local="$(git ls-files 'config/*.local.toml')"
if [[ -n "$tracked_local" ]]; then
  echo "error: private local profiles must remain untracked:" >&2
  echo "$tracked_local" >&2
  exit 1
fi

blocked_label_hashes=(
  "885f94e35ab18d9dbe262d9a3c07c50fdff6a3583530477070f62ec5a4e5c192"
  "cd187cd4352f068d7972f067aadbcc30a1e98f0339f3b0b7b4039b40e2b00d4a"
  "0a4ff57f817a36cb24eeeb2b9149d8dc9de8150aafd5f6440453d17d2d61ab47"
  "3df5e4519caaa0e5ec75aa72320a551bdbc94c903ad6de8b70a0e42ee1cafe1d"
  "ab1248a5516d6898fe7dbf1c90d4327e9d1c0898e610918d861b25f3a05f4fe4"
  "dd460a42b14b5bc65c8709d66fdb44ba5433612823e75a919b7d858fc05394fc"
  "12814855d37ac4d8f27a8722c25170baada48771422c09c05faa50f732d2fae1"
)

blocked_match="$({
  "${token_grep_args[@]}" '[[:alnum:]_.+-]+' -- . || true
} | tr '[:upper:]' '[:lower:]' | perl -MDigest::SHA=sha256_hex -e '
  my %blocked = map { $_ => 1 } @ARGV;
  while (<STDIN>) {
    chomp;
    if ($blocked{sha256_hex($_)}) {
      print "blocked\n";
      last;
    }
  }
' "${blocked_label_hashes[@]}")"

if [[ -n "$blocked_match" ]]; then
  echo "error: a machine-local model label is present in tracked content" >&2
  exit 1
fi

echo "OK: public Git content contains no private model selections"
