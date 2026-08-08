#!/usr/bin/env bash
set -euo pipefail

TMP_ROOT="$(mktemp -d "/tmp/utomp.XXXXXX")"
TMUX_ROOT="$TMP_ROOT/tmux"
FAKE_OMP="$TMP_ROOT/omp"
VERSION_FILE="$TMP_ROOT/version"
MODE_FILE="$TMP_ROOT/mode"
COUNT_FILE="$TMP_ROOT/count"
TMUX_BIN="$(command -v tmux)"

cleanup() {
  TMUX_TMPDIR="$TMUX_ROOT" "$TMUX_BIN" kill-server 2>/dev/null || true
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

mkdir -m 700 "$TMUX_ROOT"
printf '1.0.0\n' > "$VERSION_FILE"
printf 'sleep\n' > "$MODE_FILE"
printf '0\n' > "$COUNT_FILE"
cat > "$FAKE_OMP" <<'SCRIPT'
#!/usr/bin/env bash
if [ "${1:-}" = "--version" ] || [ "${1:-}" = "version" ]; then
  printf 'omp/%s\n' "$(cat "$(dirname "$0")/version")"
  exit 0
fi
count_file="$(dirname "$0")/count"
count="$(cat "$count_file")"
printf '%s\n' "$((count + 1))" > "$count_file"
case "$(cat "$(dirname "$0")/mode")" in
  usage) exit 2 ;;
  crash) exit 1 ;;
esac
exec sleep 300
SCRIPT
chmod +x "$FAKE_OMP"

signature_hash() {
  TMUX_TMPDIR="$TMUX_ROOT" \
    TMUX_BIN="$TMUX_BIN" \
    OMP_BIN="$FAKE_OMP" \
    OMP_PROFILE="test-profile" \
    "$PWD/scripts/omp-safe" signature \
    | awk -F= '/^  hash=/{print $2}'
}

assert_attach_is_not_identity_rejection() {
  local session="$1"
  local profile="$2"
  local output rc
  set +e
  output="$(
    TMUX_TMPDIR="$TMUX_ROOT" \
      TMUX_BIN="$TMUX_BIN" \
      OMP_BIN="$FAKE_OMP" \
      OMP_PROFILE="$profile" \
      OMP_TMUX_SESSION="$session" \
      "$PWD/scripts/omp-safe" </dev/null 2>&1
  )"
  rc=$?
  set -e
  if [ "$rc" -eq 78 ] || grep -q 'different OMP' <<<"$output"; then
    printf 'expected %s to pass identity validation, got rc=%s: %s\n' "$session" "$rc" "$output" >&2
    exit 1
  fi
}

before_hash="$(signature_hash)"
printf '2.0.0\n' > "$VERSION_FILE"
after_hash="$(signature_hash)"
test "$before_hash" = "$after_hash"

TMUX_TMPDIR="$TMUX_ROOT" "$TMUX_BIN" new-session -d -s legacy-version-test 'sleep 300'
TMUX_TMPDIR="$TMUX_ROOT" "$TMUX_BIN" set-option -q -t legacy-version-test @omp-profile test-profile
TMUX_TMPDIR="$TMUX_ROOT" "$TMUX_BIN" set-option -q -t legacy-version-test @omp-safe-signature old-version-sensitive-hash
assert_attach_is_not_identity_rejection legacy-version-test test-profile
test "$(TMUX_TMPDIR="$TMUX_ROOT" "$TMUX_BIN" show-options -qv -t legacy-version-test @omp-safe-identity)" = "$after_hash"

printf '3.0.0\n' > "$VERSION_FILE"
assert_attach_is_not_identity_rejection legacy-version-test test-profile
test "$(signature_hash)" = "$after_hash"

set +e
TMUX_TMPDIR="$TMUX_ROOT" TMUX_BIN="$TMUX_BIN" OMP_BIN="$FAKE_OMP" \
  OMP_PROFILE=other-profile OMP_TMUX_SESSION=legacy-version-test \
  "$PWD/scripts/omp-safe" </dev/null >/dev/null 2>&1
profile_rc=$?
set -e
test "$profile_rc" -eq 78

TMUX_TMPDIR="$TMUX_ROOT" "$TMUX_BIN" new-session -d -s foreign-session 'sleep 300'
set +e
TMUX_TMPDIR="$TMUX_ROOT" TMUX_BIN="$TMUX_BIN" OMP_BIN="$FAKE_OMP" \
  OMP_TMUX_SESSION=foreign-session "$PWD/scripts/omp-safe" </dev/null >/dev/null 2>&1
foreign_rc=$?
set -e
test "$foreign_rc" -eq 78

printf 'usage\n' > "$MODE_FILE"
printf '0\n' > "$COUNT_FILE"
set +e
usage_output="$(
  TMUX_TMPDIR="$TMUX_ROOT" TMUX_BIN="$TMUX_BIN" OMP_BIN="$FAKE_OMP" \
    OMP_MAX_RESTARTS=5 OMP_RESTART_DELAY_SECONDS=0 \
    "$PWD/scripts/omp-safe" __worker </dev/null 2>&1
)"
usage_rc=$?
set -e
if [ "$usage_rc" -ne 2 ]; then
  printf 'expected permanent usage failure rc=2, got rc=%s: %s\n' "$usage_rc" "$usage_output" >&2
  exit 1
fi
test "$(cat "$COUNT_FILE")" -eq 1

printf 'crash\n' > "$MODE_FILE"
printf '0\n' > "$COUNT_FILE"
set +e
crash_output="$(
  TMUX_TMPDIR="$TMUX_ROOT" TMUX_BIN="$TMUX_BIN" OMP_BIN="$FAKE_OMP" \
    OMP_MAX_RESTARTS=2 OMP_RESTART_DELAY_SECONDS=0 \
    "$PWD/scripts/omp-safe" __worker </dev/null 2>&1
)"
crash_rc=$?
set -e
if [ "$crash_rc" -ne 1 ]; then
  printf 'expected retry exhaustion rc=1, got rc=%s: %s\n' "$crash_rc" "$crash_output" >&2
  exit 1
fi
test "$(cat "$COUNT_FILE")" -eq 3

printf 'omp-safe version-agnostic identity tests passed\n'
