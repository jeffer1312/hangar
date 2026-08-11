# claude-conta <nome> [args...] — abre o claude na conta <nome>. Ver claude-conta.fish.
# Função com hífen direto (precedente: claude-engine no claude-engine.posix.sh). Alias não
# expande em shell não interativo (bash -c/zsh -c), e o wrapper pode ser chamado de script.
claude-conta() {
    if [ "$#" -eq 0 ]; then
        cp-conta --list
        return $?
    fi
    local dir
    dir=$(cp-conta --prep "$1") || return 1
    # Contrato do --prep: stdout = EXATAMENTE o caminho, uma linha; o resto vai pro stderr.
    # Validar antes de exportar: linha extra (log futuro) viraria config dir inválido.
    case "$dir" in
        *$'\n'*) printf 'claude-conta: saída inesperada de cp-conta --prep\n' >&2; return 1 ;;
    esac
    [ -n "$dir" ] || return 1
    [ -d "$dir" ] || { printf 'claude-conta: %s não é um diretório\n' "$dir" >&2; return 1; }
    shift
    CLAUDE_CONFIG_DIR="$dir" claude "$@"
}
