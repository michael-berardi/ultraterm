#!/usr/bin/env bash
# Age out disposable npx package sandboxes without touching npm's content cache.
# Active processes, working directories, open files, recent entries, symlinks,
# and foreign-owned entries are always preserved.
set -u

RETENTION_DAYS="${NPX_CACHE_RETENTION_DAYS:-7}"
MIN_RECLAIM_MB="${NPX_CACHE_MIN_RECLAIM_MB:-200}"
DRYRUN="${NPX_CACHE_DRYRUN:-0}"
CACHE_ROOT="${HOME}/.npm/_npx"
LOGFILE="${HOME}/Library/Logs/clean-stale-npx-cache.log"
mkdir -p "$(dirname "$LOGFILE")"

log() { printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$*" >> "$LOGFILE"; }

[[ -d "$CACHE_ROOT" ]] || { log "missing cache root: $CACHE_ROOT"; exit 0; }

cwd_snapshot="$(mktemp)"
command_snapshot="$(mktemp)"
trap 'rm -f "$cwd_snapshot" "$command_snapshot"' EXIT
lsof -n -d cwd -Fn > "$cwd_snapshot" 2>/dev/null || true
ps -axo command= > "$command_snapshot" 2>/dev/null || true

now="$(date +%s)"
retention_seconds=$(( RETENTION_DAYS * 86400 ))
declare -a candidates=()
total_kb=0

for entry in "$CACHE_ROOT"/*; do
  [[ -d "$entry" && ! -L "$entry" ]] || continue
  owner_uid="$(stat -f '%u' "$entry" 2>/dev/null || echo -1)"
  [[ "$owner_uid" == "$(id -u)" ]] || { log "skip foreign owner: $entry"; continue; }
  modified="$(stat -f '%m' "$entry" 2>/dev/null || echo "$now")"
  (( now - modified > retention_seconds )) || continue

  if grep -Fqx "n${entry}" "$cwd_snapshot" ||
      grep -Fq "n${entry}/" "$cwd_snapshot" ||
      grep -Fq "$entry" "$command_snapshot" ||
      lsof -n +D "$entry" >/dev/null 2>&1; then
    log "skip in-use: $entry"
    continue
  fi

  size_kb="$(du -sk "$entry" 2>/dev/null | awk '{print $1}')"
  candidates+=("$entry")
  total_kb=$(( total_kb + ${size_kb:-0} ))
done

total_mb=$(( total_kb / 1024 ))
if [[ "${#candidates[@]}" -eq 0 ]]; then
  log "no inactive npx sandboxes older than ${RETENTION_DAYS}d"
  exit 0
fi
if [[ "$total_mb" -lt "$MIN_RECLAIM_MB" ]]; then
  log "below threshold (${total_mb}MB < ${MIN_RECLAIM_MB}MB), skipping"
  exit 0
fi

removed=0
freed_kb=0
for entry in "${candidates[@]}"; do
  [[ -d "$entry" && ! -L "$entry" ]] || continue
  modified="$(stat -f '%m' "$entry" 2>/dev/null || echo "$now")"
  if (( now - modified <= retention_seconds )) ||
      grep -Fq "$entry" "$command_snapshot" ||
      lsof -n +D "$entry" >/dev/null 2>&1; then
    log "skip refreshed or active: $entry"
    continue
  fi

  size_kb="$(du -sk "$entry" 2>/dev/null | awk '{print $1}')"
  if [[ "$DRYRUN" == "1" ]]; then
    log "WOULD remove inactive npx sandbox: $entry (${size_kb:-0}KB)"
  elif rm -rf -- "$entry"; then
    removed=$(( removed + 1 ))
    freed_kb=$(( freed_kb + ${size_kb:-0} ))
    log "removed inactive npx sandbox: $entry (${size_kb:-0}KB)"
  else
    log "FAILED to remove npx sandbox: $entry"
  fi
done

log "done: removed=$removed freed=$(( freed_kb / 1024 ))MB"
