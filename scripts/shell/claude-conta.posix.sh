# claude-conta <nome> [args...] — abre o claude na conta <nome>. Ver claude-conta.fish.
# Função direta, não alias: bash não expande alias em shell não-interativo (o harness de teste da
# casa roda `bash -c`), e o repo já tem o precedente de hífen no nome — claude-engine().
claude-conta() {
    if [ "$#" -eq 0 ]; then
        cp-conta --list
        return 0
    fi
    dir=$(cp-conta --prep "$1") || return 1
    [ -n "$dir" ] || return 1
    shift
    CLAUDE_CONFIG_DIR="$dir" claude "$@"
}
