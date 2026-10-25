# Testa o bootstrap.ps1 sem clonar nada: um git falso (git.cmd) na frente do PATH e um install.ps1
# falso no destino, que grava os parametros recebidos. Uso (Windows PowerShell 5.1):
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-bootstrap-app.ps1
$ErrorActionPreference = 'Continue'
$bootstrap = Join-Path (Split-Path -Parent $PSScriptRoot) 'bootstrap.ps1'
$ps = Join-Path $PSHOME 'powershell.exe'
$raiz = Join-Path $env:TEMP ('hangar-bootstrap-teste-' + [guid]::NewGuid().ToString('N'))
$fakeBin = Join-Path $raiz 'bin'
$perfil = Join-Path $raiz 'perfil'
New-Item -ItemType Directory -Force -Path $fakeBin, $perfil | Out-Null
$pathOriginal = $env:Path
$perfilOriginal = $env:USERPROFILE
$script:falhas = 0
function Assert($condition, $message) { if ($condition) { Write-Host "ok   $message" } else { Write-Host "FAIL $message"; $script:falhas++ } }

Set-Content -Encoding ASCII -Path (Join-Path $fakeBin 'git.cmd') -Value @'
@echo off
>>"%FAKE_LOG%" echo git %*
if defined HANGAR_TOKEN >>"%FAKE_LOG%" echo git viu o segredo
echo %* | findstr /c:"pull --ff-only" >nul && exit /b %FAKE_PULL_RC%
if "%~3"=="remote" (echo https://github.com/jeffer1312/hangar.git& exit /b 0)
if "%~3"=="status" goto status
if "%~1"=="clone" exit /b %FAKE_CLONE_RC%
if "%~1"=="--version" (echo git version 2.50.0.windows.1& exit /b 0)
exit /b 0
:status
if "%FAKE_DIRTY%"=="1" echo  M install.ps1
exit /b 0
'@
$fakeInstall = @'
param([switch]$App, [string]$Tailscale, [switch]$SemNativo, [string]$Agentes, [switch]$SoChecar, [switch]$Sim, [switch]$Avancado, [switch]$ConsertarRoda)
"App=$App Tailscale=$Tailscale SemNativo=$SemNativo Agentes=$Agentes SoChecar=$SoChecar ConsertarRoda=$ConsertarRoda" | Set-Content -Encoding ASCII -Path (Join-Path $PSScriptRoot 'args.txt')
"tok=$env:HANGAR_TOKEN" | Set-Content -Encoding ASCII -Path (Join-Path $PSScriptRoot 'tok.txt')
exit 0
'@
function New-Destino($nome) {
    $d = Join-Path $raiz $nome
    New-Item -ItemType Directory -Force -Path (Join-Path $d '.git') | Out-Null
    Set-Content -Encoding ASCII -Path (Join-Path $d 'install.ps1') -Value $fakeInstall
    return $d
}
function Run-Bootstrap([string[]]$argumentos, [int]$pull = 0, [int]$dirty = 0, [int]$clone = 0) {
    $env:FAKE_LOG = Join-Path $raiz 'git.log'
    $env:FAKE_PULL_RC = "$pull"; $env:FAKE_DIRTY = "$dirty"; $env:FAKE_CLONE_RC = "$clone"
    $env:Path = "$fakeBin;$pathOriginal"
    # Perfil vazio: o ~\.hangar\current de verdade (maquina no pacote) faria o bootstrap recusar o git.
    $env:USERPROFILE = $perfil
    $saida = @(& $ps -NoProfile -ExecutionPolicy Bypass -File $bootstrap @argumentos 2>&1 | ForEach-Object { "$_" })
    $env:Path = $pathOriginal; $env:USERPROFILE = $perfilOriginal
    return ,$saida
}
function Last-Line($linhas) { return (@($linhas | Where-Object { "$_".Trim() }) | Select-Object -Last 1) }

# --- Caso: repasse das opcoes por -File ---
$d = New-Destino 'repasse'
$out = Run-Bootstrap @('-Destino', $d, '-App', '-Tailscale', 'nao', '-SemNativo', '-Agentes', 'codex,pi', '-ConsertarRoda')
Assert ((Get-Content (Join-Path $d 'args.txt')) -eq 'App=True Tailscale=nao SemNativo=True Agentes=codex,pi SoChecar=False ConsertarRoda=True') 'repasse: opcoes intactas, -Agentes com virgula'

# --- Caso: no -App o pull nao roda o hook; a senha chega ao install.ps1 e nao ao git ---
$d = New-Destino 'hook'
$env:HANGAR_TOKEN = 'segredo-123'; $env:HANGAR_ASKPASS = 'C:\x\askpass'
$out = Run-Bootstrap @('-Destino', $d, '-App')
Remove-Item Env:HANGAR_TOKEN, Env:HANGAR_ASKPASS
$gitLog = Get-Content (Join-Path $raiz 'git.log') -Raw
Assert ($gitLog -match 'core\.hooksPath=.*pull --ff-only') 'hook: -App puxa sem hooks'
Assert ($gitLog -notmatch 'git viu o segredo') 'hook: git nao ve a senha'
Assert ((Get-Content (Join-Path $d 'tok.txt')) -eq 'tok=segredo-123') 'hook: install.ps1 recebe a senha'
Remove-Item (Join-Path $raiz 'git.log')
$d = New-Destino 'hook-terminal'
$out = Run-Bootstrap @('-Destino', $d)
Assert ((Get-Content (Join-Path $raiz 'git.log') -Raw) -notmatch 'core\.hooksPath') 'hook: sem -App o hook segue valendo'

# --- Caso: mudanca local barra o pull ---
$d = New-Destino 'sujo'
$out = Run-Bootstrap @('-Destino', $d, '-App') -pull 1 -dirty 1
Assert ($out -contains '##HANGAR-ERRO## checkout-sujo') 'sujo: codigo'
Assert ((Last-Line $out) -eq '##HANGAR-FIM## falhou') 'sujo: FIM falhou por ultimo'
Assert (-not (Test-Path (Join-Path $d 'args.txt'))) 'sujo: install.ps1 nao roda'

# --- Caso: sem -App o codigo sai, a FIM nao ---
$d = New-Destino 'terminal'
$out = Run-Bootstrap @('-Destino', $d) -pull 1 -dirty 1
Assert ($out -contains '##HANGAR-ERRO## checkout-sujo') 'terminal: codigo tambem aqui'
Assert (-not ($out -contains '##HANGAR-FIM## falhou')) 'terminal: sem FIM'

# --- Caso: clone que falha ---
$out = Run-Bootstrap @('-Destino', (Join-Path $raiz 'novo'), '-App') -clone 128
Assert (($out -join "`n") -match 'git clone falhou') 'clone: diz o que falhou'
Assert ((Last-Line $out) -eq '##HANGAR-FIM## falhou') 'clone: FIM falhou por ultimo'

# --- Caso: irm | iex continua funcionando (param() no padrao, destino do CP_DESTINO) ---
$d = New-Destino 'iex'
$env:Path = "$fakeBin;$pathOriginal"; $env:FAKE_PULL_RC = '0'; $env:FAKE_DIRTY = '0'; $env:FAKE_CLONE_RC = '0'
$env:CP_DESTINO = $d; $env:USERPROFILE = $perfil
& $ps -NoProfile -ExecutionPolicy Bypass -Command "Get-Content -Raw '$bootstrap' | iex" *> $null
Remove-Item Env:CP_DESTINO
$env:Path = $pathOriginal; $env:USERPROFILE = $perfilOriginal
Assert ((Get-Content (Join-Path $d 'args.txt')) -eq 'App=False Tailscale= SemNativo=False Agentes= SoChecar=False ConsertarRoda=False') 'iex: roda com tudo no padrao'

# --- Caso: arquivo ASCII e sem BOM ---
$bytes = [IO.File]::ReadAllBytes($bootstrap)
Assert (-not ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)) 'bootstrap.ps1 sem BOM'
Assert (-not ($bytes | Where-Object { $_ -gt 0x7F })) 'bootstrap.ps1 ASCII puro'

Remove-Item -Recurse -Force $raiz -ErrorAction SilentlyContinue
if ($script:falhas) { Write-Host "$($script:falhas) falha(s)"; exit 1 }
Write-Host 'tudo ok'
