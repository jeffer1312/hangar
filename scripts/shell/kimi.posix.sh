# hangar — `kimi` wrapper (bash + zsh). Sourced from your rc by
# scripts/install-claude-wrapper.sh. Fish has its own version: scripts/shell/kimi.fish
#
# Makes every interactive `kimi` trackable by the hangar app by running it INSIDE tmux (the app
# only lists tmux sessions; a kimi started outside tmux is invisible to it).
#
# SIMPLER than the pi wrapper ON PURPOSE: the Kimi CLI has no `--session-id` flag (the session id
# is born inside, at the first prompt), so there is no id to inject and no env var to export. The
# pane<->session link is the ticket the backend's kimi hook writes (it inherits TMUX_PANE from the
# pane) — see backend/hooks/kimi_state_hook.py and registry.kimi_session_file.
#
# Rules:
#  - subcommand as the FIRST argument -> raw (export/provider/acp/web/server/login/doctor/vis/
#    migrate/upgrade/update). Matched on $1 only, so `kimi "export the function"` stays an
#    interactive launch with an initial prompt. Same rule as the pi/codex wrappers.
#  - a flag that makes kimi print and exit anywhere in the args -> raw: -p/--prompt,
#    --output-format, --help/-h, --version/-V. Wrapping these would open a TUI that never prints.
#  - resume flags (-S/--session, -c/--continue) are NOT special here: there's nothing to inject,
#    so they take the normal path — the tmux wrap keeps the resumed session visible in the app and
#    the hook ticket carries the REAL session id regardless.
#  - already in tmux ($TMUX) / stdin not a tty (pipe/script) -> raw: nothing to do.
#  - outside tmux + interactive -> create a tmux session named after the folder BASENAME (suffix
#    -2/-3 if it already exists) and run kimi inside it. Quitting kimi ends the command, so the
#    tmux session dies and disappears from the app.
#
# Escape hatch: `command kimi ...` runs the raw binary, bypassing this wrapper.
kimi() {
    local a
    # subcomando: só vale como PRIMEIRO argumento (`kimi export x` é subcomando; `kimi "export x"` é prompt).
    case "${1:-}" in
        export|provider|acp|web|server|login|doctor|vis|migrate|upgrade|update)
            command kimi "$@"
            return
            ;;
    esac

    # usos não interativos primeiro: o kimi imprime e sai, não tem TUI pra envolver em tmux.
    for a in "$@"; do
        case "$a" in
            -p|--prompt|--prompt=*|--output-format|--output-format=*|--help|-h|--version|-V)
                command kimi "$@"
                return
                ;;
        esac
    done

    # Daqui pra baixo abre TUI: refaz a ponte de skills agora, pra skill instalada no Claude valer
    # já nesta abertura (antes dependia de reiniciar o backend). Silencioso e fail-soft.
    command -v hangar-skills-sync >/dev/null 2>&1 && hangar-skills-sync

    # TMUX herdado pode estar MORTO (mesmo caso do wrapper claude/pi: kitty single-instance cujo
    # mestre nasceu dentro de um pane que já fechou). Valida o pane; stale -> limpa e segue pro
    # caminho "fora do tmux" (cria sessão nova).
    if [ -n "${TMUX:-}" ]; then
        if [ -z "${TMUX_PANE:-}" ] || ! tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1; then
            unset TMUX TMUX_PANE
        fi
    fi

    # já dentro de tmux, ou stdin não é um tty (pipe/script): nada a fazer — o bilhete do hook
    # liga o pane à sessão sem nenhuma variável nossa.
    if [ -n "${TMUX:-}" ] || [ ! -t 0 ]; then
        command kimi "$@"
        return
    fi

    # outside tmux + interactive: tmux session named after the folder basename, unique.
    local base name i ascii
    base=$(basename "$PWD")
    # Acento vira o ASCII equivalente ANTES do filtro — mesma regra do backend (app/names.py) e dos
    # wrappers claude/pi.
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
    # wrappers claude/pi: o gerenciador systemd do usuario pode recusar scope transiente).
    local run=()
    if command -v systemd-run >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ] \
       && systemd-run --user --scope --collect -q -- true >/dev/null 2>&1; then
        run=(systemd-run --user --scope --collect -q --)
    fi
    # `sh -c` + exec mantém a MESMA forma dos outros wrappers. CP_SESSION_NAME: carimbo de
    # identidade pro hangar-send de dentro do pane (ver claude.posix.sh).
    # KIMI_CODE_TUI_FULL_SCREEN=1: fullscreen TUI experimental do Kimi 0.36+ (scroll proprio,
    # composer fixo). O -e do backend (app/tmux.py) cobre sessoes criadas pelo app; este cobre as
    # abertas pelo terminal. Caminho "ja dentro de tmux -> raw" nao passa aqui: quem cobre e o
    # export no rc do shell.
    "${run[@]}" tmux new-session -s "$name" -c "$PWD" -e "CP_SESSION_NAME=$name" \
        -e "KIMI_CODE_TUI_FULL_SCREEN=1" \
        sh -c 'exec kimi "$@"' _ "$@"
}
