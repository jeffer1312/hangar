# claude-conta <nome> [args...] — abre o claude na conta <nome>. Ver claude-conta.fish.
function claude-conta {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Args)
    if (-not $Args -or $Args.Count -eq 0) { cp-conta --list; return }
    $dir = cp-conta --prep $Args[0]
    if ($LASTEXITCODE -ne 0 -or -not $dir) { return }
    $env:CLAUDE_CONFIG_DIR = $dir
    claude @($Args | Select-Object -Skip 1)
}
