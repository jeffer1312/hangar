# claude-pocket — `claude-engine` (fish). Instalado por scripts/install-claude-wrapper.sh.
#
# Abre uma sessão Claude Code normal (tmux + --session-id + teu ~/.claude inteiro: skills, hooks,
# CLAUDE.md, plugins, statusline) rodando num MOTOR diferente da conta Anthropic. O motor vale só
# para esta invocação: nenhum arquivo de config é tocado e o terminal ao lado segue na tua conta.
#
#   claude-engine              -> lista os motores configurados
#   claude-engine kimi         -> abre uma sessão no motor "kimi"
#   claude-engine kimi --foo   -> args extras vão pro claude
#
# Configurar: app -> Configurações -> Motores de modelo (ou ~/.claude/engines.json).
function claude-engine
    if test (count $argv) -eq 0
        set -l lista (cp-engine --list)
        if test -z "$lista"
            echo "Nenhum motor configurado. Configure no app (Configurações -> Motores de modelo)."
            return 1
        end
        printf '%s\n' $lista
        return 0
    end

    # Valida ANTES de abrir a sessão: motor inexistente abriria um pane que morre na cara do usuário.
    cp-engine --env $argv[1] >/dev/null; or return 1

    # CP_ENGINE é lido pelo wrapper `claude`, que prefixa o comando com `cp-engine --exec`. A key não
    # passa por aqui em momento nenhum.
    set -lx CP_ENGINE $argv[1]
    set -e argv[1]
    claude $argv
end
