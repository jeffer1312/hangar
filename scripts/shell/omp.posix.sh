# hangar — `omp` wrapper (bash + zsh). Sourced from your rc by
# scripts/install-claude-wrapper.sh. Fish has its own version: scripts/shell/omp.fish
#
# Mesmo papel do pi.posix.sh, com UMA diferença que muda a forma: o omp NÃO tem `--session-id`.
# Quem escolhe a sessão é o CAMINHO do transcript (`--session <arquivo>`), então o wrapper monta o
# caminho igual ao backend (`backend/app/adapters/omp/adapter.py` -> `pi/sessions.transcript_alvo`)
# e exporta CP_PI_SESSION com o MESMO uuid do nome do arquivo — é por ele que o backend acha a
# sessão em /proc/<pid>/environ quando o bilhete da extensão ainda não existe (registry:_pi_sid_of).
#
# Regras (idênticas às do pi):
#  - flag que já gerencia a própria sessão (--session, --resume/-r, --continue/-c, --no-session,
#    --session-dir) -> NÃO monta caminho nenhum (sobrepor `--session` a um `omp -c` abriria sessão
#    FRESCA em vez de continuar), mas AINDA envolve em tmux quando interativo e fora dele: sessão
#    retomada tem que continuar visível no app, e o rastreio não depende do uuid injetado — a
#    hangar-state.ts publica o arquivo real por dentro do omp.
#  - uso não interativo -> cru. Duas formas: (a) o PRIMEIRO argumento é um subcomando do omp
#    (`omp models` lista modelos; `omp "models are slow"` continua sendo prompt inicial);
#    (b) uma flag que imprime e sai (-p/--print, --export, --help/-h, --version/-v).
#  - já em tmux ($TMUX) / stdin não é tty -> só o --session + o export.
#  - fora do tmux + interativo -> cria sessão tmux com o BASENAME da pasta (sufixo -2/-3 se já
#    existir) e roda o omp dentro dela.
#
# CP_PI_SESSION é EXPORTADO, nunca passado por `tmux -e`: um servidor tmux já rodando não entrega o
# ambiente do cliente a um pane novo, então o export tem que acontecer DENTRO do processo do pane —
# daí o `sh -c` do ramo que cria sessão (mesmo desenho do pi.posix.sh).
#
# Escape hatch: `command omp ...` roda o binário cru, sem este wrapper.

# Mesmo caminho que backend/app/adapters/pi/sessions.py:transcript_alvo monta pro `--session`:
# <raiz>/<slug do cwd>/<ts>_<id>.jsonl. O slug troca só o separador por '-' (acento e espaço passam).
hangar_omp_alvo() {  # $1 = uuid
    local raiz="${PI_CODING_AGENT_DIR:-$HOME/.omp/agent}/sessions"
    local slug="--$(printf '%s' "$PWD" | sed -e 's#^[/\\]##' -e 's#[/\\:]#-#g')--"
    printf '%s/%s/%s_%s.jsonl' "$raiz" "$slug" "$(date -u +%Y-%m-%dT%H-%M-%S-000Z)" "$1"
}

omp() {
    local a
    # subcomando: só vale como PRIMEIRO argumento (lista de `omp --help`, seção COMMANDS).
    case "${1:-}" in
        acp|agents|auth-broker|auth-gateway|bench|browser-relay|cleanse|commit|completions|compress|\
config|dry-balance|gallery|gc|git|grep|grievances|if-bench|images|install|join|models|plugin|ps|\
read|render|say|search|setup|share|shell|ssh|stats|tiny-models|token|ttsr|update|usage|worktree)
            command omp "$@"
            return
            ;;
    esac

    # usos não interativos: o omp imprime e sai, não tem TUI pra envolver em tmux.
    for a in "$@"; do
        case "$a" in
            -p|--print|--export|--export=*|--help|-h|--version|-v)
                command omp "$@"
                return
                ;;
        esac
    done

    local own_session=0
    for a in "$@"; do
        case "$a" in
            --session|--session=*|--resume|--resume=*|-r|--continue|-c|--no-session|--session-dir|--session-dir=*)
                own_session=1
                break
                ;;
        esac
    done

    local id
    id=$(uuidgen 2>/dev/null) || id=$(cat /proc/sys/kernel/random/uuid)

    # TMUX herdado pode estar MORTO (kitty single-instance cujo mestre nasceu num pane já fechado).
    # Valida o pane; stale -> limpa e segue pro caminho "fora do tmux".
    if [ -n "${TMUX:-}" ]; then
        if [ -z "${TMUX_PANE:-}" ] || ! tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1; then
            unset TMUX TMUX_PANE
        fi
    fi

    if [ -n "${TMUX:-}" ] || [ ! -t 0 ]; then
        if [ "$own_session" = 1 ]; then
            command omp "$@"
            return
        fi
        export CP_PI_SESSION="$id"
        command omp --session "$(hangar_omp_alvo "$id")" "$@"
        return
    fi

    # fora do tmux + interativo: sessão tmux com o basename da pasta, único.
    local base name i ascii
    base=$(basename "$PWD")
    # Acento vira o ASCII equivalente ANTES do filtro — mesma regra do backend (app/names.py).
    if command -v iconv >/dev/null 2>&1; then
        ascii=$(printf '%s' "$base" | iconv -f UTF-8 -t ASCII//TRANSLIT 2>/dev/null) \
            && [ -n "$ascii" ] && base=$ascii
    fi
    base=$(printf '%s' "$base" | tr -c 'A-Za-z0-9_-' '-')
    while [ "${base#-}" != "$base" ]; do base=${base#-}; done
    while [ "${base%-}" != "$base" ]; do base=${base%-}; done
    [ -n "$base" ] || base=session
    name=$base; i=2
    while tmux has-session -t "=$name" 2>/dev/null; do
        name="$base-$i"; i=$((i + 1))
    done

    # `command -v systemd-run` diz que o BINARIO existe, nao que ele FUNCIONA (mesmo probe dos
    # outros wrappers: gerenciador systemd do usuario pode recusar scope transiente).
    local run=()
    if command -v systemd-run >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ] \
       && systemd-run --user --scope --collect -q -- true >/dev/null 2>&1; then
        run=(systemd-run --user --scope --collect -q --)
    fi
    # `sh -c` exporta CP_PI_SESSION DENTRO do pane antes do exec — ver o cabeçalho pra o porquê.
    # $0 vira "_" (placeholder), $1 o uuid, $2 o caminho do transcript, o resto os args originais.
    # CP_SESSION_NAME: carimbo de identidade pro hangar-send de dentro do pane (ver claude.posix.sh).
    if [ "$own_session" = 1 ]; then
        "${run[@]}" tmux new-session -s "$name" -c "$PWD" -e "CP_SESSION_NAME=$name" \
            sh -c 'exec omp "$@"' _ "$@"
    else
        "${run[@]}" tmux new-session -s "$name" -c "$PWD" -e "CP_SESSION_NAME=$name" \
            sh -c 'export CP_PI_SESSION="$1"; alvo="$2"; shift 2; exec omp --session "$alvo" "$@"' \
            _ "$id" "$(hangar_omp_alvo "$id")" "$@"
    fi
}
