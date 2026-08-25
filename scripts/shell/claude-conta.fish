# claude-conta <nome> [args...] — abre o claude na conta <nome>. Sem argumento, lista as contas.
# O `--prep` refaz os atalhos antes de abrir: é o que impede a conta de ficar pra trás quando
# aparece pasta nova no ~/.claude.
function claude-conta
    if test (count $argv) -eq 0
        hangar-conta --list
        return $status
    end
    set -l dir (hangar-conta --prep $argv[1])
    # Checar o CONTEÚDO, não `or`: em fish o `or` depois de `set` lê o status do SET, que é 0
    # mesmo quando a substituição de comando falhou. Com `or`, uma falha do hangar-conta deixava $dir
    # vazio, não retornava, e `CLAUDE_CONFIG_DIR=""` abria na conta PADRÃO sem avisar.
    # Contrato do --prep: stdout = exatamente UM caminho. Linha extra vira LISTA em fish — o
    # test -d pegaria o primeiro elemento e o resto viraria ruído calado; 0 linhas = falhou.
    test (count $dir) -eq 1; or return 1
    test -n "$dir"; or return 1
    test -d "$dir"; or return 1
    set -lx CLAUDE_CONFIG_DIR $dir
    claude $argv[2..]
end
