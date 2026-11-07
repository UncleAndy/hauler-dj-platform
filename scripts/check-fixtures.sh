#!/usr/bin/env bash
# Golden audio fixtures: the bytes the tests describe must be the bytes in the repository.
#
# The acceptance of A11 is blunt — CI fails when an offline golden fixture changes unexpectedly. The
# manifest is a plain `sha256sum` file, so changing a fixture means editing one line with a reason in
# the commit message, not silently re-recording whatever the current build produced.
set -euo pipefail
cd "$(dirname "$0")/.."

manifest="crates/dj-codec/tests/fixtures/audio.sha256"
directory="$(dirname "$manifest")/audio"

if [ ! -f "$manifest" ]; then
  echo "fixtures: ${manifest} is missing" >&2
  exit 1
fi

status=0

if ! LC_ALL=C sha256sum --check --strict --quiet "$manifest" >/dev/null 2>&1; then
  echo "fixtures: a golden file changed or is missing:" >&2
  LC_ALL=C sha256sum --check --strict --quiet "$manifest" 2>&1 | sed 's/^/  /' >&2 || true
  echo "fixtures: if the change is intended, refresh the manifest and say why in the commit:" >&2
  echo "  sha256sum $(printf '%s ' "$directory"/*) | sort -k2 > $manifest" >&2
  status=1
fi

# A new file that nobody listed is the same defect in the other direction: the gate would not see it.
listed="$(awk '{print $2}' "$manifest" | sed "s|^${directory}/||" | sort)"
present="$(find "$directory" -maxdepth 1 -type f -printf '%f\n' | sort)"
unlisted="$(comm -13 <(printf '%s\n' "$listed") <(printf '%s\n' "$present"))"
missing="$(comm -23 <(printf '%s\n' "$listed") <(printf '%s\n' "$present"))"
if [ -n "$unlisted" ]; then
  echo "fixtures: these files are not in the manifest: ${unlisted}" >&2
  status=1
fi
if [ -n "$missing" ]; then
  echo "fixtures: the manifest names files that are not there: ${missing}" >&2
  status=1
fi

if [ "$status" -eq 0 ]; then
  count="$(wc -l < "$manifest")"
  echo "fixtures: ${count} golden files match the manifest"
fi
exit "$status"
