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
#   2. Ela ACORDA por `hangar-send --tmux`, que entra como prompt e reanima turno morto. O `--tmux` é
#      OBRIGATÓRIO: o `hangar-send` normal RECUSA falar com sessão Claude da mesma máquina (rc=3,
#      "use SendMessage") — e um script de shell não tem SendMessage.
#
# A condição de disparo é conservadora de propósito: só quando TODAS estão paradas ao mesmo tempo.
# Árbitro parado com alguém trabalhando é o estado NORMAL (ele espera), e acordá-lo ali é ruído que
# gasta o token mais caro da mesa. Num lote paralelo isso importa mais ainda: com UMA vigia por par,
# cada uma enxergava só o seu pedaço e acordava o árbitro enquanto outro executor trabalhava.
#
# Rode como SERVIÇO, nunca como processo de fundo do turno (`setsid nohup … &` morre junto com o
# turno que a lançou — some do ps, log vazio, sem erro; medido em 17/08/2026):
#   systemd-run --user --unit=vigia-<gid> --property=Restart=always --property=RestartSec=20 \
#     "$SKILL/scripts/vigia.sh" <sessao> [sessao...] <arbitro> -m 5 -d <registro.md>
# `Restart=always` é a outra metade: sem ele, a unidade que cair deixa o trabalho sem rede.
#
# Uso: vigia.sh <sessao> [sessao...] <arbitro> [-m <minutos>] [-d <diario.md>]
#      O ÚLTIMO nome é sempre o árbitro. Os minutos vão por FLAG (`-m 5`), nunca como número solto
#      no fim: com mais de três sessões o número posicional vira NOME de sessão, e os alarmes vão
#      pra uma sessão chamada "5" enquanto o grupo para. Ex.:
#      vigia.sh t1 t2 t3 review review2 arbitro -m 10 -d ~/.hangar/orq/<data>-<gid>/registro.md
#      A forma antiga `vigia.sh exec rev arb 5` continua valendo.
#
# Confirmar que ela VIVE (is-active logo após o systemd-run responde `active` porque acabou de
# nascer, não porque lê a API — uma vigia já ficou `active` por horas sem uma linha de log):
#   journalctl --user -u vigia-<gid> --since "-3min"   # sem erro repetido a cada ciclo
#   systemctl --user show vigia-<gid> -p ActiveState -p MainPID
# E espere um ciclo inteiro (60s). A prova de verdade é o alarme sintético chegar (abaixo).
#
# Três alarmes além do "todo mundo parado", cada um nascido de uma falha real de 17/08/2026:
#   - REPETIÇÃO: sessão `working` cujo último comando é o MESMO por N leituras seguidas está em
#     loop, não trabalhando — polling produz evento a cada poucos segundos e engana o sensor de
#     ociosidade. Medido: 1.231 execuções do mesmo comando por 3h, `working` o tempo inteiro,
#     68% da fatura da execução. Sucesso repetido é tão parado quanto erro repetido.
#   - DIÁRIO (-d): registro do árbitro sem escrita há 60min com o grupo ativo. Medido: 6h45 sem
#     uma linha, justamente as duas Tasks mais caras.
#   - ARMAMENTO PROVADO: ao subir, a vigia manda um alarme sintético ao árbitro PELO MESMO caminho
#     dos alarmes reais. "Funcionando" é esse prompt chegar — não `is-active`, não teste digitado à
#     mão (os dois "provaram" duas vezes um canal que estava quebrado).

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
DIARIO=
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    -m|--minutos) LIMITE=${2:?"-m precisa do numero de minutos"}; shift 2 ;;
    -d|--diario)  DIARIO=${2:?"-d precisa do caminho do registro"}; shift 2 ;;
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
ENVFILE=${CP_ENV:-$(dirname "$(realpath "$(command -v hangar-send)")")/../backend/.env}
T=$(grep '^CP_AUTH_TOKEN=' "$ENVFILE" | cut -d= -f2-)
# Token vazio nao pode virar "nao consigo ler a API" cinco minutos depois: a vigia ficaria de pe,
# com log limpo, sem vigiar nada — o mesmo modo de falha que o leitor em arquivo veio corrigir.
[ -n "$T" ] || { echo "[vigia] CP_AUTH_TOKEN ausente em $ENVFILE — nao da pra ler /api/sessions" >&2; exit 1; }

