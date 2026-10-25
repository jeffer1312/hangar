# hangar - instalar numa linha so, no Windows.
#
#   irm https://raw.githubusercontent.com/jeffer1312/hangar/main/bootstrap.ps1 | iex
#
# Por padrao instala o PACOTE pronto do canal `latest`, sem git: baixa o hangar-server.exe da release
# `server-<canal>`, confere o sha256 pelo `server-latest.json` dela, roda o `install-package` dele (que
# poe o pacote em ~\.hangar\current) e depois o install.ps1 que vem dentro do pacote.
#
# Opcoes por variavel, definidas ANTES da linha acima (sob `irm | iex` nao existe argumento):
#
#   $env:CP_CANAL = 'beta'       canal de teste (`server-beta`/`package-beta`; pelo git, a branch beta)
#   $env:CP_DEV = '1'            instala pelo git como sempre foi (clona e roda o install.ps1 do
#                                checkout); a maquina fica em modo desenvolvedor quando a migracao
#                                para o pacote chegar
#   $env:CP_DESTINO = 'D:\hangar'  pasta do checkout; vale o git, como o CP_DEV (padrao: $HOME\hangar)
#
# Sistema sem pacote publicado segue pelo git, com o motivo na tela.
#
# O assistente do app nativo roda este arquivo por -File, e ai o param() abaixo recebe as opcoes e
# as repassa ao install.ps1. -Destino e a pasta do Hangar que o app encontrou, e vale o git como o
# CP_DESTINO:
#
#   powershell -ExecutionPolicy Bypass -File bootstrap.ps1 -Destino D:\hangar -App -Tailscale sim
#
# Sob `irm | iex` o script nao sabe o proprio caminho ($MyInvocation.MyCommand.Path vem vazio),
# entao nada aqui depende dele.
#
# Escrito pra Windows PowerShell 5.1 (o que vem no Windows), igual ao install.ps1: nada de
# operador ternario, `??` nem API de .NET Core.
#
# Este arquivo e ASCII puro e SEM BOM: o `irm` do 5.1 nao tira o BOM, e a primeira linha
# viraria "\uFEFF# ..." - que o iex tenta executar como comando.
#
# Instale num disco LOCAL. Numa pasta compartilhada por rede (`\\servidor\...`) o `uv sync` e o
# `npm ci` recriariam `backend\.venv` e `frontend\node_modules` por cima dos da maquina de
# origem - e esses sao dela, nao seus: o venv aponta pro Python daquela maquina e o node_modules
# traz o binario nativo dela. Instalar de uma segunda maquina quebra a instalacao da primeira.
#
# Sem [ValidateSet] no -Tailscale: sob `irm | iex` o param() vira declaracao de variavel, e o
# atributo recusa o valor vazio do padrao ("would no longer be valid"), derrubando o iex inteiro.
param(
    [string]$Destino,
    [switch]$App,
    [string]$Tailscale,
    [switch]$SemNativo,
    [string]$Agentes,
    [switch]$SoChecar,
    [switch]$Sim,
    [switch]$Avancado,
    [switch]$ConsertarRoda
)

$ErrorActionPreference = 'Stop'

# Senha do celular e auxiliar de administrador do app: so o install.ps1 as recebe, nao o git nem o winget.
$appEnv = @{}
foreach ($nome in @('HANGAR_TOKEN', 'HANGAR_ASKPASS', 'HANGAR_ASKPASS_CODE')) {
    $valor = [Environment]::GetEnvironmentVariable($nome)
    if ($valor) { $appEnv[$nome] = $valor }
    Remove-Item "Env:$nome" -ErrorAction SilentlyContinue
}
# So esta execucao diz ao install.ps1 que acabou de criar a instalacao: herdada, marcaria passos pendentes.
Remove-Item Env:HANGAR_FRESH_PACKAGE -ErrorAction SilentlyContinue

$repoUrl = 'https://github.com/jeffer1312/hangar.git'
$ramo    = 'main'
# HANGAR_RELEASE_BASE so existe para o teste servir releases falsas (file://).
$releases = if ($env:HANGAR_RELEASE_BASE) { $env:HANGAR_RELEASE_BASE.TrimEnd('/') } else { 'https://github.com/jeffer1312/hangar/releases/download' }
$canal = if ($env:CP_CANAL) { $env:CP_CANAL } else { 'latest' }
$dev = ($env:CP_DEV -eq '1')
# O USERPROFILE, e nao o $HOME: e a pasta que o hangar-server le.
$perfil = $env:USERPROFILE
$pacote = Join-Path $perfil '.hangar\current'

