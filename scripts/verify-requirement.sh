#!/usr/bin/env bash
# Отметить требование проверенным: обёртка, чтобы не помнить, где лежит python3.
#
#   bash scripts/verify-requirement.sh --list
#   bash scripts/verify-requirement.sh DJP-MIX-006 "Андрей Великоредчанин"
set -euo pipefail
cd "$(dirname "$0")/.."
if command -v python3 >/dev/null 2>&1; then
    exec python3 scripts/verify-requirement.py "$@"
fi
exec nix shell nixpkgs#python3 --command python3 scripts/verify-requirement.py "$@"
