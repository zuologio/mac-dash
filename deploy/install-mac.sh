#!/usr/bin/env bash
# Install the agent as a launchd user agent, so it survives logout and reboot.
set -euo pipefail

cd "$(dirname "$0")/.."

PLIST="$HOME/Library/LaunchAgents/com.macdash.agent.plist"
BIN="$HOME/.local/bin/macdash-agent"

echo "Building the agent…"
cargo build --release -p macdash-agent

mkdir -p "$(dirname "$BIN")" "$(dirname "$PLIST")"
# launchctl refuses to replace a running binary in place.
launchctl unload "$PLIST" 2>/dev/null || true
install -m 755 target/release/macdash-agent "$BIN"

cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.macdash.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>${BIN}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/macdash-agent.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/macdash-agent.log</string>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
PLIST_EOF

launchctl load "$PLIST"
sleep 1

if pgrep -x macdash-agent >/dev/null; then
    echo "Agent running (pid $(pgrep -x macdash-agent))."
else
    echo "Agent did not start — check /tmp/macdash-agent.log"
    exit 1
fi

echo
echo "  logs:      tail -f /tmp/macdash-agent.log"
echo "  uninstall: launchctl unload $PLIST && rm $PLIST"
