# Garante a libmpv-2.dll do player de video do app nativo ao lado do Hangar.exe: no Windows o app so a procura ali
# (ou em HANGAR_LIBMPV), nunca no PATH. Build fixo do shinchiro/mpv-winbuild-cmake, conferido pelo sha256 do pacote
# e da dll; o aviso de licenca (resources\THIRD_PARTY_NOTICES.md) vai junto. Nunca falha: sem ela o app abre o
# video no player do sistema. A troca do app (update.rs, install-native.ps1) so renomeia o Hangar.exe, e a dll fica.
# A marca ~\.hangar\native\libmpv-ok e a prova do passo de atualizacao e diz o que achou.
# Arquivo so em ASCII: o PowerShell 5.1 le sem BOM como cp1252.
# Uso: powershell -ExecutionPolicy Bypass -File scripts/install-libmpv.ps1 [-Destino <pasta do Hangar.exe>]
param([string]$Destino = '')
$ErrorActionPreference = 'Stop'
$raiz = Split-Path -Parent $PSScriptRoot
if (-not $Destino) { $Destino = Join-Path $env:LOCALAPPDATA 'Programs\Hangar' }

# Mudar a versao e mudar os tres juntos, e o aviso de licenca tambem.
$url = 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-dev-x86_64-20261111-git-bcde6b93cc.7z'
$shaPacote = '273c079b11ea5575822f972756f4c9a71149a5c10130353abeb8586738938452'
$shaDll = '88504ee5560942a955405e99dfb5971368e4cf851321f36565bf2a667271db1b'

$marcaDir = Join-Path $HOME '.hangar\native'
$nome = 'libmpv-2.dll'
$dll = Join-Path $Destino $nome

function Marca($texto) {
    try {
        New-Item -ItemType Directory -Force -Path $marcaDir | Out-Null
        [IO.File]::WriteAllText((Join-Path $marcaDir 'libmpv-ok'), $texto)
    } catch { Write-Host "video do app nativo: nao consegui gravar a marca em $marcaDir ($($_.Exception.Message))" }
}
function Aviso($motivo) {
    Write-Host "video do app nativo: aviso - $motivo. O app abre os videos no player do sistema."
    Write-Host '  Para tentar de novo: scripts\install-libmpv.ps1 da pasta do Hangar (no pacote, %USERPROFILE%\.hangar\current).'
    Marca "falta: $motivo"
}
function Sha($caminho) { (Get-FileHash -Algorithm SHA256 -LiteralPath $caminho).Hash.ToLower() }
# Programa externo: o codigo de saida e o arquivo gerado decidem, nunca o stderr (que no 5.1 com Stop vira excecao).
function Roda($exe, [string[]]$argumentos) {
    $antes = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $exe @argumentos 2>&1 | Out-Null; return $LASTEXITCODE } catch { return -1 } finally { $ErrorActionPreference = $antes }
}

if ($env:HANGAR_LIBMPV -and (Test-Path -LiteralPath $env:HANGAR_LIBMPV)) { Marca "HANGAR_LIBMPV: $env:HANGAR_LIBMPV"; exit 0 }
$arq = "$env:PROCESSOR_ARCHITEW6432$env:PROCESSOR_ARCHITECTURE"
if ($arq -notmatch 'AMD64') { Marca "sem libmpv para Windows $arq"; exit 0 }

$tmp = Join-Path $env:TEMP ('hangar-libmpv.' + [IO.Path]::GetRandomFileName())
try {
    New-Item -ItemType Directory -Force -Path $Destino | Out-Null
    $licenca = Join-Path $raiz 'resources\THIRD_PARTY_NOTICES.md'
    if (Test-Path -LiteralPath $licenca) { Copy-Item -LiteralPath $licenca -Destination (Join-Path $Destino 'THIRD_PARTY_NOTICES.md') -Force }

    if ((Test-Path -LiteralPath $dll) -and (Sha $dll) -eq $shaDll) {
        Marca "pronta: $dll"
        exit 0
    }

    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    # TLS 1.2 explicito (o 5.1 ainda tenta 1.0 e o GitHub recusa) e sem barra de progresso (no 5.1 ela custa mais que o download).
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $ProgressPreference = 'SilentlyContinue'
    $pacote = Join-Path $tmp 'mpv-dev.7z'
    Write-Host 'video do app nativo: baixando a libmpv (cerca de 30 MB)'
    try { Invoke-WebRequest -UseBasicParsing -TimeoutSec 300 -Uri $url -OutFile $pacote | Out-Null }
    catch { Aviso "o download da libmpv falhou ($($_.Exception.Message))"; exit 0 }
    if ((Sha $pacote) -ne $shaPacote) { Aviso 'o sha256 do pacote da libmpv nao confere; nada foi instalado'; exit 0 }

    # O tar.exe do Windows e o bsdtar e le 7z; o 7-Zip, quando ha, fica de reserva.
    $saida = Join-Path $tmp 'x'
    New-Item -ItemType Directory -Force -Path $saida | Out-Null
    $extraida = Join-Path $saida $nome
    $tar = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (Test-Path -LiteralPath $tar) { $null = Roda $tar @('-xf', $pacote, '-C', $saida, $nome) }
    if (-not (Test-Path -LiteralPath $extraida)) {
        $sete = Join-Path $env:ProgramFiles '7-Zip\7z.exe'
        if (Test-Path -LiteralPath $sete) { $null = Roda $sete @('e', '-y', "-o$saida", $pacote, $nome) }
    }
    if (-not (Test-Path -LiteralPath $extraida)) { Aviso 'nao consegui extrair a libmpv-2.dll do pacote (nem o tar.exe nem o 7-Zip)'; exit 0 }
    if ((Sha $extraida) -ne $shaDll) { Aviso 'o sha256 da libmpv-2.dll extraida nao confere; nada foi instalado'; exit 0 }

    # Copia ao lado e rename: o caminho da dll nunca fica pela metade. A dll em uso pelo app aberto nao se
    # sobrescreve, mas se renomeia; os restos de trocas anteriores saem quando da (mesma regra do Hangar.exe).
    $novo = "$dll.new"
    Copy-Item -LiteralPath $extraida -Destination $novo -Force
    $velho = $null
    if (Test-Path -LiteralPath $dll) {
        Get-ChildItem -LiteralPath $Destino -Filter "$nome.old*" -File -Force -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
        $velho = "$dll.old"
        if (Test-Path -LiteralPath $velho) { $velho = "$dll.old-" + [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
        [IO.File]::Move($dll, $velho)
    }
    try { [IO.File]::Move($novo, $dll) }
    catch {
        if ($velho -and (Test-Path -LiteralPath $velho)) { [IO.File]::Move($velho, $dll) }
        Remove-Item -LiteralPath $novo -Force -ErrorAction SilentlyContinue
        throw
    }
    if ($velho) { Remove-Item -LiteralPath $velho -Force -ErrorAction SilentlyContinue }
    if ((Sha $dll) -ne $shaDll) { Aviso "$dll nao ficou no lugar"; exit 0 }
    Marca "instalada: $dll"
    Write-Host "video do app nativo: libmpv em $dll"
} catch {
    Aviso "a instalacao da libmpv falhou ($($_.Exception.Message))"
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
exit 0
