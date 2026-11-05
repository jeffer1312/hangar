# O CLI usa o binário da instalação, inclusive com o serviço parado.
$ErrorActionPreference = 'Stop'
$serverRoot = Split-Path $PSScriptRoot -Parent
$serverExecutable = $null
if ($env:CP_RUST_SERVER_BIN) {
    $serverExecutable = $env:CP_RUST_SERVER_BIN
    if (-not (Test-Path -LiteralPath $serverExecutable -PathType Leaf)) {
        Write-Error 'CP_RUST_SERVER_BIN não é um executável'; exit 126
    }
} else {
    $serverCurrent = Join-Path $HOME '.hangar\current'
    for ($serverWait = 0; $serverWait -lt 20 -and -not (Test-Path $serverCurrent) -and (Test-Path "$serverCurrent.old"); $serverWait++) { Start-Sleep -Milliseconds 50 }
    $serverPackage = Join-Path $serverCurrent 'bin\hangar-server.exe'
    if (Test-Path -LiteralPath $serverPackage -PathType Leaf) { $serverExecutable = (Resolve-Path $serverPackage).Path }
    else {
        $serverCandidates = @((Join-Path $serverRoot 'crates\target\release\hangar-server.exe'), (Join-Path $HOME '.hangar\bin\hangar-server.exe')) |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Get-Item | Sort-Object LastWriteTime -Descending
        if ($serverCandidates) { $serverExecutable = $serverCandidates[0].FullName }
    }
}
if (-not $serverExecutable) { Write-Error 'hangar-server não encontrado; instale o Hangar ou configure CP_RUST_SERVER_BIN'; exit 126 }
& $serverExecutable @args
exit $LASTEXITCODE
