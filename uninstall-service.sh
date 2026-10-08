#!/bin/sh
# Stops and removes the lms-stats LaunchAgent. The database under
# ~/Library/Application Support/lms-stats and the log are left in place.
#
#   ./uninstall-service.sh
set -u

LABEL="com.shawonashraf.lms-stats"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"

if [ ! -f "$PLIST" ]; then
    echo "LaunchAgent $LABEL is not installed."
    exit 0
fi

launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
rm -f "$PLIST"
echo "LaunchAgent $LABEL removed."
echo "Database and log kept under ~/Library/Application Support/lms-stats and ~/Library/Logs."
