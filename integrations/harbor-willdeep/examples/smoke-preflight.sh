#!/usr/bin/env bash
# Preflight only: import adapter + optional one-task harbor dry run args.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v harbor >/dev/null 2>&1; then
  echo "harbor CLI not found; run: uv tool install harbor" >&2
  exit 1
fi

if [[ -z "${SOMEIM_API_KEY:-}" ]]; then
  echo "Set SOMEIM_API_KEY before a real run (preflight import-only is OK)." >&2
fi

python3 -c "from willdeep_harbor.agent import WilldeepAgent; print('agent:', WilldeepAgent.name())"

echo "Example:"
echo "  export SOMEIM_API_KEY=..."
echo "  harbor run -c \"$ROOT/examples/job-willdeep-someim-glm5.yaml\""

