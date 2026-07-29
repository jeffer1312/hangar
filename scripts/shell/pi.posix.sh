# claude-cockpit — `pi` wrapper (bash + zsh). Sourced from your rc by
# scripts/install-claude-wrapper.sh. Fish has its own version: scripts/shell/pi.fish
#
# Makes every interactive `pi` trackable by the claude-cockpit app:
#  1. injects a unique --session-id  -> CP_PI_SESSION carries the SAME uuid, exported into pi's own
#     environment. pi rewrites its own argv, so --session-id disappears from /proc/<pid>/cmdline —
#     the backend reads CP_PI_SESSION from /proc/<pid>/environ instead (registry.py:_pi_sid_of).
#  2. runs INSIDE tmux               -> the app only lists tmux sessions; a pi started outside tmux
#     is invisible to the app.
#
# Rules:
#  - already passed a flag that manages its own session state -> respected, untouched. Per
#    `pi --help`: --session-id, --resume/-r, --continue/-c, --session (path|id), --fork (path|id),
#    --no-session. Same class of bug as the claude wrapper guards against (-c there is Claude's own
#    "continue previous session" short flag) — injecting --session-id alongside any of these either
#    errors or silently overrides what the user asked for (e.g. `pi -c` would start a FRESH random
#    session instead of continuing the last one).
#  - not an interactive session at all -> raw, exactly like the codex wrapper leaves its subcommands
#    alone. Two shapes: (a) the FIRST argument is one of pi's subcommands (install/remove/uninstall/
#    update/list/config) — matched on the first argument only, so `pi "remove the dead code"` stays an
#    interactive launch with an initial prompt; (b) a flag that makes pi print and exit anywhere in the
#    args (-p/--print, --mode, --list-models, --export, --help/-h, --version/-v). Wrapping these was
#    the bug: `pi remove npm:foo` opened the TUI and never removed anything.
#  - already in tmux ($TMUX) / stdin not a tty (pipe/script) -> only inject the id + export the var.
#  - outside tmux + interactive -> create a tmux session named after the folder BASENAME (suffix
#     -2/-3 if it already exists) and run pi (with the id) inside it. Quitting pi ends the command,
#     so the tmux session dies and disappears from the app.
#
# CP_PI_SESSION is EXPORTED, never passed via `tmux -e`: a tmux server that is already running does
# NOT hand its client's environment to a brand-new pane (measured — an `export` before `tmux
# new-session` never reaches the pane when a server is already up), so the export has to happen
# INSIDE the pane's own process. That's why the "create a new tmux session" branch below runs a tiny
# `sh -c` that exports CP_PI_SESSION and only then execs pi, instead of relying on ambient
# inheritance or on `-e` (same reasoning as CP_ENGINE in CLAUDE.md — keeps one shape for both
# wrappers, and here `-e` would put the uuid in the tmux client's own /proc/<pid>/cmdline for no
# reason since the export-then-exec shape is just as cheap).
#
# Escape hatch: `command pi ...` runs the raw binary, bypassing this wrapper.
pi() {
    local a
    # subcomando: só vale como PRIMEIRO argumento (`pi remove x` é subcomando; `pi "remove x"` é prompt).
    case "${1:-}" in
        install|remove|uninstall|update|list|config)
            command pi "$@"
            return
            ;;
    esac

    for a in "$@"; do
        case "$a" in
            # flags que gerenciam a própria sessão (injetar --session-id junto erra ou sobrescreve)
            --session-id|--session-id=*|--resume|--resume=*|-r|--continue|-c|--session|--session=*|--fork|--fork=*|--no-session)
                command pi "$@"
                return
                ;;
            # usos não interativos: o pi imprime e sai, não tem TUI pra envolver em tmux
            -p|--print|--mode|--mode=*|--list-models|--list-models=*|--export|--export=*|--help|-h|--version|-v)
                command pi "$@"
                return
                ;;
        esac
    done

    local id
    id=$(uuidgen 2>/dev/null) || id=$(cat /proc/sys/kernel/random/uuid)

    # TMUX herdado pode estar MORTO (mesmo caso do wrapper claude: kitty single-instance cujo mestre
    # nasceu dentro de um pane que já fechou). Valida o pane; stale -> limpa e segue pro caminho
    # "fora do tmux" (cria sessão nova).
    if [ -n "${TMUX:-}" ]; then
        if [ -z "${TMUX_PANE:-}" ] || ! tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1; then
            unset TMUX TMUX_PANE
        fi
    fi

    # só injeta o id (sem tmux novo) quando: já dentro de tmux, ou stdin não é um tty (pipe/script).
    if [ -n "${TMUX:-}" ] || [ ! -t 0 ]; then
        export CP_PI_SESSION="$id"
        command pi --session-id "$id" "$@"
        return
    fi

    # outside tmux + interactive: tmux session named after the folder basename, unique.
    local base name i ascii
    base=$(basename "$PWD")
    # Acento vira o ASCII equivalente ANTES do filtro — mesma regra do backend (app/names.py) e do
    # wrapper claude.
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

    # `command -v systemd-run` diz que o BINARIO existe, nao que ele FUNCIONA (mesmo probe do
    # wrapper claude: gerenciador systemd do usuario pode recusar scope transiente).
    local run=()
    if command -v systemd-run >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ] \
       && systemd-run --user --scope --collect -q -- true >/dev/null 2>&1; then
        run=(systemd-run --user --scope --collect -q --)
    fi
    # `sh -c` exporta CP_PI_SESSION DENTRO do próprio pane antes do exec pi — ver o comentário de
    # cabeçalho pra o porquê. $0 vira "_" (placeholder), $1 o uuid, o resto ($@) os args originais.
    # CP_SESSION_NAME: carimbo de identidade pro cp-send de dentro do pane (ver claude.posix.sh).
    "${run[@]}" tmux new-session -s "$name" -c "$PWD" -e "CP_SESSION_NAME=$name" \
        sh -c 'export CP_PI_SESSION="$1"; shift; exec pi --session-id "$CP_PI_SESSION" "$@"' _ "$id" "$@"
}
