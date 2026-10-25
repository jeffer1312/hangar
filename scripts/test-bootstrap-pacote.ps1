# Testa o bootstrap.ps1 no caminho do pacote sem rede: releases falsas por file://, um hangar-server.exe
# falso (compilado aqui) que grava os argumentos e poe um install.ps1 falso no ~\.hangar\current, um
# git falso (git.cmd) para o caminho pelo git e um USERPROFILE temporario. Uso (Windows PowerShell 5.1):
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-bootstrap-pacote.ps1
$ErrorActionPreference = 'Continue'
$bootstrap = Join-Path (Split-Path -Parent $PSScriptRoot) 'bootstrap.ps1'
$ps = Join-Path $PSHOME 'powershell.exe'
$raiz = Join-Path $env:TEMP ('hangar-bootstrap-pacote-' + [guid]::NewGuid().ToString('N'))
$fakeBin = Join-Path $raiz 'bin'
$perfil = Join-Path $raiz 'perfil'
$releases = Join-Path $raiz 'releases'
$log = Join-Path $raiz 'chamadas.log'
$fakeInstall = Join-Path $raiz 'install-falso.ps1'
New-Item -ItemType Directory -Force -Path $fakeBin, $releases | Out-Null
$pathOriginal = $env:Path
$perfilOriginal = $env:USERPROFILE
$script:falhas = 0
function Assert($condition, $message) { if ($condition) { Write-Host "ok   $message" } else { Write-Host "FAIL $message"; $script:falhas++ } }

# O hangar-server falso: FAKE_SERVER_RC decide a saida; com 0 instala um "pacote" com o install.ps1 falso.
$fakeServer = Join-Path $raiz 'hangar-server-windows-x86_64.exe'
Add-Type -OutputType ConsoleApplication -OutputAssembly $fakeServer -TypeDefinition @'
using System; using System.IO;
public static class FakeServer {
    public static int Main(string[] args) {
        string log = Environment.GetEnvironmentVariable("FAKE_LOG");
        File.AppendAllText(log, "server " + string.Join(" ", args) + "\r\n");
        if (Environment.GetEnvironmentVariable("HANGAR_TOKEN") != null) File.AppendAllText(log, "server viu o segredo\r\n");
        string rcText = Environment.GetEnvironmentVariable("FAKE_SERVER_RC");
        int rc = string.IsNullOrEmpty(rcText) ? 0 : int.Parse(rcText);
        if (rc != 0) { Console.Error.WriteLine("motivo do servidor falso " + rc); return rc; }
        string cur = Path.Combine(Environment.GetEnvironmentVariable("USERPROFILE"), ".hangar", "current");
        Directory.CreateDirectory(cur);
        File.WriteAllText(Path.Combine(cur, "manifest.json"), "{}");
        File.Copy(Environment.GetEnvironmentVariable("FAKE_INSTALL_SRC"), Path.Combine(cur, "install.ps1"), true);
        Console.WriteLine("instalado: versao 0.1.0.42");
        return 0;
    }
}
'@
$shaServer = (Get-FileHash -Algorithm SHA256 -LiteralPath $fakeServer).Hash.ToLower()

