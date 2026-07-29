# claude-cockpit - instalacao completa no Windows.
# ATENCAO: este arquivo PRECISA ser gravado em UTF-8 COM BOM.
# O Windows PowerShell 5.1 (o que vem no Windows) le script sem BOM como cp1252, e ai um
# travessao U+2014 (E2 80 94) vira 'a-E2-80-94' -> o byte 0x94 e a ASPA CURVA U+201D, que o
# PowerShell aceita como DELIMITADOR DE STRING. A primeira ocorrencia abre uma string que
# nunca fecha, engole o resto do arquivo e o erro sai como 'MissingEndCurlyBrace' numa funcao
# qualquer, dezenas de linhas antes. Medido: install.ps1 nao parseava no PS 5.1.
#
#   powershell -ExecutionPolicy Bypass -File install.ps1            # interativo
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Sim       # aceita tudo
#   powershell -ExecutionPolicy Bypass -File install.ps1 -SoChecar  # so diz o que falta
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Update    # re-aplica o que o git pull nao atualiza
#
# Espelha o install.sh do Linux. Escrito pra Windows PowerShell 5.1 (o que vem no Windows):
# nada de operador ternario nem API de .NET Core, senao quebra em quem nao instalou o PS 7.
# -Update: modo do hook post-merge (.git/hooks/post-merge, instalado pelo install.sh). Re-aplica
# o que o `git pull` NAO atualiza sozinho - deps do backend, build do front, wrapper, tarefa
# agendada - e nao toca em nada que peca decisao ou elevacao: sem instalar dependencia, sem
# token, sem firewall, sem Tailscale. Um hook que trava pedindo confirmacao no meio de um pull
# e pior que hook nenhum.
param([switch]$Sim, [switch]$SoChecar, [switch]$Update)
if ($Update) { $Sim = $true }

$ErrorActionPreference = 'Stop'
$raiz = Split-Path -Parent $MyInvocation.MyCommand.Path
$pendencias = @()

function Titulo($m) { Write-Host "`n$m" -ForegroundColor Cyan }
function Ok($m)     { Write-Host "  ok  $m" -ForegroundColor Green }
function Nota($m)   { Write-Host "      $m" -ForegroundColor DarkGray }
function Falta($m)  { Write-Host "  --  $m" -ForegroundColor Yellow }
function Erro($m)   { Write-Host "  X   $m" -ForegroundColor Red }

function Pergunte($texto) {
    if ($Sim) { return $true }
    $r = Read-Host "$texto [S/n]"
    return ($r -eq '' -or $r -match '^[SsYy]')
}

function Atualiza-Path {
    # winget grava o PATH no registro, mas o PowerShell JA ABERTO segue com o antigo -> o
    # programa recem-instalado "nao existe". Reler os dois escopos evita mandar fechar o terminal
    # no meio da instalacao, que e onde roteiro de Windows costuma perder o usuario.
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
                [Environment]::GetEnvironmentVariable('Path', 'User')
}

