# Wrapper do `codex` pro PowerShell - equivalente do scripts/shell/codex.posix.sh.
#
# `codex` e `codex "prompt"` interativos criam a sessao pelo backend (hangar-codex) e anexam o
# terminal ao psmux, entao ela aparece no app como as criadas por ele. Subcomandos, flags e uso
# nao interativo (pipe/redirecionamento) vao pro binario oficial. Escape explicito:
# `& (Get-Command codex -CommandType Application)`.
# Compativel com Windows PowerShell 5.1 (nada de operador ternario).

function codex {
    $exe = (Get-Command codex -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1).Source
    if (-not $exe) { Write-Error 'codex nao encontrado no PATH'; return }

    $sub = @('exec', 'review', 'resume', 'fork', 'archive', 'delete', 'unarchive', 'login', 'logout',
             'mcp', 'plugin', 'app-server', 'remote-control', 'cloud', 'doctor', 'debug', 'features',
             'completion', 'update', 'sandbox', 'apply', 'app')
    $direto = [Console]::IsInputRedirected -or $args.Count -gt 1
    if (-not $direto -and $args.Count -eq 1) {
        $a = [string]$args[0]
        if ($a.StartsWith('-') -or $sub -contains $a) { $direto = $true }
    }
    if ($direto) { & $exe @args; return }

    $lancador = (Get-Command hangar-codex -CommandType Application -ErrorAction SilentlyContinue |
                 Select-Object -First 1).Source
    if (-not $lancador) {
        Write-Warning 'hangar-codex nao encontrado (rode install.ps1); abrindo o codex sem o app'
        & $exe @args; return
    }
    & $lancador @args
}
