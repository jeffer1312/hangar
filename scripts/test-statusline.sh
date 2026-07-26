#!/usr/bin/env bash
# Trava o contrato da statusline em sessão de motor: não mostrar custo com tabela Anthropic e não
# exportar esse custo para o cost-tracker do ecc. Sem framework — script com asserts.
set -euo pipefail
cd "$(dirname "$0")"

PAYLOAD='{"session_id":"t1","model":{"display_name":"k3"},
"workspace":{"current_dir":"/tmp"},"cost":{"total_cost_usd":1.04,"total_duration_ms":60000},
"context_window":{"remaining_percentage":50,"total_input_tokens":94000,
"total_output_tokens":5,"context_window_size":262144},"effort":{"level":"high"},
"thinking":{"enabled":true}}'

falhou=0
checa() {  # $1=descricao $2=tem|nao $3=agulha $4=saida
  case "$2" in
    tem) case "$4" in *"$3"*) ;; *) echo "FALHOU: $1 (esperava '$3')"; falhou=1 ;; esac ;;
    nao) case "$4" in *"$3"*) echo "FALHOU: $1 (não devia ter '$3')"; falhou=1 ;; *) ;; esac ;;
  esac
}

# Sessão normal: nada muda.
out=$(printf '%s' "$PAYLOAD" | env -u CP_ENGINE node omniroute-statusline.js)
checa "sessao normal mostra custo"     tem '$1.04'  "$out"
checa "sessao normal mostra o contexto" tem '262k'  "$out"

# Motor fake: não encosta no cache dos motores reais. NO_REFRESH corta a chamada de rede.
MOTOR=motor-teste
CACHE="${TMPDIR:-/tmp}/cp-engine-usage-$MOTOR.json"
motor() { printf '%s' "$PAYLOAD" | env CP_ENGINE=$MOTOR CP_STATUSLINE_NO_REFRESH=1 node omniroute-statusline.js; }

# Sessão de motor: custo suprimido, resto intacto.
rm -f "$CACHE"
out=$(motor)
checa "motor nao mostra custo Anthropic" nao '$1.04' "$out"
checa "motor mantem o contexto"          tem '262k'  "$out"
# Effort NÃO é suprimido: no Kimi o thinking é real, suprimir seria mentira nova.
checa "motor mantem o effort"            tem 'high'  "$out"
# Sem quota cacheada não inventa chip (payload de motor não traz rate_limits).
checa "motor sem cache nao mostra 5h"    nao '⚡5h'   "$out"
checa "motor sem cache nao mostra 7d"    nao '📅7d'   "$out"

# Com quota cacheada: janela curta vira ⚡, longa vira 📅.
AGORA=$(date +%s)
printf '{"ts":%s,"janelas":[{"pct":36,"reset":%s},{"pct":20,"reset":%s}]}' \
  "$AGORA" "$((AGORA + 3600))" "$((AGORA + 6 * 86400))" > "$CACHE"
out=$(motor)
checa "motor mostra a janela curta em 5h" tem '⚡5h:36%' "$out"
checa "motor mostra a janela longa em 7d" tem '📅7d:20%' "$out"

# Cache corrompido/de outro formato derruba o chip, NUNCA a statusline inteira.
printf '{"ts":%s,"janelas":{"nao":"array"}}' "$AGORA" > "$CACHE"
out=$(motor)
checa "cache malformado mantem o resto" tem '262k' "$out"
checa "cache malformado nao vira chip"  nao '⚡5h'  "$out"
printf '{"ts":%s,"janelas":[null,{"pct":"x"},{"pct":42}]}' "$AGORA" > "$CACHE"
out=$(motor)
checa "entrada lixo e ignorada"      nao '⚡5h:NaN' "$out"
checa "entrada boa ainda desenha"    tem '⚡5h:42%'  "$out"
checa "cache sao nao marca velho"    nao '⚡5h:42%⚠' "$out"

# Falha de leitura registrada pelo refresher precisa CHEGAR NA TELA: número velho não pode ter a
# mesma cara de número fresco.
printf '{"ts":%s,"janelas":[{"pct":42,"reset":%s}],"erro":"provedor fora do ar"}' "$AGORA" "$((AGORA + 3600))" > "$CACHE"
out=$(motor)
# O ⚠ tem que estar COLADO no %: sozinho ele casaria com o chip de contexto kubectl prod.
checa "quota com erro marca o chip" tem '⚡5h:42%⚠' "$out"

# Sessão normal ignora o cache de motor — quota de provedor não vale pra conta Anthropic.
out=$(printf '%s' "$PAYLOAD" | env -u CP_ENGINE node omniroute-statusline.js)
checa "sessao normal ignora quota de motor" nao '⚡5h' "$out"
rm -f "$CACHE"

# ── Chip de cache (só em motor) ──────────────────────────────────────────────
TR="${TMPDIR:-/tmp}/cp-test-transcript.jsonl"
com_transcript() {  # $1 = payload extra com transcript_path
  printf '%s' "$1" | env CP_ENGINE=$MOTOR CP_STATUSLINE_NO_REFRESH=1 node omniroute-statusline.js
}
PT=$(printf '{"session_id":"t2","model":{"display_name":"k3"},"workspace":{"current_dir":"/tmp"},"transcript_path":"%s","context_window":{"remaining_percentage":50,"total_input_tokens":94000,"total_output_tokens":5,"context_window_size":262144}}' "$TR")

