#!/usr/bin/env bash
set -euo pipefail

# thence subprocess adapter for Codex CLI.
# Expected environment from thence provider:
# - THENCE_ROLE
# - THENCE_WORKTREE
# - THENCE_PROMPT_FILE
# - THENCE_RESULT_FILE
# - THENCE_CAPSULE_FILE (implementer/reviewer attempts)

if ! command -v codex >/dev/null 2>&1; then
  echo "codex not found in PATH" >&2
  exit 127
fi

ROLE="${THENCE_ROLE:-}"
WORK_DIR="${THENCE_WORKTREE:-$PWD}"
PROMPT_FILE="${THENCE_PROMPT_FILE:-}"
RESULT_FILE="${THENCE_RESULT_FILE:?missing THENCE_RESULT_FILE}"
CAPSULE_FILE="${THENCE_CAPSULE_FILE:-}"
SANDBOX_MODE="${THENCE_CODEX_SANDBOX:-workspace-write}"

TMP_DIR="$(mktemp -d "$WORK_DIR/.thence-agent.XXXXXX")"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

run_codex() {
  local schema_file="$1"
  local prompt_file="$2"
  local output_file="$3"
  codex exec \
    --cd "$WORK_DIR" \
    --skip-git-repo-check \
    --sandbox "$SANDBOX_MODE" \
    --color never \
    --output-schema "$schema_file" \
    --output-last-message "$output_file" \
    - < "$prompt_file" >/dev/null
}

schema_plan_translator() {
  cat > "$1" <<'JSON'
{
  "type": "object",
  "required": ["spl", "tasks"],
  "properties": {
    "spl": {"type": "string"},
    "tasks": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "required": ["id", "objective", "acceptance", "dependencies", "checks"],
        "properties": {
          "id": {"type": "string", "pattern": "^[A-Za-z0-9_-]+$"},
          "objective": {"type": "string"},
          "acceptance": {"type": "string"},
          "dependencies": {"type": "array", "items": {"type": "string"}},
          "checks": {"type": "array", "items": {"type": "string"}}
        },
        "additionalProperties": false
      }
    }
  },
  "additionalProperties": false
}
JSON
}

schema_implementer() {
  cat > "$1" <<'JSON'
{
  "type": "object",
  "required": ["submitted"],
  "properties": {
    "submitted": {"type": "boolean", "const": true}
  },
  "additionalProperties": false
}
JSON
}

schema_reviewer() {
  cat > "$1" <<'JSON'
{
  "type": "object",
  "required": ["approved", "findings"],
  "properties": {
    "approved": {"type": "boolean"},
    "findings": {
      "type": "array",
      "items": {"type": "string"}
    }
  },
  "additionalProperties": false
}
JSON
}

schema_checks_proposer() {
  cat > "$1" <<'JSON'
{
  "type": "object",
  "required": ["commands", "rationale"],
  "properties": {
    "commands": {
      "type": "array",
      "minItems": 1,
      "items": {"type": "string"}
    },
    "rationale": {"type": "string"}
  },
  "additionalProperties": false
}
JSON
}

build_prompt() {
  local out="$1"
  local header="$2"
  {
    echo "$header"
    echo
    if [[ -n "$PROMPT_FILE" && -f "$PROMPT_FILE" ]]; then
      echo "THENCE_PROMPT_FILE payload:"
      cat "$PROMPT_FILE"
      echo
    fi
    if [[ -n "$CAPSULE_FILE" && -f "$CAPSULE_FILE" ]]; then
      echo "THENCE_CAPSULE_FILE payload:"
      cat "$CAPSULE_FILE"
      echo
    fi
  } > "$out"
}

extract_instruction() {
  local prompt_file="$1"
  python3 - "$prompt_file" <<'PY'
import json
import sys

path = sys.argv[1]
try:
    with open(path, "r", encoding="utf-8") as f:
        payload = json.load(f)
except Exception:
    print("")
    raise SystemExit(0)

instruction = payload.get("instruction")
if isinstance(instruction, str):
    print(instruction.strip())
else:
    print("")
PY
}

main() {
  local schema_file="$TMP_DIR/schema.json"
  local prompt_file="$TMP_DIR/prompt.txt"
  local output_file="$TMP_DIR/out.json"

  case "$ROLE" in
    plan-translator)
      schema_plan_translator "$schema_file"
      build_prompt "$prompt_file" \
        "You are the thence plan-translator. Return ONLY JSON matching the schema. Produce self-contained SPL with canonical task/depends-on facts and no import directives."
      ;;
    implementer)
      schema_implementer "$schema_file"
      build_prompt "$prompt_file" \
        "You are the thence implementer. Make concrete code edits in the current workdir to satisfy objective and acceptance, then return ONLY JSON: {\"submitted\": true}."
      ;;
    reviewer)
      schema_reviewer "$schema_file"
      reviewer_instruction=""
      if [[ -n "$PROMPT_FILE" && -f "$PROMPT_FILE" ]]; then
        reviewer_instruction="$(extract_instruction "$PROMPT_FILE")"
      fi
      if [[ -z "$reviewer_instruction" ]]; then
        reviewer_instruction="Review implementation against objective/acceptance and return strict JSON with approved boolean and findings array."
      fi
      build_prompt "$prompt_file" \
        "You are the thence reviewer. $reviewer_instruction Return ONLY JSON with approved boolean and concrete findings array."
      ;;
    checks-proposer)
      schema_checks_proposer "$schema_file"
      build_prompt "$prompt_file" \
        "You are the thence checks-proposer. Return ONLY JSON with deterministic command list in commands[] and brief rationale."
      ;;
    *)
      echo "unsupported THENCE_ROLE='$ROLE'" >&2
      exit 2
      ;;
  esac

  run_codex "$schema_file" "$prompt_file" "$output_file"
  cp "$output_file" "$RESULT_FILE"
}

main "$@"
