# claude-pocket — `claude` wrapper (bash + zsh). Sourced from your rc by
# scripts/install-claude-wrapper.sh. Fish has its own version: scripts/shell/claude.fish
#
# Makes every interactive `claude` trackable by the claude-pocket app:
#  1. injects a unique --session-id  -> the backend binds the exact transcript (.jsonl), so two
#     claudes in the SAME folder never leak into / overwrite each other.
#  2. runs INSIDE tmux               -> the app only lists tmux sessions; a claude started outside
#     tmux is invisible to the app.
#
# Rules:
#  - already passed --session-id/--resume/-c/--continue -> respected, untouched (the CLI rejects
#     --session-id combined with --resume/--continue).
#  - already in tmux ($TMUX) / -p / --print / stdin not a tty (pipe/script) -> only inject the id.
#  - outside tmux + interactive            -> create a tmux session named after the folder BASENAME
#     (suffix -2/-3 if it already exists) and run claude (with the id) inside it. Quitting claude
#     ends the command, so the tmux session dies and disappears from the app.
#
# COLORTERM + CLAUDE_CODE_TMUX_TRUECOLOR keep Claude's theme 24-bit inside tmux (see
# docs/tmux-truecolor-setup.md). The tmux server goes in its own systemd scope so closing the
# terminal that spawned it doesn't kill every session (same fix as backend/app/tmux.py).
#
# Escape hatch: `command claude ...` runs the raw binary, bypassing this wrapper.
claude() {
    # Motor de modelo (CP_ENGINE, setado por claude-engine): env aplicado DENTRO do pane por
    # `cp-engine --exec`, não por `tmux -e` — tmux não herda o env do caller, e `-e TOKEN=…` deixaria
    # a key em /proc/<pid>/cmdline, legível por qualquer usuário. Array em vez de string para não
    # depender de word-splitting (zsh não faz em variável não-quotada). Construído ANTES do scan de
    # flags abaixo porque -c/--resume/--session-id saem por um early return e também precisam do
    # prefixo — senão o motor é silenciosamente ignorado e a sessão sobe na conta Anthropic.
    local -a pre
    pre=()
    if [ -n "${CP_ENGINE:-}" ]; then
        pre=(cp-engine --exec "$CP_ENGINE" --)
    fi

    local a
    # respect flags that manage their own session (injecting --session-id alongside them errors)
    for a in "$@"; do
        case "$a" in
            --session-id|--session-id=*|--resume|--resume=*|-c|--continue)
                # mesmo bypass de `command` do caminho "só injeta o id" abaixo: sem motor, chama o
                # binário direto (evita recursão na função); com motor, quem executa é o cp-engine
                # (processo à parte achado no PATH) — `command` não existiria pra ele executar.
                if [ ${#pre[@]} -eq 0 ]; then
                    command claude "$@"
                else
                    "${pre[@]}" claude "$@"
                fi
                return
                ;;
        esac
    done

    local id
    id=$(uuidgen 2>/dev/null) || id=$(cat /proc/sys/kernel/random/uuid)

    # only inject the id (no tmux) when: already in tmux, print mode, or stdin not a tty
    local print=0
    for a in "$@"; do case "$a" in -p|--print) print=1 ;; esac; done
    # TMUX herdado pode estar MORTO (ex: kitty single-instance cujo mestre nasceu dentro de um pane
    # que já fechou). Valida o pane; stale -> limpa e segue pro caminho "fora do tmux" (cria sessão).
    if [ -n "${TMUX:-}" ]; then
        # list-panes -t <pane>: exit 1 se o pane não existe (display-message devolve 0 até pra pane morto).
        if [ -z "${TMUX_PANE:-}" ] || ! tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1; then
            unset TMUX TMUX_PANE
        fi
    fi

    if [ -n "${TMUX:-}" ] || [ "$print" = 1 ] || [ ! -t 0 ]; then
        # `command` só faz sentido quando o QUEM roda é este próprio shell: bypassa a função `claude`
        # para não recursar nela mesma. Com $pre setado, quem executa é o cp-engine (um processo
        # separado, achado no PATH) — o execvpe dele nunca vê função de shell, então "command" viraria
        # só uma string a mais no argv (e um alvo inexistente pro execvpe procurar).
        if [ ${#pre[@]} -eq 0 ]; then
            COLORTERM=truecolor CLAUDE_CODE_TMUX_TRUECOLOR=1 command claude --session-id "$id" "$@"
        else
            COLORTERM=truecolor CLAUDE_CODE_TMUX_TRUECOLOR=1 "${pre[@]}" claude --session-id "$id" "$@"
        fi
        return
    fi

    # outside tmux + interactive: tmux session named after the folder basename, unique.
    local base name i ascii
    base=$(basename "$PWD")
    # Acento vira o ASCII equivalente ANTES do filtro — mesma regra do backend (app/names.py).
    # Sem isto "Área de Trabalho" perdia a 1a letra: o tr trabalha em BYTES, o "Á" (2 bytes) virava
    # "--", e o strip de UM traco so deixava "-rea-de-Trabalho" (nome de sessao tmux comecando com
    # "-" e pedir encrenca). iconv ausente ou sem //TRANSLIT (BSD/macOS) -> segue com o nome cru,
    # degradando pro comportamento antigo em vez de falhar.
    if command -v iconv >/dev/null 2>&1; then
        ascii=$(printf '%s' "$base" | iconv -f UTF-8 -t ASCII//TRANSLIT 2>/dev/null) \
            && [ -n "$ascii" ] && base=$ascii
    fi
    base=$(printf '%s' "$base" | tr -c 'A-Za-z0-9_-' '-')
    # TODOS os traços das pontas (o ${base%-}/${base#-} tirava so um).
    while [ "${base#-}" != "$base" ]; do base=${base#-}; done
    while [ "${base%-}" != "$base" ]; do base=${base%-}; done
    [ -n "$base" ] || base=session
    name=$base; i=2
    while tmux has-session -t "=$name" 2>/dev/null; do
        name="$base-$i"; i=$((i + 1))
    done

    # duplicated call: zsh doesn't word-split an unquoted prefix var, so no $run trick here.
    # O `command -v` diz que o BINARIO existe, nao que ele FUNCIONA: o gerenciador systemd do usuario
    # pode recusar criar scope transiente ("Failed to start transient scope unit"), e ai o `claude`
    # nao abre. Medido nesta maquina: 5/5 falhas com binario e gerenciador na mesma versao. O probe
    # (um fork) transforma "nao abre" em "abre sem scope proprio".
    # CP_SESSION_NAME: mesmo carimbo de identidade que o backend poe em new_session (app/tmux.py).
    # Sem ele o cp-send de dentro desta sessao cai no `tmux display-message -p '#S'`, que devolve a
    # sessao do CLIENTE anexado e nao a de quem chama — o `--unpair` de uma sessao desfazia o vinculo
    # da OUTRA. Sessao aberta no terminal e criada AQUI, nao pelo backend, entao o carimbo tem que
    # sair daqui tambem, senao o bug fica vivo justamente no caminho mais usado.
    if command -v systemd-run >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ] \
       && systemd-run --user --scope --collect -q -- true >/dev/null 2>&1; then
        systemd-run --user --scope --collect -q -- tmux new-session -s "$name" -c "$PWD" \
            -e COLORTERM=truecolor -e CLAUDE_CODE_TMUX_TRUECOLOR=1 \
            -e "CP_SESSION_NAME=$name" \
            "${pre[@]}" claude --session-id "$id" "$@"
    else
        tmux new-session -s "$name" -c "$PWD" \
            -e COLORTERM=truecolor -e CLAUDE_CODE_TMUX_TRUECOLOR=1 \
            -e "CP_SESSION_NAME=$name" \
            "${pre[@]}" claude --session-id "$id" "$@"
    fi
}
