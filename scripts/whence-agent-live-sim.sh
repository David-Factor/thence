#!/usr/bin/env bash
set -euo pipefail

role="${WHENCE_ROLE:-}"
result_file="${WHENCE_RESULT_FILE:-}"
attempt="${WHENCE_ATTEMPT:-1}"
task="${WHENCE_TASK_ID:-unknown}"
worktree="${WHENCE_WORKTREE:-.}"
prompt_file="${WHENCE_PROMPT_FILE:-}"

if [[ -z "$role" || -z "$result_file" || -z "$prompt_file" ]]; then
  echo "missing required WHENCE_* env vars" >&2
  exit 2
fi

case "$role" in
  checks-proposer)
    cat > "$result_file" <<'JSON'
{"commands":["test -f work-output.txt","ls *_prompt.json >/dev/null"],"rationale":"verify implementer artifact + prompt presence"}
JSON
    ;;

  implementer)
    # Simulate substantive work in the worktree
    {
      echo "task=$task"
      echo "attempt=$attempt"
      echo "prompt_hash=$(shasum "$prompt_file" | awk '{print $1}')"
      echo "implemented_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    } > "$worktree/work-output.txt"

    cat > "$result_file" <<'JSON'
{"submitted":true}
JSON
    ;;

  reviewer)
    # Force one rework cycle per task: fail attempt 1, pass attempt >=2
    if [[ "$attempt" == "1" ]]; then
      cat > "$result_file" <<'JSON'
{"approved":false,"findings":["Please address reviewer concern from attempt 1"]}
JSON
    else
      cat > "$result_file" <<'JSON'
{"approved":true,"findings":[]}
JSON
    fi
    ;;

  *)
    echo "unknown role: $role" >&2
    exit 3
    ;;
esac