function Tem($cmd) { return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

function EhAdmin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    return ([Security.Principal.WindowsPrincipal]$id).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Instale($rotulo, $cmd, $id, $porque) {
    if (Tem $cmd) { Ok $rotulo; return $true }
    if ($SoChecar) { Falta "$rotulo - $porque"; $script:pendencias += $rotulo; return $false }
    if ($Update) { Erro "$rotulo faltando (-Update nao instala dependencia)"; $script:pendencias += $rotulo; return $false }
    Write-Host "  .. instalando $rotulo ($id)"
    winget install --id $id --exact --silent `
        --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null
    Atualiza-Path
    if (Tem $cmd) { Ok "$rotulo instalado"; return $true }
    Erro "$rotulo nao instalou - procure o id com: winget search $cmd"
    $script:pendencias += $rotulo
    return $false
}

Atualiza-Path
if (-not (Tem 'winget')) {
    Erro 'winget nao encontrado. Instale o "App Installer" pela Microsoft Store e rode de novo.'
    exit 1
}

# -- 1/8 Dependencias obrigatorias -------------------------------------------
Titulo '1/8 Dependencias'
Instale 'psmux (multiplexador)' 'psmux'  'marlocarlo.psmux'     'sem ele nao existe sessao' | Out-Null
Instale 'Claude Code'           'claude' 'Anthropic.ClaudeCode' 'e o que o app pilota'      | Out-Null
Instale 'Python'                'py'     'Python.Python.3.13'   'o backend e Python'        | Out-Null
Instale 'Node 20+'              'node'   'OpenJS.NodeJS.LTS'    'o frontend e Svelte'       | Out-Null
Instale 'uv'                    'uv'     'astral-sh.uv'         'gerencia o venv do backend' | Out-Null

# O backend chama o multiplexador por `tmux`. O psmux publica esse alias; se um dia parar,
# isso vira erro claro AQUI em vez de "falha ao criar sessao" dentro do app.
if ((Tem 'psmux') -and -not (Tem 'tmux')) {
    Erro 'psmux instalado mas sem o alias `tmux` - o backend chama por esse nome'
    $pendencias += 'alias tmux'
}

# Git e OPCIONAL: sem ele o app roda, so perde o chip de branch e a aba de git.
if (-not (Tem 'git')) {
    Falta 'git ausente - o painel de git e o chip de branch ficam vazios (o resto funciona)'
    if (-not $SoChecar -and (Pergunte '      Instalar o Git agora?')) {
        Instale 'Git' 'git' 'Git.Git' 'painel de git' | Out-Null
    }
} else { Ok 'git' }

if ($SoChecar) {
    if ($pendencias.Count -eq 0) { Titulo 'Nada faltando.'; exit 0 }
    Titulo "Faltam: $($pendencias -join ', ')"
    exit 1
}
if ($pendencias.Count -gt 0) { Erro "faltam: $($pendencias -join ', ')"; exit 1 }

# -- 2/8 Backend -------------------------------------------------------------
Titulo '2/8 Backend'
Push-Location "$raiz\backend"
uv sync --quiet
Ok 'dependencias instaladas'
Nota 'psutil entra aqui: no Windows nao ha /proc pra ler informacao de processo'
Pop-Location

# -- 3/8 Token de acesso -----------------------------------------------------
# Perguntado, nao gerado: voce DIGITA isto no celular, e 48 caracteres hex e castigo. Enter em
# branco ainda gera um aleatorio. O piso de 8 e daqui - o backend so recusa o literal
# 'change-me', entao uma senha de 4 digitos passaria batido sem esta checagem.
Titulo '3/8 Token de acesso'
$envFile = "$raiz\backend\.env"
$temToken = (Test-Path $envFile) -and (Select-String -Path $envFile -Pattern '^CP_AUTH_TOKEN=' -Quiet)
function Token-Aleatorio {
    # RNGCryptoServiceProvider, nao RandomNumberGenerator::Fill: o segundo e .NET Core e nao
    # existe no PowerShell 5.1 que vem no Windows.
    $bytes = New-Object byte[] 24
    (New-Object System.Security.Cryptography.RNGCryptoServiceProvider).GetBytes($bytes)
    return (-join ($bytes | ForEach-Object { $_.ToString('x2') }))
}

if ($temToken) {
    Ok 'backend\.env ja tem CP_AUTH_TOKEN (mantido)'
} elseif ($Sim) {
    Add-Content -Path $envFile -Value "CP_AUTH_TOKEN=$(Token-Aleatorio)"
    Ok 'CP_AUTH_TOKEN aleatorio gerado (modo -Sim nao pergunta)'
} else {
    Write-Host '  Voce vai DIGITAR este token no celular, entao escolha algo que lembre.'
    Write-Host '  Enter em branco = gera um aleatorio de 48 caracteres (seguro, chato de digitar).'
    Nota 'No Tailscale a rede ja e fechada e o token e a segunda tranca. No Wi-Fi de casa ele'
    Nota 'e a UNICA: quem estiver na rede e acertar a senha roda comando como voce. Nada de "1234".'
    while ($true) {
        $token = Read-Host '  Token'
        if (-not $token) { $token = Token-Aleatorio; Ok 'aleatorio gerado'; break }
        # O backend recusa subir fora do loopback com 'change-me'; o piso de 8 e daqui, pra
        # senha curta nao passar batido so porque o backend so barra aquele valor literal.
        if ($token.Length -lt 8) { Erro 'curto demais - no minimo 8 caracteres'; continue }
        if ($token -eq 'change-me') { Erro 'esse valor o backend recusa de proposito'; continue }
        break
    }
    Add-Content -Path $envFile -Value "CP_AUTH_TOKEN=$token"
    Ok 'CP_AUTH_TOKEN gravado em backend\.env'
}
Nota 'E esse token que voce digita no celular na primeira conexao.'

# -- 4/8 Frontend ------------------------------------------------------------
Titulo '4/8 Frontend'
Push-Location "$raiz\frontend"
npm ci --silent
npm run build --silent
Pop-Location
Ok 'buildado em frontend\dist\'

# -- 5/8 Wrapper do claude ---------------------------------------------------
# Sem ele um `claude` que VOCE abre no terminal e invisivel pro app: nao tem --session-id (o
# backend nao sabe qual transcript e daquela sessao) e nao vive num pane (nao ha estado nem
# input). Sessao criada PELO app funciona de qualquer jeito; isto e sobre a outra direcao.
Titulo '5/8 Wrapper do claude (sessao aberta por voce aparece no app)'
$marca = '# >>> claude-cockpit >>>'
$perfil = $PROFILE.CurrentUserAllHosts
$jaTem = (Test-Path $perfil) -and (Select-String -Path $perfil -Pattern ([regex]::Escape($marca)) -Quiet)
if ($jaTem) {
    Ok 'bloco ja presente no seu $PROFILE'
} elseif (Pergunte '  Instalar (recomendado)?') {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $perfil) | Out-Null
    Add-Content -Path $perfil -Value @"

$marca
. "$raiz\scripts\shell\claude.ps1"
# <<< claude-cockpit <<<
"@
    Ok "bloco adicionado em $perfil"
    Nota 'Vale nos terminais NOVOS - este aqui ainda esta com o perfil antigo.'
} else {
    Nota 'pulado - sessao aberta no terminal nao vai aparecer no app'
}

# -- 6/8 Acesso pelo celular -------------------------------------------------
Titulo '6/8 Acesso pelo celular'
if ($Update) {
    Ok 'pulado no -Update (firewall e Tailscale pedem elevacao; nada aqui muda com git pull)'
} else {
Write-Host '  Duas formas, e elas nao competem:'
Write-Host '    LAN      - celular no mesmo Wi-Fi. Precisa liberar as portas no firewall.'
Write-Host '    Tailscale - VPN pessoal. Funciona de QUALQUER lugar sem expor nada pra internet.'
Nota 'Fora de casa, use Tailscale. NUNCA abra porta pra internet publica: o app roda o'
Nota 'claude como VOCE, entao um host exposto e execucao remota na sua maquina.'

# Firewall: precisa de admin. Sem admin nao adianta tentar - a regra falha e o usuario fica
# achando que liberou. Ja liberado -> nem pergunta: re-rodar o instalador depois de um git pull
# deve pegar so o que falta, nao repetir pergunta do que ja esta de pe.
$regras = @(8765, 5173) | ForEach-Object {
    Get-NetFirewallRule -DisplayName "claude-cockpit $_" -ErrorAction SilentlyContinue
}
if ($regras.Count -eq 2) {
    Ok 'portas 8765 e 5173 ja liberadas no firewall'
} elseif (Pergunte '  Liberar as portas 8765 e 5173 no firewall pra rede LOCAL?') {
    if (EhAdmin) {
        foreach ($p in 8765, 5173) {
            $nome = "claude-cockpit $p"
            Get-NetFirewallRule -DisplayName $nome -ErrorAction SilentlyContinue |
                Remove-NetFirewallRule -ErrorAction SilentlyContinue
            # Profile Private: rede de casa. Em rede Publica (cafe, aeroporto) segue fechado,
            # que e o comportamento que se quer sem precisar lembrar de desligar nada.
            New-NetFirewallRule -DisplayName $nome -Direction Inbound -Action Allow `
                -Protocol TCP -LocalPort $p -Profile Private | Out-Null
        }
        Ok 'portas liberadas (perfil Private apenas)'
    } else {
        Falta 'sem privilegio de administrador - abra um PowerShell como admin e rode:'
        Nota 'New-NetFirewallRule -DisplayName "claude-cockpit 8765" -Direction Inbound -Action Allow -Protocol TCP -LocalPort 8765 -Profile Private'
        Nota 'New-NetFirewallRule -DisplayName "claude-cockpit 5173" -Direction Inbound -Action Allow -Protocol TCP -LocalPort 5173 -Profile Private'
    }
}

