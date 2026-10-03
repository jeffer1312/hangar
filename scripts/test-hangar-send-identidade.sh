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
  "list-clients -F")
      [[ "${CLIENTES_RC:-0}" == 0 ]] || exit "$CLIENTES_RC"
      printf '%s\n' "${CLIENTES_FAKE:-}" ;;
  *) exit 1 ;;
esac
FAKE
chmod +x "$TMP/bin/tmux"
export PATH="$TMP/bin:$PATH"
source "$TMP/me.sh"
unset CP_SESSION_KEY HANGAR_CANO_KEY HANGAR_HEADLESS_DIR

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
export CLIENTES_FAKE=$'0\toutra'
checa "degradação final" "outra" "$(me 2>/dev/null)"
if me 2>&1 >/dev/null | grep -q "identidade caiu"; then
    echo "ok   degradação avisa no stderr"
else
    echo "FALHA degradação silenciosa — o dono não fica sabendo"; falhas=$((falhas + 1))
fi

export SESSAO_ATUAL="observada" CLIENTES_FAKE=$'1\tobservada'
checa "observador não assina recado" "cli" "$(me 2>/dev/null)"
export CLIENTES_FAKE=$'1\tobservada\n0\thumana'
checa "humano vence observador" "humana" "$(me 2>/dev/null)"
export CLIENTES_FAKE=$'0\thumana\n0\thumana'
checa "dois clientes da mesma sessão" "humana" "$(me 2>/dev/null)"
export CLIENTES_FAKE=$'0\thumana\n0\toutra'
checa "clientes de sessões distintas são ambíguos" "cli" "$(me 2>/dev/null)"
export CLIENTES_FAKE=$'0\tnome com espaco'
checa "fallback preserva espaço" "nome com espaco" "$(me 2>/dev/null)"
export CLIENTES_FAKE=$'\thumana'
checa "modo desconhecido não fornece identidade" "cli" "$(me 2>/dev/null)"
export CLIENTES_FAKE=$'0\thumana' CLIENTES_RC=1
checa "consulta falha não fornece identidade" "cli" "$(me 2>/dev/null)"
unset CLIENTES_RC
export CLIENTES_FAKE='/dev/pts/0: observada: powershell [200x50] (utf8)'
checa "psmux sem formato não fornece identidade" "cli" "$(me 2>/dev/null)"

# 5) Sessão SEM terminal: a chave do sidecar vence tudo — inclusive um pane herdado do
#    operador que subiu o backend de dentro de um tmux, e um carimbo obsoleto pós-rename.
mkdir -p "$TMP/headless"
printf '{"name": "renomeada", "provider": "claude", "key": "abc123", "session_id": "x"}\n' > "$TMP/headless/renomeada.json"
export HANGAR_HEADLESS_DIR="$TMP/headless" CP_SESSION_KEY="abc123" CP_SESSION_NAME="nome-do-nascimento"
export TMUX=1 TMUX_PANE="%1" PANES_FAKE=$'%1 outra' SESSAO_ATUAL="outra"
checa "sem terminal (chave do sidecar)" "renomeada" "$(me 2>/dev/null)"

# 6) Codex sem terminal usa o sidecar próprio, sem cair no tmux.
mkdir -p "$TMP/home/.hangar/codex-sessions"
printf '{"name": "codex-headless", "provider": "codex", "key": "cx123", "headless": true}\n' > "$TMP/home/.hangar/codex-sessions/codex-headless.json"
unset HANGAR_HEADLESS_DIR
HOME_ANTIGO=$HOME
HOME="$TMP/home" HANGAR_CANO_KEY="cx123" checa "codex sem terminal" "codex-headless" "$(HOME="$TMP/home" CP_SESSION_KEY="" HANGAR_CANO_KEY="cx123" me 2>/dev/null)"
HOME=$HOME_ANTIGO

# 7) Chave sem sidecar (sessão encerrada e arquivo apagado): segue pros passos de sempre.
export CP_SESSION_KEY="nao-existe"
checa "chave órfã cai no pane" "outra" "$(me 2>/dev/null)"
unset CP_SESSION_KEY HANGAR_CANO_KEY HANGAR_HEADLESS_DIR

# 8) `--whoami` do script de verdade: imprime a identidade e sai sem chamar a API (curl falso que falha).
mkdir -p "$TMP/scripts" "$TMP/backend" "$TMP/claude-headless"
cp "$REPO/scripts/hangar-send" "$TMP/scripts/hangar-send"
echo "CP_AUTH_TOKEN=x" > "$TMP/backend/.env"
printf '{"name": "w-t4", "provider": "claude", "key": "k1"}\n' > "$TMP/claude-headless/t.json"
printf '#!/usr/bin/env bash\necho chamou-api >> "%s/api.log"\nexit 1\n' "$TMP" > "$TMP/bin/curl"
chmod +x "$TMP/bin/curl"
quem=$(CP_SESSION_KEY=k1 HANGAR_HEADLESS_DIR="$TMP/claude-headless" "$TMP/scripts/hangar-send" --whoami 2>/dev/null)
checa "--whoami imprime a identidade" "w-t4" "$quem"
checa "--whoami sem chamada à API" "nao" "$([[ -e "$TMP/api.log" ]] && echo sim || echo nao)"

echo
if (( falhas )); then echo "$falhas falha(s)"; exit 1; fi
echo "tudo ok"
