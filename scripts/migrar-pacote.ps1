# Passo 2026-10-24-pacote-por-versao no Windows: o mesmo que o scripts/migrar-pacote.sh. Baixa o
# hangar-server da release para ~\.hangar\migrate e roda a migracao por ele (o binario que o motor
# `git` tem em ~\.hangar\bin ainda e o da versao anterior, sem o `migrate-package`).
#
# Ja migrada: sai 0. Sem binario do Rust para este sistema: grava a marca `not_migrated` e sai 0.
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$marca = Join-Path $HOME '.hangar\migration.json'
if (Test-Path -LiteralPath $marca) { Write-Output "ja migrada: $marca"; exit 0 }
$bin = Join-Path $HOME '.hangar\migrate\hangar-server.exe'

# 2 = a release nao tem build para esta maquina; os avisos do download vao para a saida do passo.
Push-Location backend
try {
    & .\.venv\Scripts\python.exe -c "from pathlib import Path; from app import rust_release as r; a = r.fetch(dest=Path.home() / '.hangar' / 'migrate'); print(*(a or []), sep=chr(10)); raise SystemExit(2 if a is None else 0)"
    $rc = $LASTEXITCODE
} finally { Pop-Location }

if ($rc -eq 2) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $marca) | Out-Null
    $json = [ordered]@{ mode = 'not_migrated'; date = (Get-Date).ToString('o'); checkout = (Get-Location).Path; version = $null
        reason = 'o Hangar nao publica o binario do Rust para este sistema; a maquina segue no motor git'; notes = @(); done = @() } | ConvertTo-Json -Compress
    # Sem BOM: o Set-Content -Encoding utf8 do PowerShell 5.1 poe um, e o leitor JSON do Rust recusa.
    [IO.File]::WriteAllText("$marca.tmp", $json)
    Move-Item -Force -LiteralPath "$marca.tmp" -Destination $marca
    Write-Output 'nao migrada: sem binario do Rust para este sistema'
    exit 0
}
if ($rc -ne 0 -or -not (Test-Path -LiteralPath $bin)) {
    Write-Output "##HANGAR-FALHA## o binario da migracao nao foi baixado para $bin (download saiu com $rc)"
    exit 1
}
& $bin migrate-package
exit $LASTEXITCODE
