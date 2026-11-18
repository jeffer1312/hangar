# hangar — wrapper interativo do Codex para bash/zsh.
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
            exec|review|resume|fork|archive|delete|unarchive|login|logout|mcp|plugin|app-server|app|remote-control|cloud|doctor|debug|features|completion|update|sandbox|apply)
                command codex "$@"; return ;;
        esac
    fi
    # Sem o lançador, ou sem sessão aberta (sai 3: nem servidor nem `hangar-server`), o oficial abre.
    if command -v hangar-codex >/dev/null 2>&1; then
        command hangar-codex "$@"
        _hangar_codex_rc=$?
        [ "$_hangar_codex_rc" -ne 3 ] && return "$_hangar_codex_rc"
    fi
    command codex "$@"
}
