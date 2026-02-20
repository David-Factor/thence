#!/usr/bin/env bash
set -euo pipefail

role="${WHENCE_ROLE:-}"
prompt_file="${WHENCE_PROMPT_FILE:-}"
result_file="${WHENCE_RESULT_FILE:-}"
worktree="${WHENCE_WORKTREE:-.}"
attempt="${WHENCE_ATTEMPT:-1}"
task="${WHENCE_TASK_ID:-unknown}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
assets_dir="${script_dir}/codex-real"

if [[ -z "$role" || -z "$prompt_file" || -z "$result_file" ]]; then
  echo "missing required WHENCE_* env vars" >&2
  exit 2
fi

render_prompt() {
  local template="$1"
  while IFS= read -r line; do
    line="${line//\{\{TASK_ID\}\}/$task}"
    line="${line//\{\{ATTEMPT\}\}/$attempt}"
    if [[ "$line" == "{{PROMPT_CONTEXT}}" ]]; then
      cat "$prompt_file"
    else
      printf '%s\n' "$line"
    fi
  done < "$template"
}

run_codex_json() {
  local schema_file="$1"
  local prompt_template="$2"
  local final_prompt
  final_prompt="$(render_prompt "$prompt_template")"
  codex exec \
    --ephemeral \
    --skip-git-repo-check \
    -C "$worktree" \
    --sandbox danger-full-access \
    --dangerously-bypass-approvals-and-sandbox \
    --output-schema "$schema_file" \
    --output-last-message "$result_file" \
    "$final_prompt" >/dev/null
}

case "$role" in
  checks-proposer)
    schema="${assets_dir}/schemas/checks-proposer.schema.json"
    prompt_template="${assets_dir}/prompts/checks-proposer.txt"
    run_codex_json "$schema" "$prompt_template"
    ;;

  implementer)
    schema="${assets_dir}/schemas/implementer.schema.json"
    prompt_template="${assets_dir}/prompts/implementer.txt"
    run_codex_json "$schema" "$prompt_template"
    ;;

  reviewer)
    schema="${assets_dir}/schemas/reviewer.schema.json"
    if [[ "$attempt" == "1" ]]; then
      prompt_template="${assets_dir}/prompts/reviewer-attempt1.txt"
    else
      prompt_template="${assets_dir}/prompts/reviewer-later.txt"
    fi
    run_codex_json "$schema" "$prompt_template"
    ;;

  *)
    echo "unknown role: $role" >&2
    exit 3
    ;;
esac
