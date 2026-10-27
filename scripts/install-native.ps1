# Instala o app nativo do Hangar (desktop-native) da release `native-latest` no Windows, conferido
# pelo sha256 do manifesto antes de tocar em qualquer coisa. Maquina sem build na release sai 0 com
# aviso: a maquina fica com o Electron que ja tinha, ou com o navegador. Arquivo so em ASCII: o PowerShell 5.1 le sem BOM como cp1252.
# Uso: powershell -ExecutionPolicy Bypass -File scripts/install-native.ps1 [-Forcar]
param([switch]$Forcar)
$ErrorActionPreference = 'Stop'
$raiz = Split-Path -Parent $PSScriptRoot

$release = if ($env:HANGAR_NATIVE_UPDATE_URL) { $env:HANGAR_NATIVE_UPDATE_URL } else { 'https://github.com/jeffer1312/hangar/releases/download/native-latest' }
$marcaDir = Join-Path $HOME '.hangar\native'
$marca = Join-Path $marcaDir 'release.json'
$destDir = Join-Path $env:LOCALAPPDATA 'Programs\Hangar'
$app = Join-Path $destDir 'Hangar.exe'

function Marca($texto) {
    New-Item -ItemType Directory -Force -Path $marcaDir | Out-Null
    [IO.File]::WriteAllText($marca, $texto)
}
function Versao-De($texto) { if ("$texto" -match '"version":\s*"([0-9.]+)"') { $Matches[1] } else { '' } }

$arq = "$env:PROCESSOR_ARCHITEW6432$env:PROCESSOR_ARCHITECTURE"
if ($arq -notmatch 'AMD64') {
    Write-Host "app nativo: a release nao tem build para Windows $arq; o Hangar segue no navegador"
    Marca '{"sem_build": true}'
    # Sem build nao ha o que registrar: a marca do passo do link hangar:// so diz "passo feito nesta maquina".
    [IO.File]::WriteAllText((Join-Path $marcaDir 'scheme-hangar'), '')
    exit 0
}
$pacote = 'Hangar-windows-x86_64.zip'

