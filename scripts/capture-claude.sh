#!/bin/bash
# Capture only argv and protocol pipes, never the inherited environment.
set -eu
root="$(cd "$(dirname "$0")" && pwd)"
dir="${CLOD_CAPTURE_DIR:-$root/captures/manual}"
mkdir -p "$dir"
prefix="$dir/$(date -u +%Y%m%dT%H%M%S)-$$"
printf '%s\0' "$@" > "$prefix.argv"
printf '%s\n' "$prefix" >> "$dir/processes.txt"
# exec preserves the CLI's exit code; tee processes drain when its pipes close.
exec "${CLOD_CAPTURE_REAL_CLAUDE:-claude}" "$@" < <(tee "$prefix.stdin") > >(tee "$prefix.stdout") 2> >(tee "$prefix.stderr" >&2)
