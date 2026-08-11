# claude-conta <nome> [args...] — abre o claude na conta <nome>. Ver claude-conta.fish.
function claude-conta {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Args)
    if (-not $Args -or $Args.Count -eq 0) { cp-conta --list; return }
    $dir = cp-conta --prep $Args[0]
    if ($LASTEXITCODE -ne 0) { return }
    # Contrato do --prep: stdout = exatamente UM caminho. Multi-linha vira ARRAY em PowerShell —
    # validar antes de exportar, senão o array vira string com espaços e o claude abre em lugar
    # errado (ou falha) sem ninguém entender por quê.
    $dirs = @($dir)
    if ($dirs.Count -ne 1 -or -not (Test-Path -LiteralPath $dirs[0] -PathType Container)) {
        Write-Error 'claude-conta: saída inesperada de cp-conta --prep'
        return
    }
    # Restaurar o env do chamador no fim: bash usa atribuição temporária e fish set -lx, os dois
    # restauram sozinhos; o processo PowerShell NÃO — sem isto o shell inteiro fica na conta e o
    # próximo `claude` comum (ou qualquer comando que leia a variável) usa a conta errada.
    $tinha = Test-Path Env:CLAUDE_CONFIG_DIR
    $antiga = $env:CLAUDE_CONFIG_DIR
    try {
        $env:CLAUDE_CONFIG_DIR = $dirs[0]
        claude @($Args | Select-Object -Skip 1)
    } finally {
        if ($tinha) { $env:CLAUDE_CONFIG_DIR = $antiga } else { Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction SilentlyContinue }
    }
}