# O token vai por ARQUIVO de configuracao do curl, nunca por `-H` na linha de comando: argumento de
# processo e legivel por qualquer usuario da maquina em `ps aux` / /proc/<pid>/cmdline, e esta
# chamada acontece uma vez por minuto durante a noite inteira. O mktemp cria com 0600.
CURLRC=$(mktemp /tmp/vigia-curlrc-XXXXXX)
printf 'header = "Authorization: Bearer %s"\n' "$T" > "$CURLRC"

# Onde vai o erro do leitor. O default NAO pode ser /dev/stderr: rodando sem terminal (systemd,
# cron, nohup redirecionado) ele nao abre pra escrita, o redirecionamento `2>>` FALHA, e o bash nao
# executa o comando — `st` volta vazio e a vigia conclui "API sem resposta". Medido em 17/08/2026:
# a unidade ficava `active` cinco minutos e gritava "nao estou vigiando nada" com o backend
# respondendo 200 em 11ms. Testa uma vez e cai pra arquivo quando nao der.
if [ -z "${CP_VIGIA_LOG:-}" ]; then
  if : 2>>/dev/stderr; then CP_VIGIA_LOG=/dev/stderr
  else CP_VIGIA_LOG=${TMPDIR:-/tmp}/vigia-$$.err; fi
fi
export CP_VIGIA_LOG

parados=0
avisos=0
PSEQ=()          # leituras seguidas paradas, por sessão (o aviso por sessão usa isto)
NUDGE=()         # já cutucou esta sessão nesta parada? (1 empurrão por parada, não a cada aviso)
RHASH=()         # hash do último comando visto, por sessão (detector de loop)
RSEQ=()          # leituras seguidas com o MESMO comando, por sessão
RAVISO=()        # já avisou este loop? (1 aviso por streak)
mudos=0
avisou_cota=
avisou_travado=
diario_avisado=0

# ARMAMENTO PROVADO: o alarme sintético sai pelo MESMO caminho dos reais. Se ele não entrega, a
# vigia NÃO fica de pé fingindo rede — sai com erro alto, que é o contrário de gritar pro vazio.
hangar-send --tmux "$ARB" "[vigia] ARMADA sobre: ${SESSOES[*]} (janela ${LIMITE}min${DIARIO:+, diario $DIARIO}). Esta mensagem E a prova do canal — se voce a leu, os alarmes chegam. Nao responda."
rc_arm=$?
if [ "$rc_arm" -ne 0 ]; then
  echo "[vigia] FALHA ao provar o canal com '$ARB' (hangar-send --tmux rc=$rc_arm). NAO estou armada." >&2
  exit 1
fi

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
# tempo todo, porque o hook do Pi (`scripts/pi/hangar-state.ts`) só publica `working` ou `idle`: não
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

# Detector de LOOP: extrai do /history o hash do último comando de ferramenta. Sessão `working`
# devolvendo o MESMO hash por REP_LIMITE leituras não está trabalhando — está apertando a mesma
# tecla. O id do evento muda a cada chamada; o que se compara é o CONTEÚDO (tool_input).
LOOPDET=$(mktemp /tmp/vigia-loopdet-XXXXXX.py)
trap 'rm -f "$LEITOR" "$CURLRC" "$LOOPDET"' EXIT
cat > "$LOOPDET" <<'PY'
import hashlib, json, sys
try:
    evs = json.load(sys.stdin)
    tool = [e for e in evs if isinstance(e, dict) and e.get("kind") == "tool_use"]
    if not tool:
        print("")
    else:
        payload = json.dumps(tool[-1].get("tool_input"), sort_keys=True, ensure_ascii=False)
        print(hashlib.md5(payload.encode()).hexdigest())
except Exception:
    print("")
