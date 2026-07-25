# claude-cockpit — wrapper interativo do Codex para bash/zsh.
# `codex` e `codex "prompt"` criam a sessao pelo backend e anexam seu tmux.
# Subcomandos/flags e usos nao interativos continuam no binario oficial.
# Escape explicito: `command codex ...`.
codex() {
    if [ ! -t 0 ] || [ "$#" -gt 1 ]; then
        command codex "$@"
        return
    fi
    if [ "$#" -eq 1 ]; then
        case "$1" in
            -*) command codex "$@"; return ;;
            exec|review|resume|fork|archive|delete|unarchive|login|logout|mcp|plugin|app-server|remote-control|cloud|doctor|debug|features|completion|update|sandbox|apply)
                command codex "$@"; return ;;
        esac
    fi
    command cp-codex "$@"
}
