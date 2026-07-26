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

# Sessão de motor: custo suprimido, resto intacto.
out=$(printf '%s' "$PAYLOAD" | env CP_ENGINE=kimi node omniroute-statusline.js)
checa "motor nao mostra custo Anthropic" nao '$1.04' "$out"
checa "motor mantem o contexto"          tem '262k'  "$out"
# Effort NÃO é suprimido: no Kimi o thinking é real, suprimir seria mentira nova.
checa "motor mantem o effort"            tem 'high'  "$out"

# O sidecar de custo não pode ser escrito em sessão de motor.
rm -f "${TMPDIR:-/tmp}/harness-cost-t1.json"
printf '%s' "$PAYLOAD" | env CP_ENGINE=kimi node omniroute-statusline.js >/dev/null
if [ -f "${TMPDIR:-/tmp}/harness-cost-t1.json" ]; then
  echo "FALHOU: sidecar de custo escrito em sessao de motor"; falhou=1
fi
# E precisa continuar sendo escrito na sessão normal (não quebrar o cost-tracker do ecc).
rm -f "${TMPDIR:-/tmp}/harness-cost-t1.json"
printf '%s' "$PAYLOAD" | env -u CP_ENGINE node omniroute-statusline.js >/dev/null
if [ ! -f "${TMPDIR:-/tmp}/harness-cost-t1.json" ]; then
  echo "FALHOU: sidecar de custo deixou de ser escrito na sessao normal"; falhou=1
fi

[ "$falhou" = 0 ] && echo "statusline OK" || exit 1