# Turno que acertou o cache, 12 min depois do anterior: taxa alta + marca de intervalo.
{ printf '{"timestamp":"2026-07-26T10:00:00Z","message":{"id":"m1","usage":{"input_tokens":900,"cache_read_input_tokens":120000,"output_tokens":10}}}\n'
  printf '{"timestamp":"2026-07-26T10:12:00Z","message":{"id":"m2","usage":{"input_tokens":1000,"cache_read_input_tokens":130000,"output_tokens":10}}}\n'
  printf '{"timestamp":"2026-07-26T10:12:00Z","message":{"id":"m2","usage":{"input_tokens":1000,"cache_read_input_tokens":130000,"output_tokens":10}}}\n'
} > "$TR"
out=$(com_transcript "$PT")
checa "cache alto vira taxa"        tem '♻99%' "$out"
checa "intervalo longo vira ampulheta" tem '⏳12m' "$out"

# Turno sintético (usage zerado) no meio não pode encurtar o intervalo: 52min tem que continuar 52min.
{ printf '{"timestamp":"2026-07-26T09:00:00Z","message":{"id":"m1","usage":{"input_tokens":900,"cache_read_input_tokens":50000,"output_tokens":10}}}\n'
  printf '{"timestamp":"2026-07-26T09:40:00Z","message":{"id":"ms","model":"<synthetic>","usage":{"input_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}\n'
  printf '{"timestamp":"2026-07-26T09:52:00Z","message":{"id":"m2","usage":{"input_tokens":1000,"cache_read_input_tokens":190000,"output_tokens":10}}}\n'
} > "$TR"
out=$(com_transcript "$PT")
checa "turno sintetico nao encurta o intervalo" tem '⏳52m' "$out"
checa "turno sintetico nao vira o intervalo"    nao '⏳12m' "$out"

# Intervalo de horas: o resto precisa vir com unidade ('1h5m', nunca '1h5').
{ printf '{"timestamp":"2026-07-26T10:00:00Z","message":{"id":"m1","usage":{"input_tokens":900,"cache_read_input_tokens":120000,"output_tokens":10}}}\n'
  printf '{"timestamp":"2026-07-26T11:05:00Z","message":{"id":"m2","usage":{"input_tokens":1000,"cache_read_input_tokens":130000,"output_tokens":10}}}\n'
} > "$TR"
out=$(com_transcript "$PT")
checa "intervalo em horas leva o resto" tem '⏳1h5m' "$out"

# Re-prefill: contexto inteiro cobrado como input novo -> taxa no chão, sem ampulheta (turno colado).
{ printf '{"timestamp":"2026-07-26T10:00:00Z","message":{"id":"m1","usage":{"input_tokens":900,"cache_read_input_tokens":120000,"output_tokens":10}}}\n'
  printf '{"timestamp":"2026-07-26T10:01:00Z","message":{"id":"m2","usage":{"input_tokens":131000,"cache_read_input_tokens":0,"output_tokens":10}}}\n'
} > "$TR"
out=$(com_transcript "$PT")
checa "re-prefill aparece como 0%" tem '♻0%' "$out"
checa "turno colado nao marca intervalo" nao '⏳' "$out"

# Sessão normal não ganha o chip, e transcript inexistente não quebra nada.
out=$(printf '%s' "$PT" | env -u CP_ENGINE node omniroute-statusline.js)
checa "sessao normal nao tem chip de cache" nao '♻' "$out"
rm -f "$TR"
out=$(com_transcript "$PT")
checa "transcript sumido nao quebra"  tem '262k' "$out"
checa "transcript sumido nao inventa" nao '♻'    "$out"
rm -f "${TMPDIR:-/tmp}/harness-cost-t2.json"

# O sidecar de custo não pode ser escrito em sessão de motor.
rm -f "${TMPDIR:-/tmp}/harness-cost-t1.json"
motor >/dev/null
if [ -f "${TMPDIR:-/tmp}/harness-cost-t1.json" ]; then
  echo "FALHOU: sidecar de custo escrito em sessao de motor"; falhou=1
fi
# E precisa continuar sendo escrito na sessão normal (não quebrar o cost-tracker do ecc).
rm -f "${TMPDIR:-/tmp}/harness-cost-t1.json"
printf '%s' "$PAYLOAD" | env -u CP_ENGINE node omniroute-statusline.js >/dev/null
if [ ! -f "${TMPDIR:-/tmp}/harness-cost-t1.json" ]; then
  echo "FALHOU: sidecar de custo deixou de ser escrito na sessao normal"; falhou=1
fi

rm -f "${TMPDIR:-/tmp}/harness-cost-t1.json" "$CACHE"  # limpa o que os testes acima escrevem de propósito

[ "$falhou" = 0 ] && echo "statusline OK" || exit 1
