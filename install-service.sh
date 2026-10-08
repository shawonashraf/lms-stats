#!/bin/sh
# Installs lms-stats as a macOS LaunchAgent (the equivalent of
# lms-stats.service on Linux and install-service.ps1 on Windows). Runs as
# the current user, starts at login, restarts on crash.
#
#   ./install-service.sh
#
# Settings, all optional, as environment variables:
#   LMS_UPSTREAM    "http://192.168.0.166:1234,http://192.168.0.163:1234"
#   LMS_LISTEN      "0.0.0.0:1235"
#   LMS_DB          "$HOME/Library/Application Support/lms-stats/lms-stats.db"
#   LMS_BACKUP_DIR  "$HOME/Documents/lms-stats"  (set to "" for no backups)
set -eu

LABEL="com.shawonashraf.lms-stats"
DIR="$(cd "$(dirname "$0")" && pwd)"
EXE="$DIR/target/release/lms-stats"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG="$HOME/Library/Logs/lms-stats.log"

UPSTREAM="${LMS_UPSTREAM:-http://192.168.0.166:1234,http://192.168.0.163:1234}"
LISTEN="${LMS_LISTEN:-0.0.0.0:1235}"
DB="${LMS_DB:-$HOME/Library/Application Support/lms-stats/lms-stats.db}"
BACKUP_DIR="${LMS_BACKUP_DIR-$HOME/Documents/lms-stats}"

if [ ! -x "$EXE" ]; then
    echo "Release binary not found at $EXE" >&2
    echo "Build it first:  cargo build --release" >&2
    exit 1
fi

mkdir -p "$(dirname "$DB")" "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"

# Replace a running instance, if any.
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true

cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>$EXE</string>
    </array>
    <key>WorkingDirectory</key>
    <string>$DIR</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>LMS_UPSTREAM</key>
        <string>$UPSTREAM</string>
        <key>LMS_LISTEN</key>
        <string>$LISTEN</string>
        <key>LMS_DB</key>
        <string>$DB</string>
        <key>LMS_BACKUP_DIR</key>
        <string>$BACKUP_DIR</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>3</integer>
    <key>StandardOutPath</key>
    <string>$LOG</string>
    <key>StandardErrorPath</key>
    <string>$LOG</string>
</dict>
</plist>
PLIST

plutil -lint -s "$PLIST"
launchctl bootstrap "gui/$(id -u)" "$PLIST"

echo "LaunchAgent $LABEL is running."
echo "  Upstream : $UPSTREAM"
echo "  Listening: $LISTEN"
echo "  Database : $DB"
echo "  Backups  : ${BACKUP_DIR:-disabled}"
echo "  Log      : $LOG"
echo "  Dashboard: http://localhost:${LISTEN##*:}/dashboard"
