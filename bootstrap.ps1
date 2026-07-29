# claude-cockpit - clonar e instalar numa linha so, no Windows.
#
#   irm https://raw.githubusercontent.com/jeffer1312/claude-cockpit/main/bootstrap.ps1 | iex
#
# Destino: $HOME\claude-cockpit. Pra mudar, defina a variavel ANTES da linha acima:
#
#   $env:CP_DESTINO = 'D:\claude-cockpit'
#   irm https://.../bootstrap.ps1 | iex
#
# Nao ha `param()` aqui de proposito: sob `irm | iex` o iex recebe so o TEXTO do script, entao
# nao existe argumento pra receber - e o script tambem nao sabe o proprio caminho
# ($MyInvocation.MyCommand.Path vem vazio), entao nada aqui depende dele.
#
# Escrito pra Windows PowerShell 5.1 (o que vem no Windows), igual ao install.ps1: nada de
# operador ternario, `??` nem API de .NET Core.
#
# Instale num disco LOCAL. Numa pasta compartilhada por rede (`\\servidor\...`) o `uv sync` e o
# `npm ci` recriariam `backend\.venv` e `frontend\node_modules` por cima dos da maquina de
# origem - e esses sao dela, nao seus: o venv aponta pro Python daquela maquina e o node_modules
# traz o binario nativo dela. Instalar de uma segunda maquina quebra a instalacao da primeira.

$ErrorActionPreference = 'Stop'

$repoUrl = 'https://github.com/jeffer1312/claude-cockpit.git'
$ramo    = 'main'

function Titulo($m) { Write-Host "`n$m" -ForegroundColor Cyan }
function Ok($m)     { Write-Host "  ok  $m" -ForegroundColor Green }
function Nota($m)   { Write-Host "      $m" -ForegroundColor DarkGray }
function Erro($m)   { Write-Host "  X   $m" -ForegroundColor Red }
function Tem($cmd)  { return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

function Atualiza-Path {
    # winget grava o PATH no registro, mas o PowerShell JA ABERTO segue com o antigo -> o git
    # recem-instalado "nao existe". Mesmo truque do install.ps1.
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
                [Environment]::GetEnvironmentVariable('Path', 'User')
}

$destino = $env:CP_DESTINO
if (-not $destino) { $destino = Join-Path $HOME 'claude-cockpit' }

# $true = o remoto desta pasta e o claude-cockpit. Checar o REMOTO, nao so a existencia da
# pasta: "tem um .git aqui" nao quer dizer que e este projeto, e dar pull no repo errado e pior
# que parar.
function EhEsteRepo($pasta) {
    if (-not (Test-Path (Join-Path $pasta '.git'))) { return $false }
    $url = & git -C $pasta remote get-url origin 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $url) { return $false }
    $url = ([string]$url).Trim().TrimEnd('/')
    if ($url.EndsWith('.git')) { $url = $url.Substring(0, $url.Length - 4) }
    return ($url -match 'github\.com[:/]jeffer1312/claude-cockpit$')
}

function Clona {
    Titulo "Clonando em $destino"
    & git clone --branch $ramo $repoUrl $destino
    if ($LASTEXITCODE -ne 0) { Erro 'git clone falhou'; exit 1 }
    Ok 'clonado'
}

Titulo 'claude-cockpit - instalacao em uma linha'

Atualiza-Path
if (Tem 'git') {
    Ok 'git'
} else {
    # Aqui o git nao e opcional como no install.ps1 (la ele so alimenta o painel de branch):
    # sem git nao ha como clonar, e nao ha o que instalar depois.
    if (-not (Tem 'winget')) {
        Erro 'sem git e sem winget - instale o "App Installer" pela Microsoft Store, ou o Git'
        Nota 'https://git-scm.com/download/win - e rode esta linha de novo'
        exit 1
    }
    Write-Host '  .. instalando Git (Git.Git)'
    winget install --id Git.Git --exact --silent `
        --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null
    Atualiza-Path
    if (-not (Tem 'git')) { Erro 'o Git nao instalou - instale na mao e rode de novo'; exit 1 }
    Ok 'Git instalado'
}

if (-not (Test-Path $destino)) {
    Clona
} elseif (EhEsteRepo $destino) {
    Ok "$destino ja e este repositorio - atualizando em vez de clonar"
    & git -C $destino pull --ff-only origin $ramo
    if ($LASTEXITCODE -ne 0) {
        Erro "git pull falhou em $destino (mudanca local pendente?) - resolva na mao e rode de novo"
        exit 1
    }
    Ok 'atualizado'
} elseif (Get-ChildItem -Force -LiteralPath $destino -ErrorAction SilentlyContinue) {
    Erro "$destino ja existe e NAO e o claude-cockpit - nao vou mexer no que e seu"
    Nota 'outro destino: $env:CP_DESTINO = "D:\claude-cockpit" e rode de novo'
    exit 1
} else {
    Clona   # pasta existe mas esta vazia: o git clona pra dentro dela
}

Titulo 'Instalando'
$instalador = Join-Path $destino 'install.ps1'
if (-not (Test-Path $instalador)) { Erro "install.ps1 nao encontrado em $destino"; exit 1 }
Set-Location $destino
# Num processo proprio com -ExecutionPolicy Bypass: sob `irm | iex` a politica desta sessao
# pode ser Restricted, e ai um script EM ARQUIVO nao roda. O console e o mesmo, entao os
# Read-Host do install.ps1 continuam perguntando a voce normalmente.
& powershell -NoProfile -ExecutionPolicy Bypass -File $instalador
exit $LASTEXITCODE