function Titulo($m) { Write-Host "`n$m" -ForegroundColor Cyan }
function Ok($m)     { Write-Host "  ok  $m" -ForegroundColor Green }
function Nota($m)   { Write-Host "      $m" -ForegroundColor DarkGray }
function Erro($m)   { Write-Host "  X   $m" -ForegroundColor Red }
function Tem($cmd)  { return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

# O codigo vem antes da mensagem (o app troca o texto pela frase e pelo botao dele), e no -App a
# ultima linha diz ao assistente que parou aqui, antes do install.ps1.
function Stop-Bootstrap($mensagem, $codigo) {
    if ($codigo) { Write-Host "##HANGAR-ERRO## $codigo" }
    Erro $mensagem
    if ($App) { Write-Host '##HANGAR-FIM## falhou' }
    exit 1
}
# 'sem-internet' quando o github.com nao responde; vazio = a falha e de outra coisa. Mesma conta do
# install.ps1, repetida porque aqui o repositorio ainda nao existe.
function Get-NetworkCode {
    $c = New-Object Net.Sockets.TcpClient
    try { if ($c.ConnectAsync('github.com', 443).Wait(10000)) { return '' } } catch { } finally { $c.Dispose() }
    return 'sem-internet'
}

function Atualiza-Path {
    # winget grava o PATH no registro, mas o PowerShell JA ABERTO segue com o antigo -> o git
    # recem-instalado "nao existe". Mesmo truque do install.ps1.
    # Expandir e ACRESCENTAR, nunca substituir: PATH gravado como REG_SZ volta com %SYSTEMROOT%
    # cru, e um %VAR% literal dentro de $env:Path nao resolve nada - substituir o PATH por isso
    # some com o proprio powershell.exe.
    $registro = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
                [Environment]::GetEnvironmentVariable('Path', 'User')
    $tudo = ($env:Path + ';' + [Environment]::ExpandEnvironmentVariables($registro)) -split ';' |
            Where-Object { $_ } | Select-Object -Unique
    $env:Path = $tudo -join ';'
}

if ($Tailscale -and @('sim', 'nao') -notcontains $Tailscale) { Stop-Bootstrap '-Tailscale aceita sim ou nao' }
if (@('latest', 'beta') -notcontains $canal) { Stop-Bootstrap "CP_CANAL aceita latest ou beta (veio: $canal)" }
if ($canal -eq 'beta') { $ramo = 'beta' }
# Uma pasta pedida (pelo app ou pelo CP_DESTINO) e um checkout: vale o git, como o CP_DEV.
$pelaPasta = [bool]($Destino -or $env:CP_DESTINO)
if (-not $Destino) { $Destino = $env:CP_DESTINO }
# $destino, abaixo, e o mesmo $Destino: o PowerShell nao diferencia caixa no nome.
if (-not $Destino) { $Destino = Join-Path $perfil 'hangar' }

# $true = o remoto desta pasta e o hangar. Checar o REMOTO, nao so a existencia da
# pasta: "tem um .git aqui" nao quer dizer que e este projeto, e dar pull no repo errado e pior
# que parar.
function EhEsteRepo($pasta) {
    if (-not (Test-Path (Join-Path $pasta '.git')) -or -not (Tem 'git')) { return $false }
    $url = & git -C $pasta remote get-url origin 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $url) { return $false }
    $url = ([string]$url).Trim().TrimEnd('/')
    if ($url.EndsWith('.git')) { $url = $url.Substring(0, $url.Length - 4) }
    return ($url -match 'github\.com[:/]jeffer1312/hangar$')
}

function Clona {
    Titulo "Clonando em $destino"
    & git clone --branch $ramo $repoUrl $destino
    if ($LASTEXITCODE -ne 0) { Stop-Bootstrap 'git clone falhou' (Get-NetworkCode) }
    Ok 'clonado'
}

