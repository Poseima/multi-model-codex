#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLI="$SCRIPT_DIR/dawn_im.py"
VALIDATOR="$SCRIPT_DIR/validate.py"
CONTROL_API_CLIENT="$SCRIPT_DIR/control_api_client.py"
CONTROL_API_CALLER="$SCRIPT_DIR/call_control_api.mjs"

python3 -m py_compile "$CLI" "$VALIDATOR" "$CONTROL_API_CLIENT"
node --check "$CONTROL_API_CALLER"
"$CLI" --help >/dev/null
"$CLI" im-action --help >/dev/null

echo "smoke_ok"
