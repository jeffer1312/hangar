#!/usr/bin/env bash
# vigia.sh -e: a lista segue `orq ball` e os avisos ao árbitro passam por `orq notify`.
# Sem backend: curl e hangar-send são falsos. O envio do orq tem fake próprio (linha `orq: …`),
# para que um alarme que saia por hangar-send direto não passe por um que saiu pelo orq.
set -euo pipefail
raiz="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
export VT="$t"
mkdir -p "$t/bin"
# /api/sessions responde $VT/sessions.json, ou nada com $VT/api-down; $VT/on-sessions, se existir,
# roda uma vez antes. /history responde $VT/history.json.
cat > "$t/bin/curl" <<'SH'
#!/bin/sh
for a; do url=$a; done
printf '%s\n' "$*" >> "$VT/urls"
case "$url" in
  */api/sessions)
    [ -f "$VT/api-down" ] && exit 0
    if [ -f "$VT/on-sessions" ]; then bash "$VT/on-sessions"; rm -f "$VT/on-sessions"; fi
    cat "$VT/sessions.json" ;;
  */history*) [ -f "$VT/history.json" ] || exit 22; cat "$VT/history.json" ;;
  *) exit 22 ;;
esac
SH
cat > "$t/bin/hangar-send" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >> "$VT/sent.log"
SH
# $VT/falha = "<n> <trecho>": as próximas n entregas do orq que contêm o trecho falham.
cat > "$t/bin/orq-send" <<'SH'
#!/bin/sh
printf 'orq: %s\n' "$*" >> "$VT/sent.log"
if [ -f "$VT/falha" ]; then
  read -r n trecho < "$VT/falha"
  case "$*" in
    *"$trecho"*) if [ "$n" -gt 0 ]; then echo "$((n - 1)) $trecho" > "$VT/falha"; exit 1; fi ;;
  esac
fi
SH
chmod +x "$t/bin/curl" "$t/bin/hangar-send" "$t/bin/orq-send"
printf 'CP_AUTH_TOKEN=x\n' > "$t/env"
export ORQ_SEND="$t/bin/orq-send" ORQ_JEV=off
ORQPY="$raiz/skills/orquestrar/scripts/orq.py"
orq() { python3 "$ORQPY" --dir "$d" "$@" >/dev/null; }
novo() {
  d="$t/$1"; mkdir -p "$d"
  : > "$t/sent.log"; : > "$t/urls"; : > "$t/err"
  rm -f "$t/falha" "$t/api-down" "$t/history.json"
  orq init --arbiter arb --repo "$raiz" --contract "$t/regras.md"
}
# M, CICLOS, INTERVALO e REP mudam por cenário (VAR=x vigia); o stderr do vigia vai pro $VT/err.
vigia() {
  PATH="$t/bin:$PATH" CP_ENV="$t/env" CP_VIGIA_INTERVALO="${INTERVALO:-0}" CP_VIGIA_CICLOS="${CICLOS:-3}" \
    CP_VIGIA_REP="${REP:-10}" CP_VIGIA_LOG="$t/err" \
    timeout 20 bash "$raiz/skills/orquestrar/scripts/vigia.sh" arb -e "$d" -m "${M:-1}" "$@" > "$t/out" 2>> "$t/err" || fail "vigia died (rc=$?)"
}
fail() { echo "FAIL: $1"; for f in out err sent.log; do echo "--- $f"; cat "$t/$f"; done; exit 1; }

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
# Batimento: o painel sabe quem o vigia olha sem perguntar ao árbitro.
python3 - "$d/vigia.json" <<'PY' || fail "batimento ausente ou torto"
import json, sys
hb = json.load(open(sys.argv[1], encoding="utf-8"))
assert hb["arbiter"] == "arb" and hb["watching"] == ["rev1", "arb"], hb
assert hb["states"] == {"rev1": "idle", "arb": "idle"} and hb["interval_s"] == 0, hb
assert isinstance(hb["pid"], int) and "T" in hb["ts"], hb
PY
if ls -A "$d" | grep -q '^\.vigia\.json\.'; then fail "sobrou temporário do batimento"; fi

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

