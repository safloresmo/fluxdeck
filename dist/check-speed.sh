#!/usr/bin/env bash
# Speed and weight of the running service: memory, CPU while idle, threads, and how fast the deck
# on the phone answers. Prints each figure against its limit; exits non-zero when one is over.
set -u
state="${XDG_STATE_HOME:-$HOME/.local/state}/fluxdeck"
pid=$(pgrep -x fluxdeck | head -1)
[[ -n "$pid" ]] || { echo "El servicio de FluxDeck no está corriendo."; exit 2; }
fail=0
check() { # name value limit unit
    if awk "BEGIN { exit !($2 <= $3) }"; then printf "  ok    %-34s %8s %s (límite %s)\n" "$1" "$2" "$4" "$3"
    else printf "  FALLA %-34s %8s %s (límite %s)\n" "$1" "$2" "$4" "$3"; fail=1; fi
}

echo "Servicio en reposo"
rss=$(awk '/VmRSS/ {printf "%.1f", $2/1024}' "/proc/$pid/status")
check "memoria" "$rss" 60 MB
threads=$(ls "/proc/$pid/task" | wc -l)
check "hilos" "$threads" 40 ""
# CPU over 20 seconds, from the kernel's tick counters.
ticks() { awk '{print $14 + $15}' "/proc/$pid/stat"; }
hz=$(getconf CLK_TCK)
t0=$(ticks); sleep 20; t1=$(ticks)
cpu=$(awk "BEGIN { printf \"%.2f\", ($t1 - $t0) / $hz / 20 * 100 }")
check "CPU en 20 s" "$cpu" 2 "%"
children=$(pgrep -P "$pid" | wc -l)
echo "  info  procesos hijos: $children ($(pgrep -P "$pid" -l | awk '{print $2}' | sort | uniq -c | awk '{printf "%s×%s ", $1, $2}'))"

if [[ -f "$state/remote.json" ]] && python3 -c "import json,sys;sys.exit(0 if json.load(open('$state/remote.json')).get('on') else 1)"; then
    echo "Deck en el celular"
    token=$(python3 -c "import json;print(json.load(open('$state/remote.json'))['phones'][0]['token'])")
    port=$(python3 -c "import json;print(json.load(open('$state/remote.json')).get('port', 27080))")
    base="http://127.0.0.1:$port"
    ms() { curl -s -o /dev/null -w '%{time_total}' "$1" | awk '{printf "%.1f", $1 * 1000}'; }
    check "página" "$(ms "$base/?k=$token")" 10 ms
    check "estado" "$(ms "$base/state?k=$token&v=0")" 10 ms
    check "imagen de una tecla" "$(ms "$base/img?k=$token&id=k0")" 10 ms
    # A tap reaches the deck and its new state comes back: a page there and back.
    v=$(curl -s "$base/state?k=$token&v=0" | python3 -c "import json,sys;print(json.load(sys.stdin)['version'])")
    page=$(curl -s "$base/state?k=$token&v=0" | python3 -c "import json,sys;print(json.load(sys.stdin)['layout']['page'])")
    pages=$(curl -s "$base/state?k=$token&v=0" | python3 -c "import json,sys;print(len(json.load(sys.stdin)['layout']['pages']))")
    if (( pages > 1 )); then
        other=$(( (page + 1) % pages ))
        start=$(date +%s%N)
        curl -s -o /dev/null -X POST "$base/input?k=$token" -d "{\"t\":\"page\",\"i\":$other}"
        curl -s -o /dev/null --max-time 5 "$base/state?k=$token&v=$v"
        took=$(( ($(date +%s%N) - start) / 1000000 ))
        curl -s -o /dev/null -X POST "$base/input?k=$token" -d "{\"t\":\"page\",\"i\":$page}"
        check "toque hasta ver el cambio" "$took" 60 ms
    fi
fi
exit $fail
