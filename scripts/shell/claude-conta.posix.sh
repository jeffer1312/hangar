# claude-conta <nome> [args...] — abre o claude na conta <nome>. Ver claude-conta.fish.
# Função direta, não alias: bash não expande alias em shell não-interativo (o harness de teste da
# casa roda `bash -c`), e o repo já tem o precedente de hífen no nome — claude-engine().
claude-conta() {
    if [ "$#" -eq 0 ]; then
        cp-conta --list
        return
    fi
    local dir
    dir=$(cp-conta --prep "$1") || return 1
    # Contrato do `--prep`: UMA linha de stdout. Duas linhas (progresso acidental no futuro)
    # entrariam no caminho com quebra de linha — recusa em vez de montar um config dir inválido.
    [ "$(printf '%s\n' "$dir" | wc -l)" -eq 1 ] || return 1
    [ -n "$dir" ] || return 1
    [ -d "$dir" ] || return 1
    shift
    CLAUDE_CONFIG_DIR="$dir" claude "$@"
}
