#!/usr/bin/env bash
# Install UltraTerm's safe daily cleanup as a per-user macOS LaunchAgent.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LABEL="com.libertydesignstudio.ultraterm-maintenance"
DUE_HOUR="${ULTRATERM_MAINTENANCE_HOUR:-22}"
if ! [[ "$DUE_HOUR" =~ ^([01]?[0-9]|2[0-3])$ ]]; then
  printf 'Invalid ULTRATERM_MAINTENANCE_HOUR: %s\n' "$DUE_HOUR" >&2
  exit 2
fi
DUE_HOUR=$((10#$DUE_HOUR))
BIN_DIR="${HOME}/bin"
PLIST_DIR="${HOME}/Library/LaunchAgents"
PLIST_PATH="${PLIST_DIR}/${LABEL}.plist"
LOG_DIR="${HOME}/Library/Logs"
mkdir -p "$BIN_DIR" "$PLIST_DIR" "$LOG_DIR"
install -m 0755 "${SCRIPT_DIR}/ultraterm-maintain" "${BIN_DIR}/ultraterm-maintain"
install -m 0755 "${SCRIPT_DIR}/clean-stale-npx-cache.sh" "${BIN_DIR}/clean-stale-npx-cache.sh"

tmp_plist="$(mktemp "${PLIST_DIR}/${LABEL}.XXXXXX")"
trap 'rm -f "$tmp_plist"' EXIT
cat > "$tmp_plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${HOME}/bin/ultraterm-maintain</string>
    <string>--if-due</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>StartCalendarInterval</key>
  <dict>
    <key>Hour</key>
    <integer>${DUE_HOUR}</integer>
    <key>Minute</key>
    <integer>15</integer>
  </dict>
  <key>EnvironmentVariables</key>
  <dict>
    <key>ULTRATERM_MAINTENANCE_HOUR</key>
    <string>${DUE_HOUR}</string>
  </dict>
  <key>ProcessType</key>
  <string>Background</string>
  <key>LowPriorityIO</key>
  <true/>
  <key>StandardOutPath</key>
  <string>${LOG_DIR}/ultraterm-maintenance.log</string>
  <key>StandardErrorPath</key>
  <string>${LOG_DIR}/ultraterm-maintenance.log</string>
</dict>
</plist>
PLIST

plutil -lint "$tmp_plist" >/dev/null
mv "$tmp_plist" "$PLIST_PATH"
launchctl bootout "gui/${UID}/${LABEL}" 2>/dev/null || true
launchctl bootstrap "gui/${UID}" "$PLIST_PATH"
launchctl enable "gui/${UID}/${LABEL}"
printf 'Installed %s (daily at %02d:15 local time)\n' "$LABEL" "$DUE_HOUR"
