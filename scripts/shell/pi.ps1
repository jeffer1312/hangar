# Wrapper do `pi` pro PowerShell - equivalente do scripts/shell/pi.posix.sh, regra por regra.
#
#  - subcomando como PRIMEIRO argumento (install/remove/uninstall/update/list/config/auth) ou flag
#    que imprime e sai em qualquer posicao (-p/--print, --mode, --list-models, --export,
#    --help/-h, --version/-v) -> pi cru.
#  - flag que gerencia a propria sessao (--session-id, --resume/-r, --continue/-c, --session,
#    --fork, --no-session) -> pi cru.
#  - fora do psmux + terminal -> o lancador do Hangar abre a sessao e o terminal anexa nela.
#    Argumento que o pedido nao cobre ou sem hangar-server -> sessao no psmux com o nome da pasta.
#  - ja no psmux / entrada redirecionada -> pi cru com --session-id e CP_PI_SESSION (o mesmo uuid):
#    o servidor acha a sessao pelo ambiente do processo.
#
# Escape explicito: `& (Get-Command pi -CommandType Application)`.
# Compativel com Windows PowerShell 5.1 (nada de operador ternario nem ??).

# Perfil gravado antes do hangar-open.ps1 existir carrega so os wrappers: o ajudante vem junto.
if (-not (Get-Command Open-HangarSession -CommandType Function -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot 'hangar-open.ps1')
}

function pi {
    # -CommandType Application: o binario, nunca esta funcao (senao recursao infinita).
    $exe = (Get-Command pi -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
    if (-not $exe) { Write-Error 'pi nao encontrado no PATH'; return }
    $lista = @($args)

    # Subcomando so vale como PRIMEIRO argumento (`pi remove x` e subcomando; `pi "remove x"` e prompt).
    if ($lista.Count -gt 0 -and @('install', 'remove', 'uninstall', 'update', 'list', 'config', 'auth') -ccontains [string]$lista[0]) {
        Invoke-HangarNative $exe $lista; return
    }
    # O pi imprime e sai: nao ha TUI pra abrir como sessao.
    if (Test-HangarArg $lista '^(-p|--print|--mode|--mode=.*|--list-models|--list-models=.*|--export|--export=.*|--help|-h|--version|-v)$') {
        Invoke-HangarNative $exe $lista; return
    }

    Sync-HangarSkills

    # Um id novo por cima de `pi -c` abriria sessao FRESCA em vez de continuar.
    if (Test-HangarArg $lista '^(--session-id|--session-id=.*|--resume|--resume=.*|-r|--continue|-c|--session|--session=.*|--fork|--fork=.*|--no-session)$') {
        Invoke-HangarNative $exe $lista; return
    }

    Clear-HangarStaleTmux

    if (-not $env:TMUX -and (Test-HangarInteractive)) {
        Open-HangarSession 'pi' 'pi' $lista
        if ($LASTEXITCODE -ne 100) { return }
        $id = [guid]::NewGuid().ToString()
        Open-HangarFolderSession @("CP_PI_SESSION=$id") (@('pi', '--session-id', $id) + $lista)
        return
    }

    # Cru com o id: ja no psmux ou entrada redirecionada.
    $id = [guid]::NewGuid().ToString()
    $env:CP_PI_SESSION = $id
    Invoke-HangarNative $exe (@('--session-id', $id) + $lista)
}
