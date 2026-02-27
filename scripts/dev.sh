#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if [[ -f ".env" ]]; then
  set -a
  # shellcheck source=/dev/null
  source ".env"
  set +a
fi

# Runtime needs OPENAI_API_KEY. Reuse DARLING_LLM_API_KEY if user only configured that.
if [[ -z "${OPENAI_API_KEY:-}" && -n "${DARLING_LLM_API_KEY:-}" ]]; then
  export OPENAI_API_KEY="${DARLING_LLM_API_KEY}"
fi

export DARLING_RUNTIME_PORT="${DARLING_RUNTIME_PORT:-3999}"
export DARLING_RUNTIME_URL="${DARLING_RUNTIME_URL:-http://127.0.0.1:${DARLING_RUNTIME_PORT}/run}"

python3 - <<'PY'
import importlib
mods = ["fastapi", "uvicorn", "pydantic", "agents"]
missing = [m for m in mods if importlib.util.find_spec(m) is None]
if missing:
    raise SystemExit("Missing Python packages: " + ", ".join(missing))
PY

cleanup() {
  if [[ -n "${RUNTIME_PID:-}" ]] && kill -0 "${RUNTIME_PID}" 2>/dev/null; then
    kill "${RUNTIME_PID}" 2>/dev/null || true
    wait "${RUNTIME_PID}" 2>/dev/null || true
  fi
}

trap cleanup EXIT INT TERM

echo "[darling] starting runtime on :${DARLING_RUNTIME_PORT}"
python3 runtime/app.py &
RUNTIME_PID=$!

sleep 1
if ! kill -0 "${RUNTIME_PID}" 2>/dev/null; then
  echo "[darling] runtime failed to start"
  exit 1
fi

echo "[darling] starting tauri dev"
npm run tauri dev -- "$@"
