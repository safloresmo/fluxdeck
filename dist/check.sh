#!/usr/bin/env bash
# Every check in one go: tests and lints, then security and speed against the running service.
# Run it before a commit; the service checks are skipped when the service or the phone deck is off.
set -u
cd "$(dirname "$0")/.."
fail=0
step() { echo; echo "== $1"; }

step "Pruebas"
FLUXDECK_LANG=es cargo test -q --workspace 2>&1 | grep -E "^test result|FAILED|panicked" || true
FLUXDECK_LANG=es cargo test -q --workspace >/dev/null 2>&1 || fail=1
step "Clippy"
cargo clippy -q --workspace --all-targets 2>&1 | grep -E "^error" -A6 && fail=1
echo "  listo"
for check in security speed; do
    step "$check"
    bash "dist/check-$check.sh"
    case $? in 0) ;; 2) echo "  (omitida)" ;; *) fail=1 ;; esac
done
echo
(( fail )) && echo "Algo falló." || echo "Todo bien."
exit $fail
