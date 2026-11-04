#!/usr/bin/env bash
# Full local gate: rust (fmt/clippy/test), docs (traceability), UI build + smoke.
# Re-enters the flake dev shell when it is not active yet.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${DJ_PLATFORM_DEVSHELL:-}" ]; then
  exec nix develop "path:$PWD" --command bash "$0" "$@"
fi

bash scripts/check-rust.sh
bash scripts/check-docs.sh
bash scripts/ui.sh --smoke

echo "check.sh: all gates passed"
