#!/usr/bin/env bash
# Vigia do tubo. Olha TODAS as sessões vivas do trabalho — cada executor, cada revisor e o próprio
# árbitro — e acorda o árbitro por mensagem quando NINGUÉM está com a bola.
#
# Por que os três, e não só o par: em 14/08/2026 o árbitro levou `API Error: 529 Overloaded` às
# 03:36 e ficou morto até 06:09. O executor tinha entregado às 03:32; o relato dele ficou preso na
# fila, o revisor não tinha o que revisar, e o time inteiro parou 2h30. A vigia da época só olhava
# o par e mandava `echo` — que vira notificação apenas se o turno do árbitro estiver VIVO. Gritou
# para o vazio.
#
# Duas correções, e as duas importam:
#   1. Ela olha o ÁRBITRO também. Juiz parado é o modo de falha que ninguém estava vigiando.
#   2. Ela ACORDA por `cp-send --tmux`, que entra como prompt e reanima turno morto. O `--tmux` é
#      OBRIGATÓRIO: o `cp-send` normal RECUSA falar com sessão Claude da mesma máquina (rc=3,
#      "use SendMessage") — e um script de shell não tem SendMessage.
#
# A condição de disparo é conservadora de propósito: só quando TODAS estão paradas ao mesmo tempo.
# Árbitro parado com alguém trabalhando é o estado NORMAL (ele espera), e acordá-lo ali é ruído que
# gasta o token mais caro da mesa. Num lote paralelo isso importa mais ainda: com UMA vigia por par,
# cada uma enxergava só o seu pedaço e acordava o árbitro enquanto outro executor trabalhava.
#
# Rode com `setsid nohup … &`. Sem isso ela é filha do turno do árbitro e morre junto com ele —
# exatamente o que não pode acontecer, já que a morte dele é o caso que ela existe para cobrir.
#
# Uso: vigia.sh <sessao> [sessao...] <arbitro> [-m <minutos>]
#      O ÚLTIMO nome é sempre o árbitro. Ex.:
#      vigia.sh t1 t2 t3 review review2 arbitro -m 10
#      A forma antiga `vigia.sh exec rev arb 5` continua valendo.

set -u
# Aceita QUANTAS sessões forem: `vigia.sh <s1> <s2> ... <arbitro> [minutos]`. O último nome é
# sempre o ÁRBITRO — é para ele que os avisos vão, e é ele que a vigia reanima.
#
# Por que N e não três: um lote paralelo tem mais de um escritor. Rodar uma vigia por par
# funcionava, mas cada uma enxergava só o seu pedaço, e o disparo ("ninguém está com a bola") só é
# verdade quando olha TODO MUNDO — com pares separados, uma vigia acordava o árbitro enquanto outro
# executor trabalhava. O leitor em Python sempre aceitou N nomes (`sys.argv[1:]`); quem limitava a
# três era este shell.
#
# Os minutos vão por FLAG (`-m N` ou `--minutos N`), em qualquer posição. A forma antiga — número
# solto no fim — continua aceita, mas SÓ na assinatura de três nomes que a documentação ensinava
# (`vigia.sh exec rev arb 5`). Motivo: com N sessões, "último argumento numérico" é ambíguo — uma
# sessão chamada `123` seria comida como limite de minutos, e a vigia passaria a olhar uma sessão a
# menos, calada. Nome de sessão numérico não é hipótese: `sanitize_session_name` os aceita.
LIMITE=5
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    -m|--minutos) LIMITE=${2:?"-m precisa do numero de minutos"}; shift 2 ;;
    *) ARGS+=("$1"); shift ;;
  esac
