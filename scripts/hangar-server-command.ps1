# O CLI usa o binário da instalação, inclusive com o serviço parado. A regra é a do
# scripts/hangar-server-lookup (e do hangar-server-lookup.cmd), com as mesmas frases.
$ErrorActionPreference = 'Stop'
$serverRoot = Split-Path $PSScriptRoot -Parent
$serverExecutable = $null
$serverChoice = "$env:CP_RUST_SERVER_BIN".Trim()
if ($serverChoice) {
    # A variável vence e nunca cai para outro: escolha explícita errada não vira outro binário calado.
    if ($serverChoice -eq '~' -or $serverChoice.StartsWith('~\') -or $serverChoice.StartsWith('~/')) { $serverChoice = $HOME + $serverChoice.Substring(1) }
    if (-not (Test-Path -LiteralPath $serverChoice -PathType Leaf)) {
        [Console]::Error.WriteLine('hangar: CP_RUST_SERVER_BIN não é um executável'); exit 126
    }
    $serverExecutable = $serverChoice
} else {
    $serverCurrent = Join-Path $HOME '.hangar\current'
    # A troca de versão deixa só o current.old por um instante: espera o novo até 1 s.
    for ($serverWait = 0; $serverWait -lt 20 -and -not (Test-Path -LiteralPath $serverCurrent) -and (Test-Path -LiteralPath "$serverCurrent.old"); $serverWait++) { Start-Sleep -Milliseconds 50 }
    if (Test-Path -LiteralPath (Join-Path $serverCurrent 'bin\hangar-server.exe') -PathType Leaf) {
        # Pelo caminho real da versão (o `cd -P` do sh): o processo de pé não troca de arquivo
        # quando o current muda. Alvo relativo não é seguido; fica o próprio current.
        $serverItem = Get-Item -LiteralPath $serverCurrent
        $serverTarget = if ($serverItem.LinkType) { @($serverItem.Target)[0] } else { $null }
        $serverReal = if ($serverTarget -and [IO.Path]::IsPathRooted($serverTarget)) { $serverTarget } else { $serverItem.FullName }
        $serverExecutable = Join-Path $serverReal 'bin\hangar-server.exe'
    } else {
        # O mais novo vence; empate fica com a cópia (só o estritamente mais novo troca).
        $serverBest = $null
        foreach ($serverCandidate in @((Join-Path $serverRoot 'crates\target\release\hangar-server.exe'), (Join-Path $HOME '.hangar\bin\hangar-server.exe'))) {
            if (Test-Path -LiteralPath $serverCandidate -PathType Leaf) {
                $serverItem = Get-Item -LiteralPath $serverCandidate
                if (-not $serverBest -or $serverItem.LastWriteTimeUtc -gt $serverBest.LastWriteTimeUtc) { $serverBest = $serverItem }
            }
        }
        if ($serverBest) { $serverExecutable = $serverBest.FullName }
    }
}
if (-not $serverExecutable) {
    [Console]::Error.WriteLine('hangar: hangar-server não encontrado; instale o Hangar ou configure CP_RUST_SERVER_BIN'); exit 126
}
& $serverExecutable @args
exit $LASTEXITCODE
