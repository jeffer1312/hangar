#!/usr/bin/env bash
# Trava a resolução de identidade do hangar-send (a função `me`), com um `tmux` FALSO no PATH — mesmo
# padrão do test-wrappers.sh, e pelo mesmo motivo: é shell, a suíte pytest não alcança.
#
# Os dois bugs que isto encerra:
#
#   1. RENOMEAR MATAVA A IDENTIDADE. O nome era carimbado no env do pane no nascimento
#      (`new-session -e CP_SESSION_NAME=`), e env de processo já rodando não se reescreve — então
#      renomear pelo app (que é FEATURE) deixava o carimbo obsoleto na hora. O hangar-send caía no
#      `display-message -p '#S'`, que é a sessão do CLIENTE ANEXADO: estado GLOBAL do servidor, não
#      propriedade de quem pergunta. Medido em 11/08/2026: uma sessão transitória da suíte de testes
#      (`cp-test-termsock`) virou a "sessão corrente" por alguns segundos e TODA sessão que
#      perguntasse naquela janela recebia esse nome. Isso já tinha dissolvido um pareamento antes:
#      a sessão B rodou `--unpair`, se identificou como A, e o backend desfez o vínculo de A.
#
#   2. PANE ID PODE SER AMBÍGUO. O psmux (Windows) numera pane id POR SESSÃO, então `%1` existe em
#      várias. Resolver com `display-message -t "$TMUX_PANE"` devolveria UMA delas, calado — nome
#      errado é pior que nome ausente. Por isso a resolução CONTA as ocorrências e só aceita quando
#      há exatamente uma.
#
# Uso: ./scripts/test-hangar-send-identidade.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
falhas=0

# `me` sai do hangar-send de verdade: testar uma cópia colada aqui passaria com o arquivo real quebrado.
sed -n '/^me() {/,/^}/p' "$REPO/scripts/hangar-send" > "$TMP/me.sh"
[[ -s "$TMP/me.sh" ]] || { echo "FALHA: não achei a função me() em scripts/hangar-send"; exit 1; }

# tmux falso: as sessões vêm de PANES_FAKE ("<pane_id> <sessao>" por linha) e SESSAO_ATUAL.
mkdir -p "$TMP/bin"
cat > "$TMP/bin/tmux" <<'FAKE'
#!/usr/bin/env bash
case "$1 $2" in
  "list-panes -a") printf '%s\n' "$PANES_FAKE" ;;
  "has-session -t")
      # `=nome` é match exato; o teste guarda os nomes válidos em SESSOES_FAKE.
      alvo="${3#=}"
      grep -qx "$alvo" <<< "$SESSOES_FAKE" ;;
  "display-message -p") printf '%s\n' "$SESSAO_ATUAL" ;;
  *) exit 1 ;;
esac
FAKE
chmod +x "$TMP/bin/tmux"
export PATH="$TMP/bin:$PATH"
source "$TMP/me.sh"

checa() { # <caso> <esperado> <obtido>
    if [[ "$2" == "$3" ]]; then
        printf 'ok   %s -> %s\n' "$1" "$3"
    else
        printf 'FALHA %s -> esperava %q, veio %q\n' "$1" "$2" "$3"; falhas=$((falhas + 1))
    fi
}

export SESSOES_FAKE=$'nome-novo\noutra'

# 1) Sessão RENOMEADA: carimbo obsoleto, cliente anexado apontando pra outra. O pane decide.
export TMUX=1 TMUX_PANE="%2" CP_SESSION_NAME="nome-do-nascimento" SESSAO_ATUAL="outra"
export PANES_FAKE=$'%1 outra\n%2 nome-novo'
checa "renomeada (pane manda)" "nome-novo" "$(me 2>/dev/null)"

# 2) Nome de sessão COM ESPAÇO: o awk monta do 2o campo em diante, não trunca.
export TMUX_PANE="%3" PANES_FAKE=$'%3 nome com espaco'
checa "nome com espaço" "nome com espaco" "$(me 2>/dev/null)"

# 3) Pane AMBÍGUO (psmux): mesmo id em duas sessões -> não chuta, cai no carimbo válido.
export TMUX_PANE="%1" CP_SESSION_NAME="nome-novo" PANES_FAKE=$'%1 outra\n%1 nome-novo'
checa "pane ambíguo cai no carimbo" "nome-novo" "$(me 2>/dev/null)"

# 4) Sem pane E com carimbo obsoleto: último recurso é o cliente anexado, e tem que AVISAR.
export TMUX_PANE="" CP_SESSION_NAME="nome-do-nascimento" SESSAO_ATUAL="outra" PANES_FAKE=""
checa "degradação final" "outra" "$(me 2>/dev/null)"
if me 2>&1 >/dev/null | grep -q "identidade caiu"; then
    echo "ok   degradação avisa no stderr"
else
    echo "FALHA degradação silenciosa — o dono não fica sabendo"; falhas=$((falhas + 1))
fi

echo
if (( falhas )); then echo "$falhas falha(s)"; exit 1; fi
echo "tudo ok"