# TLS 1.2 explicito (o 5.1 ainda tenta 1.0 e o GitHub recusa) e sem barra de progresso (no 5.1 ela custa mais que o download).
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ProgressPreference = 'SilentlyContinue'
$tmp = Join-Path $env:TEMP ('hangar-native.' + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    try {
        $bruto = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 30 -Uri "$release/native-latest.json").Content
    } catch { Write-Host "app nativo: nao consegui ler a release em $release"; exit 1 }
    if ($bruto -is [byte[]]) { $bruto = [Text.Encoding]::UTF8.GetString($bruto) }
    $versao = Versao-De $bruto
    $instalada = if (Test-Path $marca) { Versao-De ([IO.File]::ReadAllText($marca)) } else { '' }
    if (-not $Forcar -and (Test-Path $app) -and $versao -and $instalada -eq $versao) {
        Write-Host "app nativo ja na versao $versao"
        exit 0
    }
    if ($bruto -notmatch ('"' + [regex]::Escape($pacote) + '":\s*"([0-9a-f]{64})"')) {
        Write-Host "app nativo: $pacote nao esta no manifesto da release"; exit 1
    }
    $sha = $Matches[1]
    $zip = Join-Path $tmp $pacote
    try { Invoke-WebRequest -UseBasicParsing -TimeoutSec 300 -Uri "$release/$pacote" -OutFile $zip | Out-Null }
    catch { Write-Host "app nativo: download de $pacote falhou"; exit 1 }
    $lido = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLower()
    if ($lido -ne $sha) { Write-Host "app nativo: sha256 de $pacote nao confere com a release; nada foi instalado"; exit 1 }

    Expand-Archive -Path $zip -DestinationPath (Join-Path $tmp 'app') -Force
    New-Item -ItemType Directory -Force -Path $destDir | Out-Null
    # O exe aberto nao pode ser sobrescrito, mas pode ser renomeado: o velho sai da frente e o app aberto segue vivo.
    $velho = "$app.old"
    if (Test-Path $app) {
        # Um .old que ainda e a imagem de um app aberto nao pode ser apagado: os restos saem quando da,
        # e o que ficou preso cede o nome (mesma regra do update.rs do app).
        Get-ChildItem -Path $destDir -Filter 'Hangar.exe.old*' -File -Force -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
        if (Test-Path $velho) { $velho = "$app.old-" + [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
        Rename-Item $app $velho
    }
    # Copia falhou (disco, antivirus): o exe anterior volta pro lugar, senao os atalhos apontariam pro nada.
    try { Copy-Item (Join-Path $tmp 'app\Hangar.exe') $app -Force }
    catch { if (Test-Path $velho) { Rename-Item $velho $app -Force }; throw }
    if (-not (Test-Path $app)) { Write-Host "app nativo: $app nao ficou no lugar; nada foi marcado"; exit 1 }

    $ws = New-Object -ComObject WScript.Shell
    foreach ($pasta in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('DesktopDirectory'))) {
        $atalho = $ws.CreateShortcut((Join-Path $pasta 'Hangar.lnk'))
        $atalho.TargetPath = $app
        $atalho.Arguments = ''
        $atalho.WorkingDirectory = $destDir
        $atalho.IconLocation = "$app,0"
        $atalho.Description = 'Hangar'
        $atalho.Save()
    }

    # Link hangar:// (convite de sessao compartilhada) abre este app. HKCU: sem administrador.
    $classe = 'HKCU:\Software\Classes\hangar'
    New-Item -Path "$classe\shell\open\command" -Force | Out-Null
    Set-ItemProperty -Path $classe -Name '(default)' -Value 'URL:Hangar'
    Set-ItemProperty -Path $classe -Name 'URL Protocol' -Value ''
    Set-ItemProperty -Path "$classe\shell\open\command" -Name '(default)' -Value ('"{0}" "%1"' -f $app)
    # A marca e a prova do passo de atualizacao: so existe depois da chave gravada.
    New-Item -ItemType Directory -Force -Path $marcaDir | Out-Null
    [IO.File]::WriteAllText((Join-Path $marcaDir 'scheme-hangar'), '')

    # Primeira abertura ja conectada ao backend desta maquina. So quando o app ainda nao tem conexao:
    # a que a pessoa escolheu depois nunca e sobrescrita.
    $cfg = Join-Path $env:APPDATA 'hangar-native'
    # Dados do usuario: ~\.hangar\config quando o arquivo esta la; senao a backend\ (nao migrada).
    $env_ = Join-Path $HOME '.hangar\config\.env'
    if (-not (Test-Path -LiteralPath $env_)) { $env_ = Join-Path $raiz 'backend\.env' }
    $token = ''; $porta = '8765'
    if (Test-Path $env_) {
        foreach ($linha in [IO.File]::ReadAllLines($env_)) {
            if ($linha -match '^CP_AUTH_TOKEN=(.+)$') { $token = $Matches[1].Trim() }
            if ($linha -match '^CP_PORT=(\d+)') { $porta = $Matches[1] }
        }
    }
    if ($token -and -not (Test-Path (Join-Path $cfg 'connection.json'))) {
        New-Item -ItemType Directory -Force -Path $cfg | Out-Null
        [IO.File]::WriteAllText((Join-Path $cfg 'connection.json'), "{`"address`": `"http://127.0.0.1:$porta`", `"token`": `"$token`"}")
    } elseif (-not (Test-Path (Join-Path $cfg 'connection.json'))) {
        Write-Host "app nativo: sem a senha do backend (CP_AUTH_TOKEN) em $env_; na primeira abertura o app pede o endereco e a senha"
    }

    Marca $bruto
    Write-Host "app nativo $versao instalado em $app"
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
