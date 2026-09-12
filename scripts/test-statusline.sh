#!/usr/bin/env bash
# Trava o contrato da statusline em sessão de motor: não mostrar custo com tabela Anthropic e não
# exportar esse custo para o cost-tracker do ecc. Sem framework — script com asserts.
set -euo pipefail
cd "$(dirname "$0")"

# O `node` aqui é binário do WINDOWS: ele resolve a string "/tmp/x" como C:\tmp\x, que não é o
# /tmp que o Git Bash monta (esse aponta pro %TEMP%). Caminho que o teste só ESCREVE funciona pelos
# dois lados, mas caminho que entra no PAYLOAD é lido pelo node — e o transcript ficava invisível,
# derrubando os 5 asserts dos chips ♻/⏳ só no Windows. `cygpath -m` dá a forma que os dois
# entendem; onde não há cygpath (Linux/macOS), $TMPBASE já servia pros dois.
TMPBASE="${TMPDIR:-/tmp}"
if command -v cygpath >/dev/null 2>&1; then TMPBASE=$(cygpath -m "$TMPBASE"); fi

PAYLOAD='{"session_id":"t1","model":{"display_name":"k3"},
"workspace":{"current_dir":"/tmp"},"cost":{"total_cost_usd":1.04,"total_duration_ms":60000},
"context_window":{"remaining_percentage":50,"total_input_tokens":94000,
"total_output_tokens":5,"context_window_size":262144},"effort":{"level":"high"},
"thinking":{"enabled":true}}'

# Payload da medida de largura: duração fixa (52m) e contadores fixos, pra a largura dos segmentos
# não depender do relógio nem da sessão — só o `15:02` do relógio varia, e ele tem 5 colunas sempre.
PAYLOAD_LARGURA='{"session_id":"t1","model":{"display_name":"k3"},
"workspace":{"current_dir":"/tmp"},"cost":{"total_cost_usd":1.04,"total_duration_ms":3120000},
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
CACHE="$TMPBASE/hangar-engine-usage-$MOTOR.json"
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
TR="$TMPBASE/cp-test-transcript.jsonl"
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
rm -f "$TMPBASE/harness-cost-t2.json"

# O sidecar de custo não pode ser escrito em sessão de motor.
rm -f "$TMPBASE/harness-cost-t1.json"
motor >/dev/null
if [ -f "$TMPBASE/harness-cost-t1.json" ]; then
  echo "FALHOU: sidecar de custo escrito em sessao de motor"; falhou=1
fi
# E precisa continuar sendo escrito na sessão normal (não quebrar o cost-tracker do ecc).
rm -f "$TMPBASE/harness-cost-t1.json"
printf '%s' "$PAYLOAD" | env -u CP_ENGINE node omniroute-statusline.js >/dev/null
if [ ! -f "$TMPBASE/harness-cost-t1.json" ]; then
  echo "FALHOU: sidecar de custo deixou de ser escrito na sessao normal"; falhou=1
fi

# Quebra de linha medida em COLUNAS DO TERMINAL, não em unidades de código UTF-16. `.length` do JS
# conta 1 para emoji do BMP que desenha 2 (⏱ U+23F1, ⚡ U+26A1, ⏳ U+23F3, ♻ U+267B), então a barra
# media menos do que ocupava, juntava um segmento a mais e quem cortava era o Claude Code — com o
# "…" que o comentário do sidecar (omniroute-statusline.js) descreve: corte no par de contexto
# deixa o app sem medir contexto. Medido 12/09/2026 num pane de 62 colunas (terminal do celular).
#
# A régua é o test-statusline-largura.mjs, escrita à mão e INDEPENDENTE da do código: conferir a
# largura com a mesma função que decide a quebra não provaria nada.
# Larguras escolhidas (35 e 60) são onde a conta errada junta exatamente um segmento a mais; o
# `env -u TMUX` tira os segmentos que dependem da máquina (sessão do tmux) pra medida não mudar de
# lugar entre máquinas, e sem cache de quota os chips com hora de reset ficam fora — texto que muda
# com o relógio não pode entrar numa medida de largura.
SAIDA="$TMPBASE/hangar-statusline-largura.txt"
rm -f "$CACHE"
for COLS in 35 60; do
  printf '%s' "$PAYLOAD_LARGURA" | env -u TMUX -u TMUX_PANE CP_ENGINE=$MOTOR     CP_STATUSLINE_NO_REFRESH=1 COLUMNS=$COLS node omniroute-statusline.js > "$SAIDA"
  estourou=$(node test-statusline-largura.mjs "$SAIDA" "$COLS")
  if [ -n "$estourou" ]; then
    echo "FALHOU: em $COLS colunas a barra passou da largura (real: $estourou) - o Claude Code corta com '…'"; falhou=1
  fi
  # Medir certo é pra CABER, não pra picar a barra: 4 segmentos não podem virar 4 linhas.
  linhas=$(( $(wc -l < "$SAIDA") + 1 ))
  if [ "$linhas" -gt 3 ]; then
    echo "FALHOU: statusline quebrou em $linhas linhas em $COLS colunas"; falhou=1
  fi
done
rm -f "$SAIDA"

rm -f "$TMPBASE/harness-cost-t1.json" "$CACHE"  # limpa o que os testes acima escrevem de propósito

[ "$falhou" = 0 ] && echo "statusline OK" || exit 1