# git falso: o clone cria o destino com o install.ps1 falso; o remoto e sempre o do hangar.
Set-Content -Encoding ASCII -Path (Join-Path $fakeBin 'git.cmd') -Value @'
@echo off
>>"%FAKE_LOG%" echo git %*
if "%~3"=="remote" (echo https://github.com/jeffer1312/hangar.git& exit /b 0)
if "%~1"=="clone" goto clone
if "%~1"=="--version" (echo git version 2.50.0.windows.1& exit /b 0)
exit /b 0
:clone
mkdir "%~5\.git"
copy /y "%FAKE_INSTALL_SRC%" "%~5\install.ps1" >nul
exit /b 0
'@
Set-Content -Encoding ASCII -Path $fakeInstall -Value @'
param([switch]$App, [string]$Tailscale, [switch]$SemNativo, [string]$Agentes, [switch]$SoChecar, [switch]$Sim, [switch]$Avancado, [switch]$ConsertarRoda)
Add-Content -Encoding ASCII -Path $env:FAKE_LOG -Value "install $PSScriptRoot App=$App Tailscale=$Tailscale SoChecar=$SoChecar tok=$env:HANGAR_TOKEN"
Add-Content -Encoding ASCII -Path $env:FAKE_LOG -Value "fresh=$env:HANGAR_FRESH_PACKAGE"
exit 0
'@

# Publica o indice package-<canal>.json com a entrada de $plat.
function Publish-Index([string]$canal, [string]$plat = 'windows-x86_64') {
    $dir = Join-Path $releases "package-$canal"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $json = @{ platforms = @{ $plat = @{ version = '0.1.0.42' } } } | ConvertTo-Json -Depth 5
    [IO.File]::WriteAllText((Join-Path $dir "package-$canal.json"), $json)
}
# Publica a release server-<canal>; $entrada troca a entrada do sistema (vazio = sem entrada); $indice e a
# plataforma do indice do pacote ($null = sem indice).
function Publish([string]$canal, $entrada = 'boa', $indice = 'windows-x86_64') {
    if ($indice) { Publish-Index $canal $indice }
    $dir = Join-Path $releases "server-$canal"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item -Force $fakeServer (Join-Path $dir 'hangar-server-windows-x86_64.exe')
    $files = @{ 'linux-x86_64/hangar-server' = @{ name = 'hangar-server-linux-x86_64'; sha256 = ('a' * 64) } }
    if ($entrada -eq 'boa') { $files['windows-x86_64/hangar-server'] = @{ name = 'hangar-server-windows-x86_64.exe'; sha256 = $shaServer } }
    elseif ($entrada) { $files['windows-x86_64/hangar-server'] = $entrada }
    $json = @{ platform = 'windows-x86_64'; files = $files } | ConvertTo-Json -Depth 5
    [IO.File]::WriteAllText((Join-Path $dir 'server-latest.json'), $json)
}
function Reset-Machine {
    Remove-Item -Recurse -Force $perfil, $log -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force (Join-Path $releases '*') -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $perfil | Out-Null
}
function Run-Bootstrap([string[]]$argumentos, [hashtable]$vars = @{}) {
    $env:USERPROFILE = $perfil
    $env:FAKE_LOG = $log; $env:FAKE_INSTALL_SRC = $fakeInstall
    $env:HANGAR_RELEASE_BASE = 'file:///' + ($releases -replace '\\', '/')
    $env:Path = "$fakeBin;$pathOriginal"
    foreach ($k in $vars.Keys) { Set-Item "Env:$k" $vars[$k] }
    $saida = @(& $ps -NoProfile -ExecutionPolicy Bypass -File $bootstrap @argumentos 2>&1 | ForEach-Object { "$_" })
    foreach ($k in $vars.Keys) { Remove-Item "Env:$k" -ErrorAction SilentlyContinue }
    $env:Path = $pathOriginal; $env:USERPROFILE = $perfilOriginal
    return ,$saida
}
function Calls { if (Test-Path $log) { return @(Get-Content $log) } else { return @() } }
function Last-Line($linhas) { return (@($linhas | Where-Object { "$_".Trim() }) | Select-Object -Last 1) }
$current = Join-Path $perfil '.hangar\current'

# --- Padrao: pacote do canal latest, opcoes e senha so para o install.ps1 do pacote ---
Reset-Machine; Publish 'latest'
$out = Run-Bootstrap @('-App', '-Tailscale', 'nao') @{ HANGAR_TOKEN = 'segredo-123' }
$calls = Calls
Assert ($calls -contains "server install-package --channel latest --release-base file:///$($releases -replace '\\', '/')") 'padrao: install-package do canal latest'
Assert ($calls -contains "install $current App=True Tailscale=nao SoChecar=False tok=segredo-123") 'padrao: roda o install.ps1 do pacote com as opcoes e a senha'
Assert (-not ($calls -contains 'server viu o segredo')) 'padrao: o hangar-server nao ve a senha'
Assert ($calls -contains 'fresh=1') 'padrao: o install.ps1 sabe que a instalacao e nova'
Assert (-not ($calls | Where-Object { $_ -like 'git *' })) 'padrao: nenhum git'
Assert (-not (Get-ChildItem (Join-Path $perfil '.hangar\cache') -Filter 'bootstrap.*' -ErrorAction SilentlyContinue)) 'padrao: o temporario sai'
Assert (($out -join "`n") -match 'conferido \(sha256\)') 'padrao: diz que conferiu'

# --- CP_CANAL=beta: release server-beta e canal beta ---
Reset-Machine; Publish 'beta'
$out = Run-Bootstrap @() @{ CP_CANAL = 'beta' }
Assert ((Calls) -contains "server install-package --channel beta --release-base file:///$($releases -replace '\\', '/')") 'beta: canal beta'
Assert ((Calls) -contains "install $current App=False Tailscale= SoChecar=False tok=") 'beta: instalador do pacote'
Reset-Machine
$out = Run-Bootstrap @('-App') @{ CP_CANAL = 'outro' }
Assert (($out -join "`n") -match 'CP_CANAL aceita latest ou beta') 'canal invalido: recusa'
Assert ((Last-Line $out) -eq '##HANGAR-FIM## falhou') 'canal invalido: FIM falhou'

# --- sha256 que nao confere: o binario nao roda ---
Reset-Machine; Publish 'latest' @{ name = 'hangar-server-windows-x86_64.exe'; sha256 = ('0' * 64) }
$out = Run-Bootstrap @('-App')
Assert (($out -join "`n") -match 'nao confere com o publicado') 'sha errado: diz o motivo'
Assert (-not ((Calls) | Where-Object { $_ -like 'server *' })) 'sha errado: o binario nao rodou'
Assert (-not (Test-Path $current)) 'sha errado: nada instalado'
Assert ((Last-Line $out) -eq '##HANGAR-FIM## falhou') 'sha errado: FIM falhou'

# --- Entrada ilegivel: falha visivel, sem cair no git ---
Reset-Machine; Publish 'latest' @{ name = 'hangar-server-windows-x86_64.exe'; sha256 = 'xyz' }
$out = Run-Bootstrap @()
Assert (($out -join "`n") -match 'nao consegui ler o nome e o sha256') 'entrada ilegivel: falha visivel'
Assert (-not ((Calls) | Where-Object { $_ -like 'git *' })) 'entrada ilegivel: sem git'

# --- server-latest.json ausente: falha visivel ---
Reset-Machine; Publish-Index 'latest'
$out = Run-Bootstrap @()
Assert (($out -join "`n") -match 'nao consegui ler .*server-latest\.json') 'sem manifesto: falha visivel'

# --- Canal sem indice de pacote (HTTP 404): git com o motivo, sem baixar o hangar-server ---
Reset-Machine; Publish 'latest' 'boa' $null
$escuta404 = New-Object Net.Sockets.TcpListener ([Net.IPAddress]::Loopback, 0)
$escuta404.Start()
$servidor404 = [powershell]::Create().AddScript({
    param($escuta)
    $c = $escuta.AcceptTcpClient(); $s = $c.GetStream()
    $r = New-Object IO.StreamReader($s); while ($r.ReadLine()) { }
    $h = [Text.Encoding]::ASCII.GetBytes("HTTP/1.1 404 Not Found`r`nContent-Length: 0`r`nConnection: close`r`n`r`n")
    $s.Write($h, 0, $h.Length); $s.Flush(); $c.Close()
}).AddArgument($escuta404)
$rodando404 = $servidor404.BeginInvoke()
$out = Run-Bootstrap @() @{ HANGAR_RELEASE_BASE = "http://127.0.0.1:$($escuta404.LocalEndpoint.Port)" }
$escuta404.Stop(); try { $null = $servidor404.EndInvoke($rodando404) } catch { }; $servidor404.Dispose()
Assert (($out -join "`n") -match 'Sem pacote: o canal latest ainda nao publica pacote') 'sem indice: motivo na tela'
Assert (-not ((Calls) | Where-Object { $_ -like 'server *' })) 'sem indice: o hangar-server nao rodou'
Assert ((Calls) -contains "install $(Join-Path $perfil 'hangar') App=False Tailscale= SoChecar=False tok=") 'sem indice: instalador do checkout'

# --- Indice sem o Windows: git com o motivo, sem baixar o hangar-server ---
Reset-Machine; Publish 'latest' 'boa' 'linux-x86_64'
$out = Run-Bootstrap @()
Assert (($out -join "`n") -match 'Sem pacote: o canal latest nao tem pacote para windows-x86_64') 'indice sem o sistema: motivo na tela'
Assert (-not ((Calls) | Where-Object { $_ -like 'server *' })) 'indice sem o sistema: o hangar-server nao rodou'
Assert ((Calls) | Where-Object { $_ -like 'git clone --branch main *' }) 'indice sem o sistema: segue pelo git'

# --- Indice que nao se le (porta fechada): falha visivel, sem cair no git ---
Reset-Machine; Publish 'latest'
$fechada = New-Object Net.Sockets.TcpListener ([Net.IPAddress]::Loopback, 0)
$fechada.Start(); $portaFechada = $fechada.LocalEndpoint.Port; $fechada.Stop()
$out = Run-Bootstrap @('-App') @{ HANGAR_RELEASE_BASE = "http://127.0.0.1:$portaFechada" }
Assert (($out -join "`n") -match 'nao consegui ler .*package-latest\.json') 'indice ilegivel: falha visivel'
Assert (-not ((Calls) | Where-Object { $_ -like 'git *' -or $_ -like 'server *' })) 'indice ilegivel: nem git nem hangar-server'
Assert ((Last-Line $out) -eq '##HANGAR-FIM## falhou') 'indice ilegivel: FIM falhou'

# --- Release sem o hangar-server do Windows: git, sem baixar nada ---
Reset-Machine; Publish 'latest' $null
$out = Run-Bootstrap @()
Assert (($out -join "`n") -match 'Sem pacote: a release server-latest nao publica o hangar-server para windows-x86_64') 'sem entrada: motivo na tela'
Assert ((Calls) | Where-Object { $_ -like 'git clone --branch main *' }) 'sem entrada: segue pelo git'
Assert ((Calls) -contains "install $(Join-Path $perfil 'hangar') App=False Tailscale= SoChecar=False tok=") 'sem entrada: instalador do checkout'

# --- install-package sai 3 (o indice nao tem pacote): git com o motivo dele ---
Reset-Machine; Publish 'latest'
$out = Run-Bootstrap @() @{ FAKE_SERVER_RC = '3' }
Assert (($out -join "`n") -match 'Sem pacote: motivo do servidor falso 3') 'rc 3: motivo da ultima linha'
Assert ((Calls) | Where-Object { $_ -like 'git clone *' }) 'rc 3: segue pelo git'
Assert ((Calls) -contains 'fresh=') 'rc 3: checkout sem marca de instalacao nova'

# --- install-package sai 4 (instalacao existente): para com o motivo ---
Reset-Machine; Publish 'latest'
$out = Run-Bootstrap @() @{ FAKE_SERVER_RC = '4' }
Assert (($out -join "`n") -match 'ja ha uma instalacao do Hangar') 'rc 4: ja instalado'
Assert (-not ((Calls) | Where-Object { $_ -like 'git *' -or $_ -like 'install *' })) 'rc 4: nem git nem instalador'

# --- install-package falha (1): para ---
Reset-Machine; Publish 'latest'
$out = Run-Bootstrap @() @{ FAKE_SERVER_RC = '1' }
Assert (($out -join "`n") -match 'a instalacao do pacote parou') 'rc 1: para'

# --- Ja instalado pelo pacote: roda o install.ps1 dele de novo, sem baixar ---
Reset-Machine
New-Item -ItemType Directory -Force -Path $current | Out-Null
'{}' | Set-Content -Encoding ASCII (Join-Path $current 'manifest.json')
Copy-Item $fakeInstall (Join-Path $current 'install.ps1')
$out = Run-Bootstrap @('-SoChecar') @{ HANGAR_FRESH_PACKAGE = '1' }
Assert ((Calls) -contains "install $current App=False Tailscale= SoChecar=True tok=") 'ja no pacote: roda o instalador dele (com -SoChecar)'
Assert ((Calls) -contains 'fresh=') 'ja no pacote: install.ps1 nao marca passos (nem com a variavel herdada)'
Assert (-not ((Calls) | Where-Object { $_ -like 'server *' })) 'ja no pacote: nada baixado'

# --- CP_DEV ou pasta numa maquina ja no pacote: recusa, o caminho e o dev-mode ---
$out = Run-Bootstrap @() @{ CP_DEV = '1' }
Assert (($out -join "`n") -match 'hangar-server dev-mode on') 'dev no pacote: aponta o dev-mode'
$out = Run-Bootstrap @('-Destino', (Join-Path $raiz 'outro'))
Assert (($out -join "`n") -match 'hangar-server dev-mode on') 'pasta no pacote: mesma recusa'
Assert (-not ((Calls) | Where-Object { $_ -like 'git clone *' })) 'pacote: nada clonado por cima'

# --- CP_DEV=1: pelo git, e a migracao vai deixar a maquina em modo desenvolvedor ---
Reset-Machine; Publish 'latest'
$out = Run-Bootstrap @() @{ CP_DEV = '1'; CP_CANAL = 'beta' }
Assert ((Calls) | Where-Object { $_ -like 'git clone --branch beta *' }) 'dev: clona a branch beta'
Assert (Test-Path (Join-Path $perfil '.hangar\dev-requested')) 'dev: marca dev-requested'
Assert (-not ((Calls) | Where-Object { $_ -like 'server *' })) 'dev: sem pacote'

# --- Checkout do Hangar na pasta padrao: para antes do install-package ---
Reset-Machine; Publish 'latest'
New-Item -ItemType Directory -Force -Path (Join-Path $perfil 'hangar\.git') | Out-Null
$out = Run-Bootstrap @()
Assert (($out -join "`n") -match 'pelo git; ja ha uma instalacao') 'checkout na pasta padrao: recusa'
Assert (-not ((Calls) | Where-Object { $_ -like 'server *' })) 'checkout na pasta padrao: nada baixado'

# --- -SoChecar sem instalacao pelo pacote: nao instala nada ---
Reset-Machine; Publish 'latest'
$out = Run-Bootstrap @('-SoChecar')
Assert (($out -join "`n") -match 'nao ha instalacao pelo pacote para conferir') 'check sem pacote: recusa'
Assert (-not (Test-Path $current)) 'check sem pacote: nada instalado'

# --- irm | iex: o pacote pelo padrao, sem argumento nenhum ---
Reset-Machine; Publish 'latest'
$env:USERPROFILE = $perfil; $env:FAKE_LOG = $log; $env:FAKE_INSTALL_SRC = $fakeInstall
$env:HANGAR_RELEASE_BASE = 'file:///' + ($releases -replace '\\', '/'); $env:Path = "$fakeBin;$pathOriginal"
& $ps -NoProfile -ExecutionPolicy Bypass -Command "Get-Content -Raw '$bootstrap' | iex" *> $null
$env:Path = $pathOriginal; $env:USERPROFILE = $perfilOriginal
Assert ((Calls) -contains "install $current App=False Tailscale= SoChecar=False tok=") 'iex: instala pelo pacote'

# --- Download que pinga um byte a cada meio segundo: o teto vale para o download inteiro ---
$tk = $null; $pe = $null
$bAst = [Management.Automation.Language.Parser]::ParseFile($bootstrap, [ref]$tk, [ref]$pe)
. ([scriptblock]::Create($bAst.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Get-Download' }, $true).Extent.Text))
$escuta = New-Object Net.Sockets.TcpListener ([Net.IPAddress]::Loopback, 0)
$escuta.Start()
$lento = [powershell]::Create().AddScript({
    param($escuta)
    $c = $escuta.AcceptTcpClient(); $s = $c.GetStream()
    $h = [Text.Encoding]::ASCII.GetBytes("HTTP/1.1 200 OK`r`nContent-Length: 100000`r`n`r`n")
    $s.Write($h, 0, $h.Length)
    for ($i = 0; $i -lt 40; $i++) { Start-Sleep -Milliseconds 500; try { $s.WriteByte(65); $s.Flush() } catch { break } }
    $c.Close()
}).AddArgument($escuta)
$rodando = $lento.BeginInvoke()
$relogio = [Diagnostics.Stopwatch]::StartNew()
$erroLento = ''
try { Get-Download "http://127.0.0.1:$($escuta.LocalEndpoint.Port)/x" (Join-Path $raiz 'lento.bin') 3 } catch { $erroLento = "$_" }
$segundosLento = $relogio.Elapsed.TotalSeconds
$escuta.Stop(); $null = $lento.EndInvoke($rodando); $lento.Dispose()
# O prazo vence no meio de uma leitura (timeout dela) ou entre duas (o teto do Get-Download).
Assert ($erroLento -match 'passou de 3 s|timed out') "download lento: para no teto total ($erroLento)"
Assert ($segundosLento -lt 8) "download lento: para perto dos 3 s (levou $([int]$segundosLento) s)"

Remove-Item Env:HANGAR_RELEASE_BASE, Env:FAKE_LOG, Env:FAKE_INSTALL_SRC -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force $raiz -ErrorAction SilentlyContinue
if ($script:falhas) { Write-Host "$($script:falhas) falha(s)"; exit 1 }
Write-Host 'tudo ok'