# API muda 5 voltas: o aviso ao árbitro passa pelo orq e fica no registro como alarme.
printf '%s' '[{"name":"exec1","state":"working","last_activity":9999999999},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo api-muda
orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
: > "$t/api-down"
CICLOS=5 vigia
grep -q "^orq: --tmux arb \[vigia\] I cannot read /api/sessions" "$t/sent.log" || fail "API muda não passou pelo orq"
if grep -q "^--tmux arb " "$t/sent.log"; then fail "alarme de API muda saiu por fora do orq"; fi
grep -q "alarm: \[vigia\] I cannot read /api/sessions" "$d/registro.md" || fail "alarme de API muda fora do registro"

# Leitor de contexto morto 5 voltas (status_line que não é texto derruba o CTXDET): idem.
printf '%s' '[{"name":"exec1","state":"working","last_activity":9999999999,"status_line":1},{"name":"arb","state":"idle","status_line":1}]' > "$t/sessions.json"
novo ctx-mudo
orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
CICLOS=5 vigia
grep -q "^orq: --tmux arb \[vigia\] I cannot read the sessions' context" "$t/sent.log" || fail "contexto mudo não passou pelo orq"
if grep -q "^--tmux arb " "$t/sent.log"; then fail "alarme de contexto mudo saiu por fora do orq"; fi

# Entrega que falhou não conta como alarme dado: volta na volta seguinte e a falha fica no log.
retenta() {  # $1 = trecho do alarme cuja primeira entrega o fake do orq derrubou
  [ "$(grep -c "^orq: --tmux arb \[vigia\] .*$1" "$t/sent.log")" -ge 2 ] || fail "alarme '$1' não voltou depois de falhar"
  grep -q "NOT delivered" "$t/err" || fail "falha de entrega de '$1' não foi pro log"
}
printf '%s' '[{"name":"exec1","state":"idle"},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo r-parada; orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
echo "1 exec1 is stopped" > "$t/falha"
M=2 vigia
retenta "exec1 is stopped"

printf '%s' '[{"name":"exec1","state":"working","last_activity":9999999999},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo r-laco; orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
printf '%s' '[{"kind":"tool_use","tool_input":{"command":"sleep 5"}}]' > "$t/history.json"
echo "1 MAY be looping" > "$t/falha"
REP=1 vigia
retenta "MAY be looping"

novo r-trilha; orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
touch -d '2 hours ago' "$d/eventos.jsonl" "$d/registro.md"
echo "1 The trail" > "$t/falha"
vigia
retenta "The trail"

printf '%s' '[{"name":"exec1","state":"working","last_activity":1},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo r-travada; orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
echo "1 STUCK session" > "$t/falha"
M=5 vigia
retenta "STUCK session"

printf '%s' '[{"name":"exec1","state":"idle","limited":true},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo r-cota; orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
echo "1 Account out of quota" > "$t/falha"
M=5 vigia
retenta "Account out of quota"

printf '%s' '[{"name":"exec1","state":"idle"},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo r-ninguem; orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
echo "1 Nobody has had the ball" > "$t/falha"
M=2 vigia
retenta "Nobody has had the ball"

# Três entregas falhas seguidas: o alarme é largado com um [aviso] no registro, sem 4ª tentativa.
printf '%s' '[{"name":"exec1","state":"idle","limited":true},{"name":"arb","state":"idle"}]' > "$t/sessions.json"
novo r-teto; orq event task_inicio --task 1 --titulo x --executor exec1 --par rev1
echo "9 Account out of quota" > "$t/falha"
M=100 CICLOS=6 vigia
[ "$(grep -c "^orq: --tmux arb \[vigia\] Account out of quota" "$t/sent.log")" -eq 3 ] || fail "o alarme não parou na 3ª tentativa"
[ "$(grep -c "\[aviso\] \[vigia\] alarm dropped after 3 failed deliveries: .*Account out of quota" "$d/registro.md")" -eq 1 ] \
  || fail "o alarme largado não deixou um [aviso] no registro"

# Sem `orq init`, todo alarme via orq cairia só no log: o vigia recusa armar.
d="$t/vazio"; mkdir -p "$d"; : > "$t/sent.log"
if PATH="$t/bin:$PATH" CP_ENV="$t/env" CP_VIGIA_INTERVALO=0 CP_VIGIA_CICLOS=1 CP_VIGIA_LOG="$t/err" \
  timeout 10 bash "$raiz/skills/orquestrar/scripts/vigia.sh" arb -e "$d" > "$t/out" 2> "$t/err2"; then
  fail "armou sem orq init"
fi
[ ! -s "$t/sent.log" ] || fail "mandou recado sem orq init"
grep -q "orq.json" "$t/err2" || fail "não disse por que não armou"
echo ok