if (Tem 'tailscale') {
    Ok 'Tailscale ja instalado'
    Nota 'Depois do `tailscale up`, ponha o nome .ts.net em CP_PUBLIC_URL no backend\.env'
    Nota 'pra o QR sair com o endereco certo em vez do IP da LAN.'
} elseif (Pergunte '  Instalar o Tailscale? (VPN pessoal - acesso de fora de casa)') {
    # Id com MAIUSCULAS: o `--exact` do winget diferencia caixa, e 'tailscale.tailscale' nao casa
    # nada. Medido: os outros seis ids do instalador estavam certos, so este errado.
    if (Instale 'Tailscale' 'tailscale' 'Tailscale.Tailscale' 'acesso remoto') {
        # Estas instrucoes so fazem sentido se a instalacao DEU CERTO. Antes elas saiam mesmo apos
        # a falha, mandando o usuario rodar `tailscale up` de um programa que nao existia.
        Nota 'Falta logar: rode `tailscale up` e instale o Tailscale tambem no celular.'
        Nota 'Depois ponha o nome .ts.net em CP_PUBLIC_URL no backend\.env.'
    }
}

}

# -- 7/8 Subir sozinho no logon ----------------------------------------------
# Equivalente possivel dos servicos systemd do Linux. Nao e servico do Windows (isso exigiria
# admin e rodaria fora da sua sessao, sem acesso ao seu ~\.claude): e tarefa agendada no logon.
Titulo '7/8 Subir junto com o Windows'
$tarefas = @(
    @{ Nome = 'claude-cockpit-backend';  Exe = 'uv';  Args = 'run python -m app.main'; Dir = "$raiz\backend" },
    @{ Nome = 'claude-cockpit-frontend'; Exe = 'npm'; Args = 'run dev';                Dir = "$raiz\frontend" }
)
# Ja registrado -> RE-REGISTRA sem perguntar, em vez de pular. A tarefa guarda o caminho do
# executavel e o diretorio DENTRO dela; um `git pull` que mova o repo, ou um uv que mude de
# lugar, deixa a tarefa apontando pro nada - e "ja registrada" esconderia isso. Register-...
# -Force sobrescreve.
$jaAgendado = Get-ScheduledTask -TaskName $tarefas[0].Nome -ErrorAction SilentlyContinue
if ($jaAgendado -or (Pergunte '  Registrar backend e frontend pra subir no seu logon?')) {
    try {
        foreach ($t in $tarefas) {
            # -Exe pelo caminho completo: a tarefa nasce com o PATH do sistema, nao com o do
            # seu shell - `uv` instalado em ~\.local\bin nao seria encontrado.
            $exe = (Get-Command $t.Exe).Source
            $acao = New-ScheduledTaskAction -Execute $exe -Argument $t.Args -WorkingDirectory $t.Dir
            $gatilho = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
            $cfg = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries `
                        -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)
            Register-ScheduledTask -TaskName $t.Nome -Action $acao -Trigger $gatilho `
                -Settings $cfg -Force | Out-Null
            Ok "tarefa $($t.Nome) registrada"
        }
        Nota 'Remover depois: Unregister-ScheduledTask -TaskName claude-cockpit-backend'
    } catch {
        Falta "nao deu pra registrar as tarefas: $_"
        Nota 'Sem isso, o backend so roda enquanto o terminal estiver aberto.'
    }
} else {
    Nota 'pulado - rodando na mao, fechar o terminal derruba o backend'
}

