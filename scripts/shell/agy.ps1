# Wrapper do `agy` pro PowerShell - equivalente do scripts/shell/agy.posix.sh, regra por regra.
#
# O agy cria o ID no primeiro prompt; o hook `hangar-server agy-state-hook` herda TMUX_PANE e
# grava o bilhete que liga o pane a sessao. No Hangar o provider se chama `antigravity`.
#
#  - subcomando como PRIMEIRO argumento (acp/agent/agents/changelog/help/install/mcp/mic-serve/
#    models/plugin/plugins/remote-control/update) -> agy cru.
#  - flag que imprime e sai em qualquer posicao (-p/--print/--prompt, --output-format, --input-format,
#    --json-schema, --help/-h, --version/-v) -> agy cru.
#  - retomada (--conversation, -c/--continue) -> agy cru.
#  - fora do psmux + terminal -> o lancador do Hangar abre a sessao e o terminal anexa nela.
#    Argumento que o pedido nao cobre ou sem hangar-server -> sessao no psmux com o nome da pasta.
#  - ja no psmux / entrada redirecionada -> agy cru: o bilhete do hook liga o pane a sessao.
#
# Escape explicito: `& (Get-Command agy -CommandType Application)`.
# Compativel com Windows PowerShell 5.1 (nada de operador ternario nem ??).

# Perfil gravado antes do hangar-open.ps1 existir carrega so os wrappers: o ajudante vem junto.
if (-not (Get-Command Open-HangarSession -CommandType Function -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot 'hangar-open.ps1')
}

function agy {
    # -CommandType Application: o binario, nunca esta funcao (senao recursao infinita).
    $exe = (Get-Command agy -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
    if (-not $exe) { Write-Error 'agy nao encontrado no PATH'; return }
    $lista = @($args)

    # Subcomando so vale como PRIMEIRO argumento (`agy update x` e subcomando; `agy "update x"` e prompt).
    $sub = @('acp', 'agent', 'agents', 'changelog', 'help', 'install', 'mcp', 'mic-serve', 'models', 'plugin',
             'plugins', 'remote-control', 'update')
    if ($lista.Count -gt 0 -and $sub -ccontains [string]$lista[0]) { Invoke-HangarNative $exe $lista; return }
    # O agy imprime e sai: nao ha TUI pra envolver no psmux.
    $imprime = '^(-p|--print|--prompt|--prompt=.*|--output-format|--output-format=.*|--input-format|' +
               '--input-format=.*|--json-schema|--json-schema=.*|--help|-h|--version|-v)$'
    if (Test-HangarArg $lista $imprime) {
        Invoke-HangarNative $exe $lista; return
    }

    Sync-HangarSkills

    if (Test-HangarArg $lista '^(--conversation|--conversation=.*|-c|--continue)$') {
        Invoke-HangarNative $exe $lista; return
    }

    Clear-HangarStaleTmux

    if (-not $env:TMUX -and (Test-HangarInteractive)) {
        Open-HangarSession 'antigravity' 'agy' $lista
        if ($LASTEXITCODE -ne 100) { return }
        Open-HangarFolderSession @() (@('agy') + $lista)
        return
    }

    Invoke-HangarNative $exe $lista
}