# Roda o install.ps1 de <pasta> com as opcoes de quem chamou; nao volta.
function Invoke-Installer($pasta) {
    Titulo 'Instalando'
    $instalador = Join-Path $pasta 'install.ps1'
    if (-not (Test-Path $instalador)) { Stop-Bootstrap "install.ps1 nao encontrado em $pasta" }
    Set-Location $pasta
    # O que o app (ou quem chamou por -File) pediu vai inteiro ao install.ps1.
    $repasse = @()
    if ($App)       { $repasse += '-App' }
    if ($Tailscale) { $repasse += @('-Tailscale', $Tailscale) }
    if ($SemNativo) { $repasse += '-SemNativo' }
    if ($Agentes)   { $repasse += @('-Agentes', $Agentes) }
    if ($SoChecar)  { $repasse += '-SoChecar' }
    if ($Sim)       { $repasse += '-Sim' }
    if ($Avancado)  { $repasse += '-Avancado' }
    if ($ConsertarRoda) { $repasse += '-ConsertarRoda' }
    # Num processo proprio com -ExecutionPolicy Bypass: sob `irm | iex` a politica desta sessao
    # pode ser Restricted, e ai um script EM ARQUIVO nao roda. O console e o mesmo, entao os
    # Read-Host do install.ps1 continuam perguntando a voce normalmente.
    # Pelo $PSHOME, nao pelo PATH: e o proprio host que esta rodando este texto. Sob pwsh 7 o
    # $PSHOME nao tem powershell.exe, e ai vale o caminho fixo do 5.1, que todo Windows tem.
    $ps = Join-Path $PSHOME 'powershell.exe'
    if (-not (Test-Path $ps)) { $ps = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" }
    foreach ($nome in $appEnv.Keys) { Set-Item "Env:$nome" $appEnv[$nome] }
    & $ps -NoProfile -ExecutionPolicy Bypass -File $instalador @repasse
    exit $LASTEXITCODE
}

# O caminho pelo git: clona (ou atualiza) o checkout e roda o install.ps1 dele. Nao volta.
function Install-FromGit {
    if (Test-Path -LiteralPath (Join-Path $pacote 'manifest.json')) {
        # A tarefa segue chamando o pacote: o install.ps1 de um checkout por cima misturaria as versoes.
        Stop-Bootstrap 'esta maquina ja roda o Hangar pelo pacote; para usar um checkout: hangar-server dev-mode on <checkout>'
    }
    Atualiza-Path
    if (Tem 'git') {
        Ok 'git'
    } else {
        # Aqui o git nao e opcional como no install.ps1 (la ele so alimenta o painel de branch):
        # sem git nao ha como clonar, e nao ha o que instalar depois.
        if (-not (Tem 'winget')) {
            Nota 'https://git-scm.com/download/win - e rode esta linha de novo'
            Stop-Bootstrap 'sem git e sem winget - instale o "App Installer" pela Microsoft Store, ou o Git' 'sem-winget'
        }
        Write-Host '  .. instalando Git (Git.Git)'
        winget install --id Git.Git --exact --silent `
            --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null
        Atualiza-Path
        if (-not (Tem 'git')) { Stop-Bootstrap 'o Git nao instalou - instale na mao e rode de novo' (Get-NetworkCode) }
        Ok 'Git instalado'
    }

    if (-not (Test-Path $destino)) {
        Clona
    } elseif (EhEsteRepo $destino) {
        Ok "$destino ja e este repositorio - atualizando em vez de clonar"
        # No -App o pull nao roda o hook post-merge: ele faria um -Update inteiro, sem marcas, antes do
        # ##HANGAR-PROTOCOLO##; o install.ps1 -App logo abaixo ja faz o mesmo trabalho.
        $semHooks = @()
        if ($App) { $semHooks = @('-c', ('core.hooksPath=' + (Join-Path $env:TEMP 'hangar-sem-hooks'))) }
        & git @semHooks -C $destino pull --ff-only origin $ramo
        if ($LASTEXITCODE -ne 0) {
            # Mudanca local e o caso comum, e tem frase e botao proprios no app.
            $mudancas = & git -C $destino status --porcelain --untracked-files=no 2>$null
            if ($mudancas) { Stop-Bootstrap "git pull falhou em $destino (ha mudanca local) - resolva na mao e rode de novo" 'checkout-sujo' }
            Stop-Bootstrap "git pull falhou em $destino - resolva na mao e rode de novo" (Get-NetworkCode)
        }
        Ok 'atualizado'
    } elseif (Get-ChildItem -Force -LiteralPath $destino -ErrorAction SilentlyContinue) {
        Nota 'outro destino: $env:CP_DESTINO = "D:\hangar" e rode de novo'
        Stop-Bootstrap "$destino ja existe e NAO e o hangar - nao vou mexer no que e seu"
    } else {
        Clona   # pasta existe mas esta vazia: o git clona pra dentro dela
    }
    # A migracao para o pacote le esta marca e deixa a maquina em modo desenvolvedor.
    if ($dev) {
        New-Item -ItemType Directory -Force -Path (Join-Path $perfil '.hangar') | Out-Null
        [IO.File]::WriteAllText((Join-Path $perfil '.hangar\dev-requested'), '')
    }
    Invoke-Installer $destino
}

# Sistema sem pacote: o motivo na tela e o git.
function Install-WithoutPackage($motivo) {
    Titulo "Sem pacote: $motivo"
    Nota 'instalando pelo git'
    Install-FromGit
}

# Baixa <url> em <arquivo>. https nunca segue redirecionamento para http (o `--proto =https` do
# bootstrap.sh); file:// so serve ao teste.
# $segundos e o teto do download inteiro (o `--max-time` do curl), nao de cada leitura.
function Get-Download([string]$url, [string]$arquivo, [int]$segundos) {
    $https = $url.StartsWith('https://')
    $prazo = [DateTime]::UtcNow.AddSeconds($segundos)
    $resta = { $ms = [int]($prazo - [DateTime]::UtcNow).TotalMilliseconds; if ($ms -le 0) { throw "o download passou de $segundos s" }; $ms }
    for ($salto = 0; $salto -lt 6; $salto++) {
        $req = [Net.WebRequest]::Create($url)
        $req.Timeout = & $resta
        if ($req -is [Net.HttpWebRequest]) {
            $req.AllowAutoRedirect = $false
            $req.ReadWriteTimeout = & $resta
            $req.UserAgent = 'hangar-bootstrap'
        }
        $resp = $null
        try {
            try { $resp = $req.GetResponse() }
            catch [Net.WebException] { $resp = $_.Exception.Response; if (-not $resp) { throw } }
            if ($resp -is [Net.HttpWebResponse]) {
                $status = [int]$resp.StatusCode
                if ($status -ge 300 -and $status -lt 400) {
                    $url = (New-Object Uri ((New-Object Uri $url), $resp.Headers['Location'])).AbsoluteUri
                    if ($https -and -not $url.StartsWith('https://')) { throw "redirecionamento para fora do https: $url" }
                    continue
                }
                if ($status -ge 400) { throw "HTTP $status" }
            }
            $fluxo = $resp.GetResponseStream()
            $gravado = [IO.File]::Create($arquivo)
            try {
                $buf = New-Object byte[] 65536
                while ($true) {
                    if ($fluxo.CanTimeout) { $fluxo.ReadTimeout = & $resta }
                    $n = $fluxo.Read($buf, 0, $buf.Length)
                    if ($n -le 0) { break }
                    $gravado.Write($buf, 0, $n)
                    $null = & $resta
                }
            } finally { $gravado.Dispose(); $fluxo.Dispose() }
            return
        } finally { if ($resp) { $resp.Close() } }
    }
    throw "redirecionamentos demais em $url"
}

$jaInstalado = 'ja ha uma instalacao do Hangar nesta maquina: para atualizar, use o Atualizar do app; para seguir pelo git, rode com $env:CP_DEV = ''1'''

# O caminho pelo pacote. Nao volta.
function Install-FromPackage {
    # Re-rodar continua de onde parou: o pacote ja instalado roda o instalador dele de novo.
    if ((Test-Path -LiteralPath (Join-Path $pacote 'manifest.json')) -and -not (Test-Path -LiteralPath (Join-Path $pacote '.git'))) {
        Ok 'o Hangar ja esta instalado pelo pacote - rodando o instalador dele de novo (atualizar e pelo app)'
        Invoke-Installer $pacote
    }
    if ($SoChecar) { Stop-Bootstrap 'nao ha instalacao pelo pacote para conferir - rode sem -SoChecar para instalar' }
    # Checkout instalado e nunca iniciado nao deixa tarefa nem anotacao para o install-package ver.
    if (EhEsteRepo $destino) { Stop-Bootstrap "o Hangar esta em $destino, pelo git; $jaInstalado" }
    if ("$env:PROCESSOR_ARCHITEW6432$env:PROCESSOR_ARCHITECTURE" -notmatch 'AMD64') {
        Install-WithoutPackage "sem pacote para Windows $env:PROCESSOR_ARCHITECTURE"
    }
    $plat = 'windows-x86_64'

    Titulo "Baixando o Hangar (canal $canal)"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $url = "$releases/server-$canal"
    # Em ~\.hangar e nao no %TEMP%: o mesmo lugar do bootstrap.sh, e o hangar-server sai dali no fim.
    $cache = Join-Path $perfil '.hangar\cache'
    New-Item -ItemType Directory -Force -Path $cache | Out-Null
    $tmp = Join-Path $cache ('bootstrap.' + [IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        # O indice antes do hangar-server: num canal sem pacote, a release server-<canal> pode ter um
        # binario de antes do `install-package`, que subiria como servidor em vez de dizer o motivo.
        $urlIndice = "$releases/package-$canal/package-$canal.json"
        try { Get-Download $urlIndice (Join-Path $tmp 'indice.json') 60 }
        catch {
            if ($_.Exception.Message -eq 'HTTP 404') { Install-WithoutPackage "o canal $canal ainda nao publica pacote ($urlIndice nao existe)" }
            Stop-Bootstrap "nao consegui ler $urlIndice ($($_.Exception.Message))" (Get-NetworkCode)
        }
        try { $indice = [IO.File]::ReadAllText((Join-Path $tmp 'indice.json')) | ConvertFrom-Json }
        catch { Stop-Bootstrap "o $urlIndice nao e JSON legivel: $($_.Exception.Message)" }
        if (-not ($indice.platforms -and $indice.platforms.PSObject.Properties[$plat])) {
            Install-WithoutPackage "o canal $canal nao tem pacote para $plat"
        }
        try { Get-Download "$url/server-latest.json" (Join-Path $tmp 'server-latest.json') 60 }
        catch { Stop-Bootstrap "nao consegui ler $url/server-latest.json ($($_.Exception.Message))" (Get-NetworkCode) }
        try { $manifesto = [IO.File]::ReadAllText((Join-Path $tmp 'server-latest.json')) | ConvertFrom-Json }
        catch { Stop-Bootstrap "o $url/server-latest.json nao e JSON legivel: $($_.Exception.Message)" }
        $entrada = $null
        if ($manifesto.files) { $entrada = $manifesto.files.PSObject.Properties["$plat/hangar-server"] }
        if (-not $entrada) { Install-WithoutPackage "a release server-$canal nao publica o hangar-server para $plat" }
        $nome = "$($entrada.Value.name)"
        $sha = "$($entrada.Value.sha256)".ToLower()
        if ($nome -notmatch '^[A-Za-z0-9._-]+$' -or $sha -notmatch '^[0-9a-f]{64}$') {
            Stop-Bootstrap "a release server-$canal tem o hangar-server para $plat, mas nao consegui ler o nome e o sha256 da entrada: $($entrada.Value | ConvertTo-Json -Compress)"
        }
        $exe = Join-Path $tmp 'hangar-server.exe'
        try { Get-Download "$url/$nome" $exe 600 }
        catch { Stop-Bootstrap "o download de $nome falhou ($($_.Exception.Message))" (Get-NetworkCode) }
        $recebido = (Get-FileHash -Algorithm SHA256 -LiteralPath $exe).Hash.ToLower()
        if ($recebido -ne $sha) { Stop-Bootstrap "o hangar-server baixado nao confere com o publicado (sha256 $recebido, esperado $sha)" }
        Ok 'hangar-server conferido (sha256)'

        Nota 'baixando e conferindo o pacote (pode levar alguns minutos)'
        # A saida aparece enquanto roda; a ultima linha e o motivo quando ele sai 3.
        $linhas = @(& { $ErrorActionPreference = 'Continue'; & $exe install-package --channel $canal --release-base $releases 2>&1 } |
                    ForEach-Object { $l = "$_"; Write-Host "  $l"; $l })
        $rc = $LASTEXITCODE
    } finally {
        Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
    }
    switch ($rc) {
        0 { }
        # 3 = o indice nao tem pacote para este sistema; a ultima linha e o motivo.
        3 { Install-WithoutPackage (@($linhas | Where-Object { "$_".Trim() }) | Select-Object -Last 1) }
        4 { Stop-Bootstrap $jaInstalado }
        default { Stop-Bootstrap 'a instalacao do pacote parou (motivo acima)' }
    }
    # Instalacao criada agora: o install.ps1 marca os passos de atualizacao como ja aplicados.
    $env:HANGAR_FRESH_PACKAGE = '1'
    Invoke-Installer $pacote
}

Titulo 'hangar - instalacao em uma linha'
if ($dev -or $pelaPasta) {
    Install-FromGit
} else {
    Install-FromPackage
}