PY
REP_LIMITE=${CP_VIGIA_REP:-10}

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
      hangar-send --tmux "$ARB" "[vigia] Nao consigo ler /api/sessions ha 5 minutos. Enquanto isso eu NAO estou vigiando ninguem — confira o backend e me rearme." >/dev/null 2>&1
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

  # POR SESSÃO: qualquer uma do par parada por LIMITE leituras seguidas avisa SOZINHA, sem esperar
  # o time inteiro parar. O disparo coletivo abaixo ("ninguém está com a bola") existe para o tubo
  # em deadlock, e ele NUNCA fecha enquanto o árbitro trabalha — foi assim que, em 17/08/2026, as
  # três executoras morreram juntas num timeout do provedor às 12:41 e ninguém foi avisado: o
  # árbitro estava de bola cheia, então `quieto` nunca virou 1. O usuário resumiu o requisito real:
  # "se alguma sessão parar e ficar parada por muito tempo tem que avisar".
  # Re-avisa a cada LIMITE minutos enquanto continuar parada (o contador zera ao avisar).
  ULT=$(( ${#SESSOES[@]} - 1 ))
  for k in "${!SESSOES[@]}"; do
    [ "$k" -eq "$ULT" ] && continue          # o árbitro é o último; parado é o normal dele
    case "${ESTADOS[$k]:-?}" in
      idle|awaiting_input|sumiu|semcota|travado) PSEQ[$k]=$(( ${PSEQ[$k]:-0} + 1 )) ;;
      *) PSEQ[$k]=0; NUDGE[$k]=0 ;;
    esac
    if [ "${PSEQ[$k]:-0}" -ge "$LIMITE" ]; then
      # PRIMEIRO cutuca a propria sessao, DEPOIS avisa o arbitro. A ordem importa: o caso mais
      # comum (medido 17/08/2026) e turno morto por timeout do provedor com as 3 tentativas
      # estouradas — o Pi nao retenta sozinho e a sessao fica viva, parada, ate alguem digitar
      # nela. Um empurrao resolve isso sem ninguem acordar. So avisar o arbitro nao resolvia:
      # ele tambem pode estar caido, e ai o usuario e que vinha olhar.
      # Cutuca UMA vez por parada (nudge=1) e segue avisando a cada LIMITE min enquanto durar.
      if [ "${NUDGE[$k]:-0}" -eq 0 ] && [ "${ESTADOS[$k]:-?}" != "sumiu" ] && [ "${ESTADOS[$k]:-?}" != "semcota" ]; then
        hangar-send --tmux "${SESSOES[$k]}" "[vigia] Voce esta parada ha ${LIMITE} min sem reportar. Se o seu ultimo turno morreu (timeout do provedor, tentativas estouradas, conexao cortada), CONTINUE de onde parou, sem recomecar e sem refazer o que ja estava feito. Se voce ja entregou e esta esperando veredito, ignore esta mensagem. Se voce esta travada esperando alguma coisa do arbitro, diga em uma linha o que e." >/dev/null 2>&1
        NUDGE[$k]=1
        cutucada=" — CUTUQUEI ela agora (1a vez); se nao voltar, o turno nao morreu, ela esta travada de verdade"
      else
        cutucada=" — ja cutucada nesta parada; ela NAO voltou sozinha"
      fi
      msg="[vigia] ${SESSOES[$k]} esta parada (${ESTADOS[$k]:-?}) ha ${LIMITE} min${cutucada}. Time: $resumo. Olhe o PANE dela: timeout de provedor com as tentativas estouradas, turno morto e reporte preso na fila nao se desfazem sozinhos."
      echo "$msg"
      hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
      PSEQ[$k]=0
    fi
  done

  # LOOP: sessão do par em `working` com o MESMO comando por REP_LIMITE leituras. O sensor de
  # ociosidade nunca pega isso (polling produz evento a cada poucos segundos e parece trabalho);
  # medido em 17/08/2026: 1.231 execuções do mesmo comando em 3h, `working` o tempo todo, e quem
  # percebeu foi o usuário. Um curl de cauda por sessão trabalhando, por ciclo — barato.
  for k in "${!SESSOES[@]}"; do
    [ "$k" -eq "$ULT" ] && continue
    if [ "${ESTADOS[$k]:-?}" = "working" ]; then
      h=$(curl -s --config "$CURLRC" "$BASE/api/sessions/${SESSOES[$k]}/history?limit=3" \
          | python3 "$LOOPDET" 2>>"${CP_VIGIA_LOG:-/dev/stderr}")
      if [ -n "$h" ] && [ "$h" = "${RHASH[$k]:-}" ]; then
        RSEQ[$k]=$(( ${RSEQ[$k]:-0} + 1 ))
      else
        RSEQ[$k]=0; RAVISO[$k]=0
      fi
      RHASH[$k]=$h
      if [ "${RSEQ[$k]:-0}" -ge "$REP_LIMITE" ] && [ "${RAVISO[$k]:-0}" -eq 0 ]; then
        msg="[vigia] ${SESSOES[$k]} PODE estar em loop: diz working mas o ultimo comando e o MESMO ha ${RSEQ[$k]} leituras (~${RSEQ[$k]} min). Olhe o pane antes de decidir — polling de espera nao e trabalho, mas trabalho longo tambem repete comando. Quem manda parar e voce, depois de olhar. Time: $resumo"
        echo "$msg"
        # Pergunta, nunca ordem: a vigia le dois numeros e nao sabe se a sessao esta travada ou
        # trabalhando. Medido 24-28/08/2026: 3 alarmes falsos num dia, um mandou PARAR no meio de
        # 44 min de trabalho legitimo. Ordem de parar vem do arbitro, depois de olhar.
        hangar-send --tmux "${SESSOES[$k]}" "[vigia] Voce repete o MESMO comando ha ~${RSEQ[$k]} min. Isso e espera por condicao externa? Se for, o teto ja estourou: reporte ao arbitro o que voce espera e o ultimo retorno (regra do executor.md). Se voce esta trabalhando, ignore este aviso." >/dev/null 2>&1
        hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
        RAVISO[$k]=1
      fi
    else
      RSEQ[$k]=0; RHASH[$k]=""; RAVISO[$k]=0
    fi
  done

  # DIÁRIO parado: registro do árbitro é rede da retrospectiva; >60min sem escrita com o grupo
  # ativo é o árbitro trabalhando sem deixar rastro (medido: 6h45 sem uma linha). Re-avisa por hora.
  if [ -n "$DIARIO" ] && [ -f "$DIARIO" ]; then
    # O eventos.jsonl é irmão do registro no mesmo diretório, e parado durante trabalho é a MESMA
    # falha — então a cobrança olha o MAIS VELHO dos dois, e nomeia o que parou.
    parado=$DIARIO
    mtime=$(stat -c %Y "$DIARIO" 2>/dev/null || echo 0)
    eventos="$(dirname "$DIARIO")/eventos.jsonl"
    if [ -f "$eventos" ]; then
      mtime_ev=$(stat -c %Y "$eventos" 2>/dev/null || echo 0)
      if [ "$mtime_ev" -lt "$mtime" ]; then parado=$eventos; mtime=$mtime_ev; fi
    fi
    idade=$(( $(date +%s) - mtime ))
    if [ "$idade" -ge 3600 ] && [ "$diario_avisado" -lt "$(( idade / 3600 ))" ]; then
      diario_avisado=$(( idade / 3600 ))
      hangar-send --tmux "$ARB" "[vigia] O rastro ($parado) esta ha $(( idade / 60 ))min sem uma escrita, com o grupo ativo. Registro e eventos.jsonl se escrevem NO EVENTO — se pareceres/merges aconteceram nesse intervalo, eles estao fora do rastro." >/dev/null 2>&1
      echo "[vigia] rastro parado ha $(( idade / 60 ))min ($parado)"
    fi
    [ "$idade" -lt 3600 ] && diario_avisado=0
  fi

  # Sessão TRAVADA no par avisa na hora, sem esperar os três pararem: o árbitro está de bola cheia
  # justamente porque acha que o outro está trabalhando. Foi o caso de 14/08 — 1h17 de fila parada
  # com todo mundo achando que a Task andava.
  case "$par_estados" in
    *travado*)
      if [ "$avisou_travado" != "$par_estados" ]; then
        msg="[vigia] Sessao TRAVADA: $resumo. Diz 'working' mas nao produz evento ha mais de 10 minutos — o caso classico e um picker/AskUserQuestion bloqueando o turno de quem disparou. Olhe o pane e destrave (POST /api/sessions/<nome>/select com {\"option\": N})."
        echo "$msg"
        hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
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
        hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
        avisou_cota="$par_estados"
      fi
      ;;
  esac

  if [ "$quieto" -eq 1 ]; then parados=$((parados+1)); else parados=0; fi

  if [ "$parados" -ge "$LIMITE" ]; then
    msg="[vigia] Ninguem esta com a bola ha ${LIMITE} min: $resumo (minuto $i). Se voce caiu (erro de API), isto e o que te traz de volta. Cheque se alguem entregou enquanto voce estava fora — relato preso na fila e veredito parado sao os dois jeitos de o tubo travar sem ninguem perceber."
    echo "$msg"
    hangar-send --tmux "$ARB" "$msg" >/dev/null 2>&1
    avisos=$((avisos+1))
    parados=0
    if [ "$avisos" -ge 20 ]; then
      echo "20 avisos sem destravar; encerrando vigia"
      exit 0
    fi
  fi
done
echo "1440min encerrados; ultimo estado: $resumo"
