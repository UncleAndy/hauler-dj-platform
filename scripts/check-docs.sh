#!/usr/bin/env bash
# Documentation gate: every DJP-* requirement of the specification must have a row
# in docs/requirements/traceability.csv with an owner. Needs no toolchain.
set -euo pipefail
cd "$(dirname "$0")/.."

tz="DJ_Platform_TZ_v0.1_RU.md"
csv="docs/requirements/traceability.csv"

status=0
while read -r req; do
  case "$req" in
    DJP-SRS-*|DJP-TASK-*) continue ;; # document and template identifiers
  esac
  if ! grep -q "^${req}," "$csv"; then
    echo "traceability: ${req} from ${tz} has no row in ${csv}" >&2
    status=1
    continue
  fi
  owner="$(grep "^${req}," "$csv" | head -1 | cut -d, -f2)"
  if [ -z "$owner" ]; then
    echo "traceability: ${req} has no owner" >&2
    status=1
  fi
done < <(grep -o 'DJP-[A-Z]*-[0-9]*' "$tz" | sort -u)

# The CSV must stay parseable: a comma inside an unquoted field silently shifts every later
# column, and a status of "passing" then lands on the wrong requirement.
bad_rows=0
while IFS= read -r line; do
  fields="$(printf '%s' "$line" | awk -F, '{print NF}')"
  if [ "$fields" -ne 11 ]; then
    echo "traceability: row has ${fields} fields instead of 11: ${line:0:60}..." >&2
    bad_rows=$((bad_rows + 1))
  fi
done < <(tail -n +2 "$csv")

if [ "$bad_rows" -ne 0 ]; then
  status=1
fi

# A row may only claim a state that another person can check: `passing` needs a test or a command,
# `review` and `passing` need an evidence file that exists today.
while IFS=, read -r id owner priority risk source what how state evidence _rest; do
  [ "$id" = "req_id" ] && continue
  case "$state" in
    passing|review|unverified|planned) ;;
    *)
      echo "traceability: ${id} has status '${state}', which is not passing/review/unverified/planned" >&2
      status=1
      continue
      ;;
  esac
  if [ "$state" = "passing" ] && [ -z "$how" ]; then
    echo "traceability: ${id} is passing but names no test or command" >&2
    status=1
  fi
  if [ "$state" = "passing" ] || [ "$state" = "review" ]; then
    [ -z "$evidence" ] && continue
    first="${evidence%%;*}"
    if [ ! -e "$first" ]; then
      echo "traceability: ${id} points at evidence that does not exist: ${first}" >&2
      status=1
    fi
  fi
done < "$csv"

if [ "$status" -eq 0 ]; then
  echo "traceability: every DJP-* requirement has an owned row, every row has 11 fields"
fi
exit "$status"
