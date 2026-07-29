# Prova de fogo do psmux — instala o que falta e roda o teste. Um comando so.
#
#   powershell -ExecutionPolicy Bypass -File scripts\test-psmux.ps1
#
# Instala (via winget, pulando o que ja existe): psmux, Claude Code, Python.
# Depois roda scripts\test-psmux.py e diz onde ficaram os quadros de tela.
#
# O UNICO passo que nao da pra automatizar e o login do Claude: e browser + conta.
# O script para e avisa quando chegar nele.

$ErrorActionPreference = 'Stop'
$raiz = Split-Path -Parent $MyInvocation.MyCommand.Path

function Diz($msg)  { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Ok($msg)   { Write-Host "  ok  $msg" -ForegroundColor Green }
function Erro($msg) { Write-Host "  X   $msg" -ForegroundColor Red }

function Atualiza-Path {
    # winget instala e mexe no PATH do registro, mas o PowerShell JA ABERTO segue com o
    # PATH velho -> o comando recem-instalado "nao existe". Reler dos dois escopos evita
    # mandar o usuario fechar e reabrir o terminal no meio do script.
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
                [Environment]::GetEnvironmentVariable('Path', 'User')
}

function Tem($cmd) { [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

function Instala($rotulo, $cmd, [string[]]$ids) {
    # $ids: candidatos de package id, tentados em ordem. Falhando todos, manda procurar
    # em vez de fingir que instalou.
    if (Tem $cmd) { Ok "$rotulo ja instalado"; return $true }
    foreach ($id in $ids) {
        Diz "instalando $rotulo ($id)"
        winget install --id $id --exact --silent `
            --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null
        Atualiza-Path
        if (Tem $cmd) { Ok "$rotulo instalado"; return $true }
    }
    Erro "$rotulo nao instalou. Procure o id certo com:  winget search $cmd"
    return $false
}

Atualiza-Path   # o terminal ja aberto pode ter PATH velho de uma instalacao anterior

if (-not (Tem 'winget')) {
    Erro 'winget nao encontrado. Instale o "App Installer" pela Microsoft Store e rode de novo.'
    exit 1
}

$falhou = $false
if (-not (Instala 'psmux'       'psmux'  @('marlocarlo.psmux'))) { $falhou = $true }
if (-not (Instala 'Claude Code' 'claude' @('Anthropic.ClaudeCode')))                { $falhou = $true }
if (-not (Instala 'Python'      'py'     @('Python.Python.3.13', 'Python.Python.3.12'))) { $falhou = $true }
if ($falhou) { exit 1 }

# psmux publica `psmux`, `pmux` e `tmux`. O backend chama `tmux` — se esse nome nao existir,
# o tmux.py precisaria de edicao so por causa do nome do binario. Vale saber ANTES do teste.
if (Tem 'tmux') { Ok 'o alias `tmux` existe (o backend chama por esse nome)' }
else            { Erro 'psmux instalou mas nao publicou o alias `tmux` — anote isso no resultado' }

# Login do Claude: nao da pra automatizar. ~\.claude.json so nasce depois do primeiro login.
if (-not (Test-Path "$env:USERPROFILE\.claude.json")) {
    Diz 'Claude Code ainda nao foi logado'
    Write-Host '  Abra outro terminal, rode `claude`, faca o login pelo browser, saia com /exit.'
    Write-Host '  Sem isso o teste captura a TELA DE LOGIN em vez da TUI e quase tudo falha.'
    Read-Host '  Feito isso, aperte Enter para continuar (ou Ctrl+C para sair)'
}

# O teste roda num diretorio LOCAL, mesmo com o script vindo de uma share de rede
# (\\host.lan\Data\... no WinBoat). O .py usa o cwd como `-c` do new-session, ou seja o
# claude nasceria com diretorio de trabalho na rede: lento, slug de transcript torto e
# ConPTY podendo recusar UNC. O script fica onde esta; so o cwd vira local.
$trab = Join-Path $env:LOCALAPPDATA 'cptest'
New-Item -ItemType Directory -Force -Path $trab | Out-Null
Set-Location $trab

Diz 'rodando o teste'
$env:CP_TEST_SLOW = '3'   # VM/WinBoat desenha a TUI devagar; sem isto vira FAIL por impaciencia
py "$raiz\test-psmux.py"
$codigo = $LASTEXITCODE

$quadros = Join-Path $trab 'test-psmux-frames.txt'
if (Test-Path $quadros) {
    Diz "quadros de tela: $quadros"
    # Rodou a partir de uma share de rede (WinBoat monta o $HOME do Linux)? Entao devolve o
    # arquivo pra raiz da share — do lado Linux ele aparece direto no $HOME, sem copiar na mao.
    if ($raiz.StartsWith('\\')) {
        $share = [System.IO.Path]::GetPathRoot($raiz)   # \\host.lan\Data\
        try {
            Copy-Item $quadros $share -Force
            Ok "copiado tambem para $share (aparece no seu \$HOME do Linux)"
        } catch {
            Write-Host "  (nao consegui copiar para $share : $_)"
        }
    }
    Write-Host '  Mande esse arquivo — e o que diz se o state.py enxerga o que o psmux devolve.'
}
exit $codigo
