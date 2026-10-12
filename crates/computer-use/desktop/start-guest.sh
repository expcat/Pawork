#!/bin/sh
set -eu

export HOME="${HOME:-/home/desktop}"
export USER="${USER:-desktop}"
export DISPLAY="${DISPLAY:-:0}"
export LANG="${LANG:-C.UTF-8}"
export XDG_RUNTIME_DIR="/run/user/1000"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
export NO_AT_BRIDGE=0
export GTK_A11Y=1
export MOZ_DISABLE_CONTENT_SANDBOX=1

if [ -z "${PAWORK_ISOLATED_GUEST_TOKEN:-}" ]; then
  echo "PAWORK_ISOLATED_GUEST_TOKEN is missing" >&2
  exit 1
fi

mkdir -p "$HOME/.fluxbox" "$HOME/profile" "$XDG_RUNTIME_DIR" /tmp/.X11-unix
chmod 700 "$XDG_RUNTIME_DIR"
rm -f /tmp/.X0-lock /tmp/.X11-unix/X0

cat > "$HOME/.fluxbox/menu" <<'EOF'
[begin] (Pawork-Isolated)
  [exec] (Mousepad) {mousepad}
  [exec] (Firefox) {firefox}
  [restart] (Restart)
  [exit] (Exit)
[end]
EOF

cat > "$HOME/profile/user.js" <<'EOF'
user_pref("accessibility.force_disabled", -1);
user_pref("accessibility.ipc_architecture", 0);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.startup.firstrunSkipsHomepage", false);
user_pref("browser.startup.homepage", "file:///usr/local/share/pawork/cu21.html");
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.startup.page", 1);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("toolkit.telemetry.enabled", false);
user_pref("security.sandbox.content.level", 0);
user_pref("browser.sessionstore.resume_from_crash", false);
EOF

Xvnc :0 \
  -geometry 1280x800 \
  -depth 24 \
  -rfbport 5900 \
  -ac \
  -nolisten tcp \
  -pn \
  -noclipboard \
  -SecurityTypes None \
  -NeverShared \
  -DisconnectClients=0 \
  -desktop Pawork-Isolated \
  -AcceptCutText=0 \
  -SendCutText=0 \
  -verbose \
  &
XPID=$!

cleanup() {
  kill -TERM "$XPID" 2>/dev/null || true
  wait "$XPID" 2>/dev/null || true
}
trap cleanup TERM INT

i=0
while [ ! -S /tmp/.X11-unix/X0 ]; do
  if ! kill -0 "$XPID" 2>/dev/null; then
    echo "Xvnc exited before the display socket was ready" >&2
    wait "$XPID" || true
    exit 1
  fi
  i=$((i + 1))
  if [ "$i" -gt 50 ]; then
    echo "timed out waiting for Xvnc display socket" >&2
    cleanup
    exit 1
  fi
  sleep 0.1
done
echo "Xvnc ready on :0 (RFB 5900), desktop=Pawork-Isolated"

fluxbox &
mousepad &

dbus-daemon --session --address="$DBUS_SESSION_BUS_ADDRESS" --nofork --nopidfile &
i=0
while [ ! -S "$XDG_RUNTIME_DIR/bus" ]; do
  i=$((i + 1))
  if [ "$i" -gt 50 ]; then
    echo "timed out waiting for the session bus" >&2
    cleanup
    exit 1
  fi
  sleep 0.1
done

launcher=""
for candidate in \
  /usr/libexec/at-spi-bus-launcher \
  /usr/lib/at-spi2-core/at-spi-bus-launcher \
  /usr/libexec/at-spi2/at-spi-bus-launcher
do
  if [ -x "$candidate" ]; then
    launcher=$candidate
    break
  fi
done
if [ -z "$launcher" ]; then
  echo "at-spi bus launcher is not installed" >&2
  find /usr -name 'at-spi*' >&2 || true
  cleanup
  exit 1
fi
echo "starting $launcher"
"$launcher" --launch-immediately &

firefox --no-remote --profile "$HOME/profile" "file:///usr/local/share/pawork/cu21.html" &
computer_guest &

wait "$XPID"
