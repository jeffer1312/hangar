#!/usr/bin/env bash
# vigia.sh -e: a lista segue `orq ball` e os avisos ao árbitro passam por `orq notify`.
# Sem backend: curl e hangar-send são falsos. O envio do orq tem fake próprio (linha `orq: …`),
# para que um alarme que saia por hangar-send direto não passe por um que saiu pelo orq.
set -euo pipefail
raiz="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin"
# /api/sessions responde $t/sessions.json; $t/on-sessions, se existir, roda uma vez antes.
cat > "$t/bin/curl" <<SH
#!/bin/sh
for a; do url=\$a; done
printf '%s\n' "\$*" >> "$t/urls"
case "\$url" in
  */api/sessions)
    if [ -f "$t/on-sessions" ]; then bash "$t/on-sessions"; rm -f "$t/on-sessions"; fi
    cat "$t/sessions.json" ;;
  *) exit 22 ;;
esac
SH
cat > "$t/bin/hangar-send" <<SH
#!/bin/sh
printf '%s\n' "\$*" >> "$t/sent.log"
SH
cat > "$t/bin/orq-send" <<SH
#!/bin/sh
printf 'orq: %s\n' "\$*" >> "$t/sent.log"
SH
chmod +x "$t/bin/curl" "$t/bin/hangar-send" "$t/bin/orq-send"
printf 'CP_AUTH_TOKEN=x\n' > "$t/env"
export ORQ_SEND="$t/bin/orq-send" ORQ_JEV=off
ORQPY="$raiz/skills/orquestrar/scripts/orq.py"
orq() { python3 "$ORQPY" --dir "$d" "$@" >/dev/null; }
novo() { d="$t/$1"; mkdir -p "$d"; : > "$t/sent.log"; : > "$t/urls"; orq init --arbiter arb --repo "$raiz" --contract "$t/regras.md"; }
vigia() {
  PATH="$t/bin:$PATH" CP_ENV="$t/env" CP_VIGIA_INTERVALO=0 CP_VIGIA_CICLOS=3 CP_VIGIA_LOG="$t/err" \
    timeout 10 bash "$raiz/skills/orquestrar/scripts/vigia.sh" arb -e "$d" -m 1 > "$t/out"
}
fail() { echo "FAIL: $1"; echo "--- out"; cat "$t/out"; echo "--- sent"; cat "$t/sent.log"; exit 1; }

# A vez é do revisor: só ele é vigiado e cutucado; o árbitro é avisado pelo orq.
printf '%s' '[{"name":"exec1","state":"idle"},{"name":"rev1","state":"idle"},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo vez
orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
orq event entrega --task 1 --rodada 1 --commit abc
vigia
grep -q "watching: rev1 arb" "$t/out" || fail "a lista não seguiu a vez"
grep -q "^--tmux rev1 .*orq notify '\[decisao\]" "$t/sent.log" || fail "rev1 não foi cutucado com o caminho do orq notify"
if grep -q "exec1" "$t/sent.log"; then fail "exec1 espera como mandado e foi cutucado"; fi
grep -q "^orq: --tmux arb \[vigia\] ARMED" "$t/sent.log" || fail "a prova de armação não passou pelo orq"
grep -q "^orq: --tmux arb \[vigia\] rev1 is stopped" "$t/sent.log" || fail "o árbitro não foi avisado pelo orq"
if grep -q "^--tmux arb " "$t/sent.log"; then fail "recado ao árbitro saiu por fora do orq"; fi

# Sucessão do árbitro antes do lançamento: armação, lista, alarmes e /orq vão para arb2.
printf '%s' '[{"name":"exec1","state":"idle"},{"name":"rev1","state":"idle"},{"name":"arb2","state":"idle"}]' > "$t/sessions.json"
novo troca
orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
orq event entrega --task 1 --rodada 1 --commit abc
orq event sessao_trocada --de arb --para arb2
vigia
grep -q "watching: rev1 arb2" "$t/out" || fail "a lista não seguiu a sucessão do árbitro"
grep -q "^orq: --tmux arb2 \[vigia\] ARMED" "$t/sent.log" || fail "a armação não foi para o árbitro da vez"
grep -q "^orq: --tmux arb2 \[vigia\] rev1 is stopped" "$t/sent.log" || fail "o alarme não foi para arb2"
if grep -q -- "--tmux arb " "$t/sent.log"; then fail "falou com o árbitro que saiu"; fi
grep -q "/api/sessions/arb2/orq" "$t/urls" || fail "o contrato não foi lido do árbitro da vez"

# Cota estourada em duas sessões seguidas: a troca de vez rearma o alarme.
printf '%s' '[{"name":"exec1","state":"idle","limited":true},{"name":"rev1","state":"idle","limited":true},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo cota
orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
orq event entrega --task 1 --rodada 1 --commit abc
printf 'python3 %q --dir %q event veredito --task 1 --rodada 1 --resultado reprova --sessao rev1 >/dev/null\n' \
  "$ORQPY" "$d" > "$t/on-sessions"
vigia
grep -q "^orq: --tmux arb \[vigia\] Account out of quota: rev1=noquota" "$t/sent.log" || fail "sem alarme de cota para rev1"
grep -q "^orq: --tmux arb \[vigia\] Account out of quota: exec1=noquota" "$t/sent.log" || fail "o alarme de cota calou para exec1"

# Sem `orq init`, todo alarme via orq cairia só no log: o vigia recusa armar.
d="$t/vazio"; mkdir -p "$d"; : > "$t/sent.log"
if PATH="$t/bin:$PATH" CP_ENV="$t/env" CP_VIGIA_INTERVALO=0 CP_VIGIA_CICLOS=1 CP_VIGIA_LOG="$t/err" \
  timeout 10 bash "$raiz/skills/orquestrar/scripts/vigia.sh" arb -e "$d" > "$t/out" 2> "$t/err2"; then
  fail "armou sem orq init"
fi
[ ! -s "$t/sent.log" ] || fail "mandou recado sem orq init"
grep -q "orq.json" "$t/err2" || fail "não disse por que não armou"
echo ok
