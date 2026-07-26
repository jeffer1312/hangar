# claude-pocket — `claude-engine` (bash + zsh). Sourced do teu rc por
# scripts/install-claude-wrapper.sh. Fish tem a própria: scripts/shell/claude-engine.fish
#
# Abre uma sessão Claude Code normal (tmux + --session-id + teu ~/.claude inteiro) rodando num MOTOR
# diferente da conta Anthropic. Vale só para esta invocação.
claude-engine() {
    if [ "$#" -eq 0 ]; then
        local lista
        lista=$(cp-engine --list)
        if [ -z "$lista" ]; then
            echo "Nenhum motor configurado. Configure no app (Configurações -> Motores de modelo)."
            return 1
        fi
        printf '%s\n' "$lista"
        return 0
    fi

    # Valida antes de abrir: motor inexistente abriria um pane que morre na cara do usuário.
    cp-engine --env "$1" >/dev/null || return 1

    local motor=$1; shift
    # CP_ENGINE só para ESTE comando (não fica no shell): o wrapper `claude` o lê e prefixa com
    # `cp-engine --exec`. A key não passa por aqui.
    CP_ENGINE="$motor" claude "$@"
}
