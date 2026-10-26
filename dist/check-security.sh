#!/usr/bin/env bash
# Security checks against the running service. Needs the deck on the phone switched on.
# Checks: keys are required, the server shrugs off oversized requests and idle floods without a
# key, and the files holding keys are readable only by their owner. Exits non-zero on a failure.
set -u
state="${XDG_STATE_HOME:-$HOME/.local/state}/fluxdeck"
config="${XDG_CONFIG_HOME:-$HOME/.config}/fluxdeck"
fail=0
ok() { echo "  ok    $1"; }
bad() { echo "  FALLA $1"; fail=1; }

if [[ ! -f "$state/remote.json" ]] || ! python3 -c "import json,sys;sys.exit(0 if json.load(open('$state/remote.json')).get('on') else 1)"; then
    echo "El deck en el celular está apagado: enciéndelo en Ajustes → Extensiones para probar el servidor."
    exit 2
fi
token=$(python3 -c "import json;print(json.load(open('$state/remote.json'))['phones'][0]['token'])")
port=$(python3 -c "import json;print(json.load(open('$state/remote.json')).get('port', 27080))")
host=127.0.0.1
base="http://$host:$port"
pid=$(pgrep -x fluxdeck | head -1)
rss() { awk '/VmRSS/ {print $2}' "/proc/$pid/status"; }

echo "Claves"
[[ $(curl -s -o /dev/null -w '%{http_code}' "$base/") == 403 ]] && ok "sin clave: rechazado" || bad "sin clave no se rechaza"
[[ $(curl -s -o /dev/null -w '%{http_code}' "$base/?k=${token:0:31}x") == 403 ]] && ok "clave casi igual: rechazada" || bad "una clave casi igual entra"
[[ $(curl -s -o /dev/null -w '%{http_code}' -X POST "$base/input?k=nope" -d '{"t":"page","i":0}') == 403 ]] && ok "toque sin clave: rechazado" || bad "un toque sin clave entra"
[[ $(curl -s -o /dev/null -w '%{http_code}' "$base/?k=$token") == 200 ]] && ok "con la clave: entra" || bad "con la clave no entra"

echo "Peticiones abusivas sin clave"
before=$(rss)
python3 - "$host" "$port" <<'EOF'
import socket, sys
s = socket.create_connection((sys.argv[1], int(sys.argv[2])), timeout=5)
try:
    chunk = b"A" * 65536
    for _ in range(320):  # 20 MB with no line end
        s.sendall(chunk)
except OSError:
    pass  # the server hanging up early is the point
s.close()
EOF
sleep 1
after=$(rss)
grow=$(( (after - before) / 1024 ))
(( grow < 8 )) && ok "línea de 20 MB: la memoria creció $grow MB" || bad "línea de 20 MB: la memoria creció $grow MB"

python3 - "$host" "$port" "$token" "$pid" <<'EOF'
import os, socket, sys, time, urllib.request
host, port, token, pid = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
threads = lambda: len(os.listdir(f"/proc/{pid}/task"))
base = threads()
idle = []
for _ in range(300):
    try:
        idle.append(socket.create_connection((host, port), timeout=2))
    except OSError:
        break
time.sleep(0.5)
peak = threads() - base
for s in idle:
    s.close()
time.sleep(0.5)
t = time.time()
try:
    code = urllib.request.urlopen(f"http://{host}:{port}/?k={token}", timeout=5).status
except Exception as e:
    code = str(e)
took = time.time() - t
good = peak <= 70 and code == 200 and took < 1
status = "ok   " if good else "FALLA"
print(f"  {status} {len(idle)} conexiones vacías: {peak} hilos más como máximo; al irse, una petición con clave respondió {code} en {took:.2f} s")
sys.exit(0 if good else 1)
EOF
[[ $? == 0 ]] || fail=1

echo "Archivos con claves"
for f in "$state/remote.json" "$config/plugin-settings.json"; do
    [[ -f "$f" ]] || continue
    mode=$(stat -c '%a' "$f")
    [[ $mode == 600 ]] && ok "$(basename "$f") solo lo lee su dueño ($mode)" || bad "$(basename "$f") lo puede leer otro usuario ($mode)"
done

echo "Puertos"
ss -ltnp 2>/dev/null | awk '/"fluxdeck"/ {print $4}' | while read -r addr; do
    case "$addr" in
        127.0.0.1:*) echo "  ok    $addr solo dentro de esta PC" ;;
        *:"$port") echo "  ok    $addr: el deck en el celular (pide clave)" ;;
        *) echo "  FALLA $addr abierto a la red"; exit 1 ;;
    esac
done || fail=1

exit $fail
