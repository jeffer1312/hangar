#!/usr/bin/env bash
# vigia.sh -e: a lista segue `orq ball` e os avisos ao árbitro passam por `orq notify`.
# Sem backend: curl e hangar-send são falsos.
set -euo pipefail
raiz="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin" "$t/orq"
cat > "$t/bin/curl" <<'SH'
#!/bin/sh
for a; do url=$a; done
case "$url" in
  */api/sessions) printf '[{"name":"exec1","state":"idle"},{"name":"rev1","state":"idle"},{"name":"arb","state":"idle"}]' ;;
  *) exit 22 ;;
esac
SH
cat > "$t/bin/hangar-send" <<SH
#!/bin/sh
printf '%s\n' "\$*" >> "$t/sent.log"
SH
chmod +x "$t/bin/curl" "$t/bin/hangar-send"
printf 'CP_AUTH_TOKEN=x\n' > "$t/env"
export ORQ_DIR="$t/orq" ORQ_SEND="$t/bin/hangar-send" ORQ_JEV=off
orq() { python3 "$raiz/skills/orquestrar/scripts/orq.py" "$@" >/dev/null; }
orq init --arbiter arb --repo "$raiz" --contract "$t/regras.md"
orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
orq event entrega --task 1 --rodada 1 --commit abc
PATH="$t/bin:$PATH" CP_ENV="$t/env" CP_VIGIA_INTERVALO=0 CP_VIGIA_CICLOS=3 CP_VIGIA_LOG="$t/err" \
  bash "$raiz/skills/orquestrar/scripts/vigia.sh" arb -e "$t/orq" -m 1 > "$t/out"
fail() { echo "FAIL: $1"; echo "--- out"; cat "$t/out"; echo "--- sent"; cat "$t/sent.log"; exit 1; }
grep -q "watching: rev1 arb" "$t/out" || fail "a lista não seguiu a vez"
grep -q "^--tmux rev1 " "$t/sent.log" || fail "rev1 não foi cutucado"
if grep -q "exec1" "$t/sent.log"; then fail "exec1 espera como mandado e foi cutucado"; fi
grep -q "^--tmux arb \[vigia\] rev1 is stopped" "$t/sent.log" || fail "o árbitro não foi avisado pelo orq"
# Sem `orq init`, todo alarme via orq cairia só no log: o vigia recusa armar.
mkdir -p "$t/vazio"; antes=$(wc -l < "$t/sent.log")
if PATH="$t/bin:$PATH" CP_ENV="$t/env" CP_VIGIA_INTERVALO=0 CP_VIGIA_CICLOS=1 CP_VIGIA_LOG="$t/err" \
  timeout 10 bash "$raiz/skills/orquestrar/scripts/vigia.sh" arb -e "$t/vazio" > "$t/out2" 2> "$t/err2"; then
  fail "armou sem orq init"
fi
[ "$(wc -l < "$t/sent.log")" -eq "$antes" ] || fail "mandou recado sem orq init"
grep -q "orq.json" "$t/err2" || fail "não disse por que não armou"
echo ok
