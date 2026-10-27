# Passo 2026-10-24-pacote-por-versao no Windows: o mesmo que o scripts/migrar-pacote.sh. Baixa o
# hangar-server da release para ~\.hangar\migrate e roda a migracao por ele (o binario que o motor
# `git` tem em ~\.hangar\bin ainda e o da versao anterior, sem o `migrate-package`).
#
# Ja migrada: sai 0. Sem binario do Rust para este sistema: grava a marca `not_migrated` e sai 0.
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$marca = Join-Path $HOME '.hangar\migration.json'
# A prova do passo: ele rodou, migrando agora ou deixando a migracao para a tela.
try {
    New-Item -ItemType Directory -Force -Path (Join-Path $HOME '.hangar') | Out-Null
    [IO.File]::WriteAllText((Join-Path $HOME '.hangar\migration-offered'), (Get-Date).ToString('o'))
} catch {
    Write-Output "##HANGAR-FALHA## nao consegui gravar $HOME\.hangar\migration-offered: $($_.Exception.Message)"
    exit 1
}
if (Test-Path -LiteralPath $marca) { Write-Output "ja migrada: $marca"; exit 0 }
# Sem o aviso aceito na tela (o motor ou o app antigo atualizando), nada muda: a versao nova mostra o
# aviso com a escolha, e a migracao roda pela tela.
$aceitos = Join-Path $HOME '.hangar\update-accepted.json'
# Mesma leitura do servidor (`update::accepted`): so uma lista de textos vale; outro formato e nada aceito.
$aceito = $false
try {
    $lista = Get-Content -Raw -LiteralPath $aceitos -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
    $aceito = ($lista -is [array]) -and -not ($lista | Where-Object { $_ -isnot [string] }) -and ($lista -contains '2026-10-24-pacote-por-versao')
} catch { $aceito = $false }
if (-not $aceito) {
    Write-Output 'migracao pendente: espera o aviso ser aceito na tela de atualizacao'
    exit 0
}
$pasta = Join-Path $HOME '.hangar\migrate'
$bin = Join-Path $pasta 'hangar-server.exe'

# O binario de uma rodada anterior nao pode rodar no lugar de um download que falhou.
try {
    if (Test-Path -LiteralPath $pasta) { Remove-Item -LiteralPath $pasta -Recurse -Force }
} catch {
    Write-Output "##HANGAR-FALHA## nao consegui tirar o binario anterior de ${pasta}: $($_.Exception.Message)"
    exit 1
}

# 2 = a release nao tem build para esta maquina; os avisos do download vao para a saida do passo.
Push-Location backend
try {
    $avisos = (& .\.venv\Scripts\python.exe -c "from pathlib import Path; from app import rust_release as r; a = r.fetch(dest=Path.home() / '.hangar' / 'migrate'); print(*(a or []), sep=chr(10)); raise SystemExit(2 if a is None else 0)" | Out-String).Trim()
    $rc = $LASTEXITCODE
} finally { Pop-Location }
if ($avisos) { Write-Output $avisos }

if ($rc -eq 2) {
    try {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $marca) | Out-Null
        $json = [ordered]@{ mode = 'not_migrated'; date = (Get-Date).ToString('o'); checkout = (Get-Location).Path; version = $null
            reason = 'o Hangar nao publica o binario do Rust para este sistema; a maquina segue no motor git'; notes = @(); done = @() } | ConvertTo-Json -Compress
        # Sem BOM: o Set-Content -Encoding utf8 do PowerShell 5.1 poe um, e o leitor JSON do Rust recusa.
        [IO.File]::WriteAllText("$marca.tmp", $json)
        Move-Item -Force -LiteralPath "$marca.tmp" -Destination $marca
    } catch {
        Remove-Item -LiteralPath "$marca.tmp" -Force -ErrorAction SilentlyContinue
        Write-Output "##HANGAR-FALHA## sem binario do Rust para este sistema, e nao consegui gravar a marca ${marca}: $($_.Exception.Message)"
        exit 1
    }
    Write-Output 'nao migrada: sem binario do Rust para este sistema'
    exit 0
}
if ($rc -ne 0 -or -not (Test-Path -LiteralPath $bin)) {
    Write-Output "##HANGAR-FALHA## o binario da migracao nao foi baixado para $bin (download saiu com $rc): $($avisos -replace '\r?\n', ' ')"
    exit 1
}
& $bin migrate-package
exit $LASTEXITCODE
