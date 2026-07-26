# claude-pocket — `claude-engine` (bash + zsh). Sourced do teu rc por
# scripts/install-claude-wrapper.sh. Fish tem a própria: scripts/shell/claude-engine.fish
#
# Abre uma sessão Claude Code normal (tmux + --session-id + teu ~/.claude inteiro) rodando num MOTOR
# diferente da conta Anthropic. Vale só para esta invocação.
claude-engine() {
    if [ "$#" -eq 0 ]; then
        local lista
        # Sem `local` na mesma linha da atribuição: `local lista=$(cmd)` mascara o exit code do
        # cmd com o do próprio `local` (sempre 0). Separado assim, `$?`/`!` é de verdade o de
        # cp-engine — distingue "não instalado" (comando não encontrado) de "zero motor configurado"
        # (--list sempre sai 0), senão as duas caem na MESMA mensagem, que manda o usuário pro app
        # justamente no caso em que o app não vai adiantar nada.
        if ! lista=$(cp-engine --list 2>/dev/null); then
            echo "claude-engine: cp-engine não pôde ser executado — rode ./scripts/install-claude-wrapper.sh" >&2
            return 1
        fi
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
