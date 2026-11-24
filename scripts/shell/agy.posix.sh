# hangar — `agy` wrapper (bash + zsh). Sourced from your rc by
# scripts/install-claude-wrapper.sh. Fish has its own version: scripts/shell/agy.fish
#
# Makes every interactive `agy` opened in a terminal a Hangar session, same as the app opens.
#
# O agy cria o ID no primeiro prompt. O hook `hangar-server agy-state-hook` herda
# TMUX_PANE e grava o bilhete que liga o pane à sessão.
#
# Rules:
#  - subcommand as the FIRST argument -> raw (acp/agent/agents/changelog/help/install/mcp/
#    mic-serve/models/plugin/plugins/remote-control/update). Matched on $1 only, so `agy "update the docs"`
#    stays an interactive launch with an initial prompt. Same rule as the pi/codex wrappers.
#  - a flag that makes agy print and exit anywhere in the args -> raw: -p/--print/--prompt,
#    --output-format, --input-format, --json-schema, --help/-h, --version/-v. Wrapping these would open a TUI that never prints.
#  - resume flags (--conversation, -c/--continue) -> raw.
#  - outside tmux + terminal -> `hangar-open antigravity …`: the Hangar launcher opens the session and
#    the terminal attaches to it. An argument the request doesn't cover, no hangar-server or no hangar-open -> a tmux session named after the folder.
#  - already in tmux ($TMUX) / stdin not a tty (pipe/script) -> raw: the hook ticket links the pane.
#
# Escape hatch: `command agy ...` runs the raw binary, bypassing this wrapper.
agy() {
    local a
    # subcomando: só vale como PRIMEIRO argumento (`agy export x` é subcomando; `agy "export x"` é prompt).
    case "${1:-}" in
        acp|agent|agents|changelog|help|install|mcp|mic-serve|models|plugin|plugins|remote-control|update)
            command agy "$@"
            return
            ;;
    esac

    # usos não interativos primeiro: o agy imprime e sai, não tem TUI pra envolver em tmux.
    for a in "$@"; do
        case "$a" in
            -p|--print|--prompt|--prompt=*|--output-format|--output-format=*|--input-format|--input-format=*|--json-schema|--json-schema=*|--help|-h|--version|-v)
                command agy "$@"
                return
                ;;
        esac
    done

    # Daqui pra baixo abre TUI: refaz a ponte de skills agora, pra skill instalada no Claude valer
    # já nesta abertura (antes dependia de reiniciar o backend). Silencioso e fail-soft.
    command -v hangar-skills-sync >/dev/null 2>&1 && hangar-skills-sync

    # Retomada (--conversation, -c/--continue): CLI cru.
    for a in "$@"; do
        case "$a" in
            --conversation|--conversation=*|-c|--continue)
                command agy "$@"
                return
                ;;
        esac
    done

    # TMUX herdado pode estar MORTO (mesmo caso do wrapper claude/pi: kitty single-instance cujo
    # mestre nasceu dentro de um pane que já fechou). Valida o pane; stale -> limpa e segue pro
    # caminho "fora do tmux" (abre pelo Hangar).
    if [ -n "${TMUX:-}" ]; then
        if [ -z "${TMUX_PANE:-}" ] || ! tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1; then
            unset TMUX TMUX_PANE
        fi
    fi

    # Fora do tmux e em terminal: a sessão nasce pelo lançador do Hangar (ver claude.posix.sh).
    if [ -z "${TMUX:-}" ] && [ -t 0 ]; then
        if command -v hangar-open >/dev/null 2>&1; then
            local rc=0
            hangar-open antigravity "$@" || rc=$?
            [ "$rc" = 100 ] || return "$rc"
        else
            echo "hangar: hangar-open não encontrado; abrindo o agy sem o Hangar" >&2
        fi

        # Sem o tmux o CLI ficaria invisível pro app: sessão com o nome da pasta, única.
        # Acento -> ASCII antes do filtro, mesma regra do backend (app/names.py).
        local base name i ascii run=()
        base=$(basename "$PWD")
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
        # O probe confere que o systemd do usuário aceita scope transiente, não só que o binário existe.
        if command -v systemd-run >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ] \
           && systemd-run --user --scope --collect -q -- true >/dev/null 2>&1; then
            run=(systemd-run --user --scope --collect -q --)
        fi
        # CP_SESSION_NAME: identidade da sessão pro hangar-send de dentro do pane.
        "${run[@]}" tmux new-session -s "$name" -c "$PWD" -e "CP_SESSION_NAME=$name" \
            sh -c 'exec agy "$@"' _ "$@"
        return
    fi

    # já dentro de tmux ou stdin sem tty: nada a injetar — o bilhete do
    # hook liga o pane à sessão sem nenhuma variável nossa.
    command agy "$@"
}