# -- 8/8 Checagem de fumaca --------------------------------------------------
# Ate aqui foi tudo instalacao. Este passo separa "instalou" de "funciona": ate pouco tempo o
# backend nem IMPORTAVA no Windows (um `import fcntl` no topo do projects.py) e um instalador
# sem esta checagem teria reportado sucesso do mesmo jeito.
Titulo '8/8 Checagem de fumaca'
Push-Location "$raiz\backend"
uv run python -c "from app import api, registry, procinfo, projects" 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { Erro 'o backend nao importa neste Windows'; Pop-Location; exit 1 }
Ok 'o backend importa'

uv run python -c "from app import procinfo; assert not procinfo._TEM_PROC; import psutil; assert psutil.Process().cwd()" 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { Erro 'leitura de processo via psutil falhou'; Pop-Location; exit 1 }
Ok 'leitura de processo (psutil) funcionando'
Pop-Location

# Sobra de rodada anterior: o kill pode demorar, entao limpa antes de contar de novo.
tmux list-sessions -F '#{session_name}' 2>$null | Where-Object { $_ -like 'cp-fumaca-*' } |
    ForEach-Object { tmux kill-session -t "=$_" 2>&1 | Out-Null }

$sessao = "cp-fumaca-$PID"
tmux new-session -d -s $sessao -c $env:TEMP 'cmd' 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    Erro 'o psmux nao criou uma sessao de teste - o app nao vai abrir sessao'
    exit 1
}
Ok 'o multiplexador cria sessao'

