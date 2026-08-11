# claude-conta <nome> [args...] — abre o claude na conta <nome>. Ver claude-conta.fish.
claude_conta() {
    if [ "$#" -eq 0 ]; then
        cp-conta --list
        return 0
    fi
    dir=$(cp-conta --prep "$1") || return 1
    [ -n "$dir" ] || return 1
    shift
    CLAUDE_CONFIG_DIR="$dir" claude "$@"
}
alias claude-conta=claude_conta
