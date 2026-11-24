# Wrapper do `kimi` pro PowerShell - equivalente do scripts/shell/kimi.posix.sh, regra por regra.
#
# O Kimi cria o ID no primeiro prompt; o hook `hangar-server kimi-state-hook` herda TMUX_PANE e
# grava o bilhete que liga o pane a sessao.
#
#  - subcomando como PRIMEIRO argumento (export/provider/acp/web/server/login/doctor/vis/migrate/
#    upgrade/update) -> kimi cru.
#  - flag que imprime e sai em qualquer posicao (-p/--prompt, --output-format, --help/-h,
#    --version/-V) -> kimi cru.
#  - retomada (-S/--session, -c/--continue) -> kimi cru.
#  - fora do psmux + terminal -> o lancador do Hangar abre a sessao e o terminal anexa nela.
#    Argumento que o pedido nao cobre ou sem hangar-server -> sessao no psmux com o nome da pasta.
#  - ja no psmux / entrada redirecionada -> kimi cru: o bilhete do hook liga o pane a sessao.
#
# Escape explicito: `& (Get-Command kimi -CommandType Application)`.
# Compativel com Windows PowerShell 5.1 (nada de operador ternario nem ??).

# Perfil gravado antes do hangar-open.ps1 existir carrega so os wrappers: o ajudante vem junto.
if (-not (Get-Command Open-HangarSession -CommandType Function -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot 'hangar-open.ps1')
}

function kimi {
    # -CommandType Application: o binario, nunca esta funcao (senao recursao infinita).
    $exe = (Get-Command kimi -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
    if (-not $exe) { Write-Error 'kimi nao encontrado no PATH'; return }
    $lista = @($args)
    # Entrada pelo pipeline (`Get-Content f | kimi ...`) vai ao CLI cru, como o `[ -t 0 ]` do POSIX.
    $entrada = $null
    if ($MyInvocation.ExpectingInput) { $entrada = @($input) }

    # Subcomando so vale como PRIMEIRO argumento (`kimi export x` e subcomando; `kimi "export x"` e prompt).
    $sub = @('export', 'provider', 'acp', 'web', 'server', 'login', 'doctor', 'vis', 'migrate', 'upgrade', 'update')
    if ($lista.Count -gt 0 -and $sub -ccontains [string]$lista[0]) { Invoke-HangarNative $exe $lista $entrada; return }
    # O kimi imprime e sai: nao ha TUI pra envolver no psmux.
    if (Test-HangarArg $lista '^(-p|--prompt|--prompt=.*|--output-format|--output-format=.*|--help|-h|--version|-V)$') {
        Invoke-HangarNative $exe $lista $entrada; return
    }

    Sync-HangarSkills

    if (Test-HangarArg $lista '^(-S|--session|--session=.*|-c|--continue)$') {
        Invoke-HangarNative $exe $lista $entrada; return
    }

    Clear-HangarStaleTmux

    if (-not $env:TMUX -and $null -eq $entrada -and (Test-HangarInteractive)) {
        Open-HangarSession 'kimi' 'kimi' $lista
        if ($LASTEXITCODE -ne 100) { return }
        # KIMI_CODE_TUI_FULL_SCREEN=1: mesma TUI das sessoes que o app abre.
        Open-HangarFolderSession @('KIMI_CODE_TUI_FULL_SCREEN=1') (@('kimi') + $lista)
        return
    }

    Invoke-HangarNative $exe $lista $entrada
}