# Matar e testado SEPARADO de criar, porque medido no psmux 3.3.7 eles falham separado: o
# kill-session responde "session still present after 5s" com um `cmd` interativo no pane, enquanto
# a criacao funciona. Isso importa alem do instalador - o botao de apagar sessao do app chama
# exatamente este comando, e sessao que nao morre vira zumbi na lista.
tmux kill-session -t "=$sessao" 2>&1 | Out-Null
$morreu = $false
foreach ($i in 1..15) {
    tmux has-session -t "=$sessao" 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { $morreu = $true; break }
    Start-Sleep -Seconds 1
}
if ($morreu) {
    Ok 'o multiplexador mata sessao'
} else {
    # NAO aborta: tudo o que o app precisa pra FUNCIONAR ja passou. Mas tem que aparecer, porque
    # o sintoma no uso e sessao apagada que reaparece na lista.
    Falta "a sessao de teste '$sessao' nao morreu em 15s - apagar sessao pelo app pode deixar zumbi"
    Nota "limpar na mao:  tmux kill-session -t '=$sessao'"
}

# -- Fim ---------------------------------------------------------------------
Titulo 'Pronto'
Write-Host @"
  Rodar na mao (se voce pulou o passo 7):
      cd backend  ; `$env:CP_LAN_BIND_IP='auto' ; uv run python -m app.main
      cd frontend ; npm run dev

  No celular: abra a URL do QR que o backend imprime e cole o token de backend\.env.
  Guia completo (Tailscale, instalar como PWA, cada tela): docs\USAGE.md

  O que este Windows NAO tem, e nao e esquecimento:
  - cp-send (recado e pareamento entre sessoes) e as skills do repo: sao shell script,
    rodam so no Linux/macOS.
  - wrappers do `codex` e do `pi`, e a extensao cp-state.ts do Pi: idem. Sessao Codex ou Pi
    aberta por voce no terminal nao aparece; criada pelo app, funciona.
  - resurrect/continuum (sessoes sobreviverem a reboot): sao plugins de tmux em bash, e o
    psmux nao roda plugin de tmux. Fechou o Windows, as sessoes se foram.
"@
