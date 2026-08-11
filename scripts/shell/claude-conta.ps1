# claude-conta <nome> [args...] — abre o claude na conta <nome>. Ver claude-conta.fish.
function claude-conta {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Args)
    if (-not $Args -or $Args.Count -eq 0) { cp-conta --list; return }
    $dir = @(cp-conta --prep $Args[0])
    # Contrato do `--prep`: UMA linha de stdout. Multi-linha vira array — recusa.
    if ($LASTEXITCODE -ne 0 -or $dir.Count -ne 1 -or -not $dir[0] -or
        -not (Test-Path -Path $dir[0] -PathType Container)) { return }
    $antigo = $env:CLAUDE_CONFIG_DIR
    $env:CLAUDE_CONFIG_DIR = $dir[0]
    try {
        claude @($Args | Select-Object -Skip 1)
    } finally {
        # O env e do processo PS inteiro: sem restaurar, o shell do usuario ficaria preso na conta.
        if ($null -eq $antigo) { Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue }
        else { $env:CLAUDE_CONFIG_DIR = $antigo }
    }
}