done
# Retrocompatibilidade estrita: exatamente 4 posicionais e o último numérico = a chamada antiga.
n=${#ARGS[@]}
if [ "$n" -eq 4 ] && printf '%s' "${ARGS[3]}" | grep -qE '^[0-9]+$'; then
  LIMITE=${ARGS[3]}
  ARGS=("${ARGS[0]}" "${ARGS[1]}" "${ARGS[2]}")
fi
SESSOES=("${ARGS[@]}")
[ "${#SESSOES[@]}" -ge 2 ] || { echo "uso: vigia.sh <sessao> [sessao...] <arbitro> [minutos]" >&2; exit 2; }
ARB=${SESSOES[$((${#SESSOES[@]}-1))]}      # o último é o árbitro
export ARB

BASE=${CP_BASE:-http://127.0.0.1:8765}
ENVFILE=${CP_ENV:-$(dirname "$(realpath "$(command -v cp-send)")")/../backend/.env}
T=$(grep '^CP_AUTH_TOKEN=' "$ENVFILE" | cut -d= -f2-)
# Token vazio nao pode virar "nao consigo ler a API" cinco minutos depois: a vigia ficaria de pe,
# com log limpo, sem vigiar nada — o mesmo modo de falha que o leitor em arquivo veio corrigir.
[ -n "$T" ] || { echo "[vigia] CP_AUTH_TOKEN ausente em $ENVFILE — nao da pra ler /api/sessions" >&2; exit 1; }

# O token vai por ARQUIVO de configuracao do curl, nunca por `-H` na linha de comando: argumento de
# processo e legivel por qualquer usuario da maquina em `ps aux` / /proc/<pid>/cmdline, e esta
# chamada acontece uma vez por minuto durante a noite inteira. O mktemp cria com 0600.
CURLRC=$(mktemp /tmp/vigia-curlrc-XXXXXX)
printf 'header = "Authorization: Bearer %s"\n' "$T" > "$CURLRC"

parados=0
avisos=0
mudos=0
avisou_cota=
avisou_travado=

# O leitor de estado mora num arquivo, não numa linha `python3 -c '...'` dentro do laço. Motivo
# medido em 14/08/2026: o `-c` estava entre aspas SIMPLES do shell, e um `\"` ali chega ao Python
# como barra-mais-aspas dentro de uma f-string — `SyntaxError`. Como a chamada terminava em
# `2>/dev/null`, o erro sumia, `st` vinha vazio e o `continue` logo abaixo pulava a leitura. A vigia
# rodou o tempo todo, com processo vivo e log limpo, sem NUNCA ter olhado uma sessão. Nomes vão por
# argumento, não por variável de ambiente, para não precisar de aspas dentro do script embutido.
LEITOR=$(mktemp /tmp/vigia-leitor-XXXXXX.py)
trap 'rm -f "$LEITOR" "$CURLRC"' EXIT
cat > "$LEITOR" <<'PY'
import json, sys, time

# `working` NÃO é prova de que alguém está com a bola. Medido em 14/08/2026: o executor disparou um
# AskUserQuestion para provar a folha em inglês, e AskUserQuestion BLOQUEIA o turno de quem dispara.
# Ele ficou 1h17 parado esperando uma resposta que ninguém ia dar — e o app reportou `working` o
# tempo todo, porque o hook do Pi (`scripts/pi/cp-state.ts`) só publica `working` ou `idle`: não
# existe `awaiting_input` para sessão Pi. A vigia ficou calada e estava certa pela regra que tinha.
#
# O sinal que distingue os dois é `last_activity`, que avança a cada evento do transcript. Sessão
# trabalhando de verdade move esse número; sessão bloqueada num picker congela. `working` parado há
# mais de PARADO_S segundos conta como PARADO.
PARADO_S = 600

agora = time.time()
dados = json.load(sys.stdin)
mapa = {s.get("name"): s for s in dados}
saida = []
for nome in sys.argv[1:]:
    s = mapa.get(nome)
    if s is None:
        saida.append("sumiu")
    elif s.get("limited"):
        # Cota estourada é parada que não se desfaz sozinha: o modelo não volta a responder até
        # alguém trocar a conta. Vale mais que o estado do pane, que nesse caso mostra o último
        # quadro e pode ser lido como trabalho em curso.
        saida.append("semcota")
    else:
        estado = s.get("state") or "?"
        ts = s.get("last_activity")
        if estado == "working" and isinstance(ts, (int, float)) and agora - ts > PARADO_S:
            saida.append("travado")
        else:
            saida.append(estado)
print("|".join(saida))
PY

# Intervalo entre leituras. Existe como variável só para o teste de fumaça poder rodar o laço
# inteiro em segundos; em uso normal ninguém passa isso.
INTERVALO=${CP_VIGIA_INTERVALO:-60}

for i in $(seq 1 1440); do
  sleep "$INTERVALO"
  st=$(curl -s --config "$CURLRC" "$BASE/api/sessions" \
       | python3 "$LEITOR" "${SESSOES[@]}" 2>>"${CP_VIGIA_LOG:-/dev/stderr}")
  if [ -z "$st" ]; then
    # Silêncio da API não pode ser silêncio da vigia: era assim que o furo acima se escondia.
    mudos=$((mudos+1))
    if [ "$mudos" -eq 5 ]; then
      echo "[vigia] 5 leituras seguidas sem resposta de $BASE/api/sessions — nao estou vigiando nada"
      cp-send --tmux "$ARB" "[vigia] Nao consigo ler /api/sessions ha 5 minutos. Enquanto isso eu NAO estou vigiando ninguem — confira o backend e me rearme." >/dev/null 2>&1
    fi
    continue
  fi
  mudos=0

  # Um estado por sessão, na MESMA ordem de SESSOES. `resumo` é o que vai nas mensagens.
  IFS='|' read -r -a ESTADOS <<< "$st"
  resumo=""
  for k in "${!SESSOES[@]}"; do
    resumo="$resumo${resumo:+ · }${SESSOES[$k]}=${ESTADOS[$k]:-?}"
  done
  # Só o PAR (todos menos o árbitro) conta para travado/sem cota: árbitro parado é o normal.
  par_estados="${st%|*}"

  # "Parado" é tudo que não é trabalho em curso:
  #   idle          — terminou o turno e está esperando
  #   awaiting_input— travada num pedido de entrada; parada esperando gente, que é o caso mais
  #                   comum de sessão empacada e que a versão anterior tratava como "ocupada"
  #   sumiu         — morreu
  #   semcota       — limite da conta estourado; não volta sozinha
  #   travado       — diz `working` mas não produz evento há 10min (picker bloqueando o turno)
  quieto=1
  for e in "${ESTADOS[@]}"; do
    case "$e" in idle|awaiting_input|sumiu|semcota|travado) ;; *) quieto=0 ;; esac
  done

  # Sessão TRAVADA no par avisa na hora, sem esperar os três pararem: o árbitro está de bola cheia
  # justamente porque acha que o outro está trabalhando. Foi o caso de 14/08 — 1h17 de fila parada
  # com todo mundo achando que a Task andava.
  case "$par_estados" in
    *travado*)
      if [ "$avisou_travado" != "$par_estados" ]; then
        msg="[vigia] Sessao TRAVADA: $resumo. Diz 'working' mas nao produz evento ha mais de 10 minutos — o caso classico e um picker/AskUserQuestion bloqueando o turno de quem disparou. Olhe o pane e destrave (POST /api/sessions/<nome>/select com {\"option\": N})."
        echo "$msg"
        cp-send --tmux "$ARB" "$msg" >/dev/null 2>&1
        avisou_travado="$par_estados"
      fi
      ;;
  esac

  # Cota estourada no par não espera os três pararem nem o limite de minutos: o árbitro precisa
  # trocar a conta da sessão, e cada minuto de espera é minuto de fila parada.
  case "$par_estados" in
    *semcota*)
      if [ "$avisou_cota" != "$par_estados" ]; then
        msg="[vigia] Conta sem cota: $resumo. A sessao nao volta sozinha — abra a substituta numa conta PERMITIDA pelo contrato e mande o mesmo kick-off, com a Task em aberto."
        echo "$msg"
        cp-send --tmux "$ARB" "$msg" >/dev/null 2>&1
        avisou_cota="$par_estados"
      fi
      ;;
  esac

  if [ "$quieto" -eq 1 ]; then parados=$((parados+1)); else parados=0; fi

  if [ "$parados" -ge "$LIMITE" ]; then
    msg="[vigia] Ninguem esta com a bola ha ${LIMITE} min: $resumo (minuto $i). Se voce caiu (erro de API), isto e o que te traz de volta. Cheque se alguem entregou enquanto voce estava fora — relato preso na fila e veredito parado sao os dois jeitos de o tubo travar sem ninguem perceber."
    echo "$msg"
    cp-send --tmux "$ARB" "$msg" >/dev/null 2>&1
    avisos=$((avisos+1))
    parados=0
    if [ "$avisos" -ge 20 ]; then
      echo "20 avisos sem destravar; encerrando vigia"
      exit 0
    fi
  fi
done
echo "1440min encerrados; ultimo estado: $resumo"
