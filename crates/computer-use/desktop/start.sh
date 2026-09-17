#!/bin/sh
set -eu

export HOME="${HOME:-/home/desktop}"
export USER="${USER:-desktop}"
export DISPLAY="${DISPLAY:-:0}"
export LANG="${LANG:-C.UTF-8}"
export XDG_RUNTIME_DIR="/run/user/1000"

mkdir -p "$HOME/.fluxbox" "$HOME/.config" /tmp/.X11-unix "$XDG_RUNTIME_DIR"
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

wait "$XPID"
