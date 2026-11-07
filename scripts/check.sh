#!/usr/bin/env bash
# Full local gate: rust (fmt/clippy/test), docs (traceability), UI build + smoke.
# Re-enters the flake dev shell when it is not active yet.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${DJ_PLATFORM_DEVSHELL:-}" ]; then
  # Plain `nix develop` uses the git tree, so the ignored target/ is not copied into the
  # nix store (a path: reference copies it and then fails on files owned by another uid).
  exec nix develop --command bash "$0" "$@"
fi

bash scripts/check-fixtures.sh
bash scripts/check-rust.sh
bash scripts/check-docs.sh
bash scripts/ui.sh --smoke
bash scripts/ui.sh --engine-check

echo "check.sh: all gates passed"
