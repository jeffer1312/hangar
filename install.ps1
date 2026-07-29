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

function Nativo {
    <#
      Roda um programa NATIVO e devolve o exit code, engolindo a saida.

      Existe por causa de uma armadilha do PowerShell: com $ErrorActionPreference = 'Stop' (linha
      do topo), QUALQUER linha que o programa escreva no stderr vira erro TERMINANTE quando se usa
      `2>&1`. Nao e preciso o programa falhar - basta ele avisar. Medido: o psmux escreve
      "session still present after 5s" no stderr, e o instalador inteiro morria com
      NativeCommandError, apontando pra linha do ForEach-Object em vez de pro aviso.
      Vale igual pra winget, npm e uv: qualquer aviso deles derrubaria a instalacao no meio.

      Dentro daqui o preference volta pra 'Continue' so enquanto o programa roda, e o que decide
      sucesso e o $LASTEXITCODE - que e o contrato certo pra programa nativo.
    #>
    $anterior = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $args[0] @($args[1..($args.Count - 1)]) 2>&1 | Out-Null
        return $LASTEXITCODE
    } finally { $ErrorActionPreference = $anterior }
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
    Nativo winget install --id $id --exact --silent `
        --accept-package-agreements --accept-source-agreements | Out-Null
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
# So rebuilda quando ha motivo. NAO por data de modificacao: no Windows o git reescreve
# arquivos no checkout (conversao de fim de linha), entao um `git pull` que nem tocou no front
# deixa fontes com carimbo mais novo que o dist e o build roda a toa - medido, rebuildava toda vez.
# A marca e o estado do GIT: commit atual + o que estiver sujo em frontend/. Isso muda quando o
# conteudo muda, e so quando ele muda.
$dist = "$raiz\frontend\dist\index.html"
$modulos = "$raiz\frontend\node_modules"
$marcaArq = "$raiz\frontend\dist\.cp-build-stamp"

$marca = $null
if (Tem 'git') {
    $commit = (& git -C $raiz rev-parse HEAD 2>$null)
    $sujo = (& git -C $raiz status --porcelain -- frontend 2>$null) -join "`n"
    if ($commit) { $marca = "$commit`n$sujo" }
}

$precisa = $true
if ((Test-Path $dist) -and (Test-Path $modulos)) {
    if ($marca -and (Test-Path $marcaArq)) {
        # -Raw: sem isto o Get-Content devolve array de linhas e a comparacao com a string falha
        # sempre - o build rodaria toda vez de novo, so que por outro motivo.
        $precisa = ((Get-Content $marcaArq -Raw) -ne $marca)
    } elseif (-not $marca) {
        # Sem git nao da pra saber o que mudou; rebuildar e a escolha segura.
        $precisa = $true
    }
}

if ($precisa) {
    Push-Location "$raiz\frontend"
    npm ci --silent
    npm run build --silent
    Pop-Location
    # A marca so e gravada DEPOIS do build dar certo: build que falhou nao pode marcar
    # "atualizado" e fazer a proxima rodada pular um dist quebrado.
    if ($marca) { Set-Content -Path $marcaArq -Value $marca -NoNewline -Encoding UTF8 }
    Ok 'buildado em frontend\dist\'
} else {
    Ok 'frontend ja buildado e atualizado (nada mudou no git desde o ultimo build)'
}

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
    # O Windows vem com ExecutionPolicy = Restricted, que recusa carregar QUALQUER perfil. Escrever
    # o bloco assim mesmo nao so deixaria o wrapper sem carregar: todo terminal novo passaria a
    # cuspir um PSSecurityException por causa de um arquivo que nos criamos. Medido nesta maquina.
    # RemoteSigned no escopo CurrentUser nao precisa de admin e e o que qualquer ferramenta de
    # PowerShell pede: script local roda, script baixado da internet so assinado.
    # `return` aqui encerraria o SCRIPT (nao estamos numa funcao) e pularia os passos 6, 7 e 8.
    $podeEscrever = $true
    if ((Get-ExecutionPolicy) -eq 'Restricted') {
        Nota 'O Windows esta com ExecutionPolicy=Restricted: nenhum perfil carrega.'
        Nota 'Sem mudar isso, o wrapper nao funciona E todo terminal novo mostra erro.'
        if (Pergunte '  Liberar script local pro seu usuario (RemoteSigned, sem admin)?') {
            Set-ExecutionPolicy -Scope CurrentUser RemoteSigned -Force
            Ok 'ExecutionPolicy do usuario = RemoteSigned'
        } else {
            $podeEscrever = $false
            Falta 'wrapper NAO instalado - assim ele so criaria erro em todo terminal novo'
            Nota 'pra fazer depois:  Set-ExecutionPolicy -Scope CurrentUser RemoteSigned'
        }
    }
    if ($podeEscrever) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $perfil) | Out-Null
    Add-Content -Path $perfil -Value @"

$marca
. "$raiz\scripts\shell\claude.ps1"
# <<< claude-cockpit <<<
"@
    Ok "bloco adicionado em $perfil"
    Nota 'Vale nos terminais NOVOS - este aqui ainda esta com o perfil antigo.'
    }
} else {
    Nota 'pulado - sessao aberta no terminal nao vai aparecer no app'
}

# -- 5b/8 Config do multiplexador -------------------------------------------
# O psmux foi instalado POR NOS. O usuario pediu Claude Code, nao um multiplexador - entao a
# barra de status dele aparecendo no rodape e ruido que nos criamos, e a tela deixa de parecer o
# Claude Code que a pessoa conhece. Escondemos.
#
# So a barra: as linhas de truecolor do ~/.tmux.conf do Linux NAO entram aqui, porque o ConPTY
# ja e 24-bit nativo - la elas existem pra desfazer o downgrade pra 256 que o tmux faz no Unix.
#
# Bloco marcado, mesma convencao do install-claude-wrapper.sh: reescreve o que e nosso e preserva
# o resto do arquivo, entao rodar de novo nao duplica e nao apaga config de ninguem.
Titulo '5b/8 Config do multiplexador'
$confTmux = Join-Path $HOME '.tmux.conf'
$ini = '# >>> claude-pocket >>>'
$fim = '# <<< claude-pocket <<<'
$corpo = @(
    $ini,
    '# Esconde a barra de status: o multiplexador e detalhe de implementacao do claude-cockpit,',
    '# nao algo que voce pediu. Comente esta linha se quiser a barra de volta.',
    'set -g status off',
    $fim
)
$atual = @()
if (Test-Path $confTmux) { $atual = @(Get-Content $confTmux) }
$temIni = $atual -contains $ini
if ($temIni) {
    $antes = $atual[0..([array]::IndexOf($atual, $ini) - 1)]
    $depoisIdx = [array]::IndexOf($atual, $fim) + 1
    $depois = if ($depoisIdx -lt $atual.Count) { $atual[$depoisIdx..($atual.Count - 1)] } else { @() }
    $novo = @($antes) + $corpo + @($depois)
} else {
    $novo = @($atual) + $corpo
}
if (($atual -join "`n") -ne ($novo -join "`n")) {
    Set-Content -Path $confTmux -Value $novo -Encoding UTF8
    Ok "barra de status desligada em $confTmux"
    Nota 'Vale nas sessoes NOVAS - a config e lida na criacao da sessao.'
} else {
    Ok 'config do multiplexador ja aplicada'
}

# -- 5c/8 Statusline ---------------------------------------------------------
# O app le modelo, contexto, custo e limite de taxa da statusline do Claude Code - o parser
# espera ESTE formato (scripts/omniroute-statusline.js). Sem isso o painel do app mostra
# "medicao indisponivel" no lugar do contexto, e foi assim que a falta apareceu no teste.
# O instalador do Linux ja fazia; o do Windows tinha ficado sem.
Titulo '5c/8 Statusline do Claude Code'
$slJs = "$raiz\scripts\omniroute-statusline.js"
$settingsClaude = Join-Path $HOME '.claude\settings.json'
if (-not (Test-Path $slJs)) {
    Falta 'omniroute-statusline.js nao encontrado - pulando'
} elseif (-not (Tem 'node')) {
    Falta 'node nao encontrado - a statusline precisa dele'
} else {
    $nodeExe = (Get-Command node).Source
    $cmdSl = "`"$nodeExe`" `"$slJs`""
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $settingsClaude) | Out-Null
    # NADA de `ConvertFrom-Json -AsHashtable`: esse parametro so existe no PowerShell 6+, e no
    # 5.1 que vem no Windows a chamada levanta - o catch abaixo transformava isso em "settings
    # ilegivel" e o passo se pulava sozinho, num arquivo que estava perfeitamente legivel.
    # PSCustomObject entao, com Add-Member -Force pra sobrescrever a chave.
    $cfg = $null
    if (Test-Path $settingsClaude) {
        try {
            $bruto = Get-Content $settingsClaude -Raw
            if ($bruto.Trim()) { $cfg = $bruto | ConvertFrom-Json }
        } catch {
            Falta 'settings.json do Claude ilegivel - nao vou reescrever por cima'
            $cfg = 'ERRO'
        }
    }
    if ($null -eq $cfg) { $cfg = New-Object psobject }
    if ($cfg -ne 'ERRO') {
        $atual = $null
        if ($cfg.PSObject.Properties.Name -contains 'statusLine') { $atual = $cfg.statusLine.command }
        if ($atual -eq $cmdSl) {
            Ok 'statusline ja configurada'
        } else {
            if ($atual) { Copy-Item $settingsClaude "$settingsClaude.bak" -Force }
            $valor = New-Object psobject -Property @{ type = 'command'; command = $cmdSl }
            $cfg | Add-Member -NotePropertyName 'statusLine' -NotePropertyValue $valor -Force
            $cfg | ConvertTo-Json -Depth 20 | Set-Content -Path $settingsClaude -Encoding UTF8
            Ok 'statusline configurada no ~/.claude/settings.json'
            Nota 'Vale nas sessoes NOVAS do Claude Code.'
            # Mesmo aviso do Linux: o caminho do node fica CRAVADO no settings. Trocar de versao
            # de node quebra a statusline em silencio - o app volta a dizer "medicao indisponivel".
            Nota 'Se voce trocar a versao do node, rode este instalador de novo.'
        }
    }
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
            # Rodar o programa DIRETO abre uma janela de console e ela fica na tela pra sempre -
            # todo processo de console no Windows abre uma. Passando por um powershell oculto, a
            # janela some e os filhos herdam o console escondido.
            # A saida NAO pode simplesmente sumir junto: e nela que sai o QR de pareamento e
            # qualquer erro de subida. Vai pra arquivo, um por servico, sobrescrito a cada start
            # (nao cresce sem limite; o que interessa e sempre a execucao atual).
            $log = Join-Path $env:LOCALAPPDATA "claude-cockpit\$($t.Nome).log"
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $log) | Out-Null
            # -EncodedCommand (base64 UTF-16LE) em vez de -Command com aspas: o PowerShell NAO
            # escapa com barra invertida, e a string aninhada quebrava o New-ScheduledTaskAction
            # ("nao e possivel localizar um parametro posicional"). Codificado nao ha o que escapar.
            $interno = "& '$exe' $($t.Args) *>&1 | Out-File -FilePath '$log' -Encoding utf8"
            $b64 = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($interno))
            $acao = New-ScheduledTaskAction -Execute 'powershell.exe' `
                -Argument "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -EncodedCommand $b64" `
                -WorkingDirectory $t.Dir
            $gatilho = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
            $cfg = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries `
                        -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)
            Register-ScheduledTask -TaskName $t.Nome -Action $acao -Trigger $gatilho `
                -Settings $cfg -Force | Out-Null
            # Registrar NAO inicia: o gatilho e "no logon", entao sem isto nada sobe ate o
            # proximo login e a pessoa abre o navegador numa porta morta logo apos instalar.
            # O equivalente no Linux (`systemctl --user enable --now`) liga na hora - o `--now`
            # e justamente esta metade, e ela tinha ficado de fora aqui.
            Start-ScheduledTask -TaskName $t.Nome -ErrorAction SilentlyContinue
            Ok "tarefa $($t.Nome) registrada e iniciada"
        }
        Nota 'Log (inclui o QR de pareamento):'
    Nota "  $env:LOCALAPPDATA\claude-cockpit\claude-cockpit-backend.log"
    Nota 'Remover depois: Unregister-ScheduledTask -TaskName claude-cockpit-backend'
    } catch {
        Falta "nao deu pra registrar as tarefas: $_"
        Nota 'Sem isso, o backend so roda enquanto o terminal estiver aberto.'
    }
} else {
    Nota 'pulado - rodando na mao, fechar o terminal derruba o backend'
}

# -- 7b/8 cp-send + skills ---------------------------------------------------
# O cp-send e bash falando com o backend por HTTP - nada nele exige unix. Faltavam tres coisas
# no Windows, e sao estas que este passo resolve:
#   1. um `python3` que exista (o instalador do Python cria python.exe e py.exe, e o cp-send
#      chama python3 dez vezes pra ler JSON);
#   2. um lancador que o PowerShell enxergue, ja que o script nao tem extensao;
#   3. rodar o proprio install-cp-send.sh - e ele quem cria o link, as skills e o bloco de
#      protocolo no ~/.claude/CLAUDE.md. Duplicar esse texto aqui daria duas fontes da verdade,
#      e a que diverge silenciosamente e sempre a copia.
Titulo '7b/8 cp-send (recado e pareamento entre sessoes)'
$bash = $null
if (Tem 'git') {
    # O git fica em ...\cmd\git.exe; o bash mora em ...\bin\bash.exe do mesmo Git for Windows.
    $gitDir = Split-Path -Parent (Split-Path -Parent (Get-Command git).Source)
    $cand = Join-Path $gitDir 'bin\bash.exe'
    if (Test-Path $cand) { $bash = $cand }
}
if (-not $bash) {
    Falta 'bash do Git for Windows nao encontrado - cp-send fica de fora'
    Nota 'instale o Git e rode este instalador de novo'
} else {
    $binUsuario = Join-Path $HOME '.local\bin'
    New-Item -ItemType Directory -Force -Path $binUsuario | Out-Null
    # Forma /c/... do mesmo diretorio, pra usar dentro do bash.
    $binMsys = ($binUsuario -replace '\\', '/') -replace '^([A-Za-z]):', '/$1'

    # (1) shim de python3 SEM extensao: quem vai executa-lo e o bash, e ele le o shebang.
    # Um python3.cmd nao serviria - o bash nao roda .cmd por conta propria.
    # O caminho do Python vai CRAVADO no atalho, resolvido aqui pelo PowerShell - nos mesmos
    # instalamos ele no passo 1, entao o local e conhecido e nao ha por que procurar duas vezes.
    # Procurar de novo la dentro nao funciona: `bash -lc` e login shell e reconstroi o PATH, entao
    # o que o PowerShell acha o shell do atalho nao acha. Medido: caiu no `python` e pegou o stub
    # da Microsoft Store ("Python nao foi encontrado"), que e o que ha no PATH de um Windows tipico.
    $pyExe = $null
    foreach ($cand in 'py', 'python') {
        $c = Get-Command $cand -CommandType Application -ErrorAction SilentlyContinue |
             Select-Object -First 1
        # WindowsApps = atalho da Store, que nao executa nada. Pular explicitamente.
        if ($c -and $c.Source -notlike '*WindowsApps*') { $pyExe = $c.Source; break }
    }
    if (-not $pyExe) {
        Falta 'nenhum Python real encontrado (so o atalho da Store) - cp-send ficaria sem JSON'
        Nota 'instale com:  winget install --id Python.Python.3.13'
    } else {
        # C:\Windows\py.exe -> /c/Windows/py.exe, que e a forma que o bash do MSYS executa.
        $pyMsys = ($pyExe -replace '\\', '/') -replace '^([A-Za-z]):', '/$1'
        $arg = if ((Split-Path -Leaf $pyExe) -ieq 'py.exe') { ' -3' } else { '' }
        $shim = Join-Path $binUsuario 'python3'
        $corpoShim = "#!/bin/sh`n" +
                     "# Gerado por claude-cockpit/install.ps1 - o cp-send chama python3.`n" +
                     "exec '$pyMsys'$arg `"`$@`"`n"
        if (-not (Test-Path $shim) -or (Get-Content $shim -Raw) -ne $corpoShim) {
            Set-Content -Path $shim -Encoding ASCII -NoNewline -Value $corpoShim
            Ok "atalho python3 -> $pyExe$arg"
        } else { Ok 'atalho python3 ja atualizado' }
    }

    # (2) lancador pro PowerShell: o script nao tem extensao, entao o Windows nao o executa
    # sozinho. O .cmd entrega tudo pro bash e repassa os argumentos.
    $lancador = Join-Path $binUsuario 'cp-send.cmd'
    # PATH com o nosso bin NA FRENTE: o Windows tem um python3.exe proprio no atalho da
    # Microsoft Store (%LOCALAPPDATA%\Microsoft\WindowsApps), que vem antes no PATH e responde
    # "Python nao foi encontrado". Sem a precedencia, o atalho que acabamos de escrever nunca e
    # alcancado - medido, o install-cp-send.sh falhava mesmo com o atalho correto no lugar.
    $conteudo = "@echo off`r`n" +
                "set `"PATH=%USERPROFILE%\.local\bin;%PATH%`"`r`n" +
                "`"$bash`" `"$raiz\scripts\cp-send`" %*`r`n"
    if (-not (Test-Path $lancador) -or (Get-Content $lancador -Raw) -ne $conteudo) {
        Set-Content -Path $lancador -Value $conteudo -Encoding ASCII -NoNewline
        Ok "lancador cp-send.cmd criado em $binUsuario"
    } else { Ok 'lancador cp-send.cmd ja atualizado' }

    # (3) PATH do usuario, pra `cp-send` funcionar de qualquer terminal (e pro bash achar o shim).
    $pathUsuario = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($pathUsuario -notlike "*$binUsuario*") {
        [Environment]::SetEnvironmentVariable('Path', "$pathUsuario;$binUsuario", 'User')
        Atualiza-Path
        Ok "$binUsuario adicionado ao PATH do usuario"
        Nota 'Vale nos terminais NOVOS.'
    } else { Ok 'PATH do usuario ja tem o diretorio' }

    # (4) o instalador de verdade, rodado pelo bash: link, skills e o bloco do CLAUDE.md.
    # A saida NAO passa pelo Nativo aqui: ele engole tudo, e num passo que pode falhar por dez
    # motivos diferentes (link, permissao, caminho) a saida E o diagnostico. Medido: falhou uma
    # vez e a mensagem util tinha sido descartada, sobrando so "falhou".
    $rota = ($raiz -replace '\\', '/') -replace '^([A-Za-z]):', '/$1'
    # ErrorActionPreference volta pra Continue AQUI: com 'Stop', qualquer linha de stderr do
    # script vira excecao terminante (mesma armadilha que a funcao Nativo existe pra evitar) -
    # e este passo QUER a stderr, e ela e o diagnostico.
    $anterior = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $saida = & $bash '-lc' "export PATH='$binMsys':`$PATH; cd '$rota' && ./scripts/install-cp-send.sh" 2>&1
    } finally { $ErrorActionPreference = $anterior }
    if ($LASTEXITCODE -eq 0) {
        # O `ln -s` do Git Bash COPIA em vez de linkar, e o cp-send se localiza pelo proprio
        # caminho: `dirname $(realpath $0)/../backend/.env`. Com a copia em ~/.local/bin, ele
        # procura o .env em ~/.local/backend/ - que nao existe, e o --list falha dizendo que nao
        # acha o backend. Substituimos a copia por um lancador que chama o script NO REPO com
        # caminho absoluto: dentro dele, $0 volta a ser o do repo e a busca acerta.
        $cpSendSh = Join-Path $binUsuario 'cp-send'
        $corpoCp = "#!/bin/sh`n" +
                   "# Gerado por claude-cockpit/install.ps1 - ver comentario no instalador.`n" +
                   "exec '$rota/scripts/cp-send' `"`$@`"`n"
        if (-not (Test-Path $cpSendSh) -or (Get-Content $cpSendSh -Raw) -ne $corpoCp) {
            Set-Content -Path $cpSendSh -Encoding ASCII -NoNewline -Value $corpoCp
            Ok 'cp-send do ~/.local/bin aponta pro script do repo'
        }
        Ok 'cp-send + skills instalados'
        Nota 'teste (em terminal NOVO):  cp-send --list'
    } else {
        Falta 'install-cp-send.sh falhou:'
        $saida | Select-Object -Last 12 | ForEach-Object { Nota "  $_" }
        Nota "rodar na mao:  & '$bash' -lc 'cd $rota && ./scripts/install-cp-send.sh'"
    }
}

# -- 8/8 Checagem de fumaca --------------------------------------------------
# Ate aqui foi tudo instalacao. Este passo separa "instalou" de "funciona": ate pouco tempo o
# backend nem IMPORTAVA no Windows (um `import fcntl` no topo do projects.py) e um instalador
# sem esta checagem teria reportado sucesso do mesmo jeito.
Titulo '8/8 Checagem de fumaca'
Push-Location "$raiz\backend"
$rc = Nativo uv run python -c "from app import api, registry, procinfo, projects"
if ($rc -ne 0) { Erro 'o backend nao importa neste Windows'; Pop-Location; exit 1 }
Ok 'o backend importa'

$rc = Nativo uv run python -c "from app import procinfo; assert not procinfo._TEM_PROC; import psutil; assert psutil.Process().cwd()"
if ($rc -ne 0) { Erro 'leitura de processo via psutil falhou'; Pop-Location; exit 1 }
Ok 'leitura de processo (psutil) funcionando'
Pop-Location

# Sobra de rodada anterior: o kill pode demorar, entao limpa antes de contar de novo.
$antigas = & tmux list-sessions -F '#{session_name}' 2>$null | Where-Object { $_ -like 'cp-fumaca-*' }
foreach ($velha in $antigas) { Nativo tmux kill-session -t "=$velha" | Out-Null }

$sessao = "cp-fumaca-$PID"
$rc = Nativo tmux new-session -d -s $sessao -c $env:TEMP 'cmd'
if ($rc -ne 0) {
    Erro 'o psmux nao criou uma sessao de teste - o app nao vai abrir sessao'
    exit 1
}
Ok 'o multiplexador cria sessao'

# Matar e testado SEPARADO de criar, porque medido no psmux 3.3.7 eles falham separado: o
# kill-session responde "session still present after 5s" com um `cmd` interativo no pane, enquanto
# a criacao funciona. Isso importa alem do instalador - o botao de apagar sessao do app chama
# exatamente este comando, e sessao que nao morre vira zumbi na lista.
Nativo tmux kill-session -t "=$sessao" | Out-Null
$morreu = $false
foreach ($i in 1..15) {
    if ((Nativo tmux has-session -t "=$sessao") -ne 0) { $morreu = $true; break }
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
  O backend fica em http://127.0.0.1:8765 e o PWA em http://localhost:5173.
  O dev server do vite escuta SO em 127.0.0.1 (vite.config.ts) - do celular se chega
  pelo Tailscale, nao pelo IP da LAN direto.

  Rodar na mao (se voce pulou o passo 7):
      cd backend  ; `$env:CP_LAN_BIND_IP='auto' ; uv run python -m app.main
      cd frontend ; npm run dev

  No celular: abra a URL do QR que o backend imprime e cole o token de backend\.env.
  Guia completo (Tailscale, instalar como PWA, cada tela): docs\USAGE.md

  O que este Windows ainda NAO tem:
  - wrappers do `codex` e do `pi`, e a extensao cp-state.ts do Pi. Sessao Codex ou Pi aberta
    por voce no terminal nao aparece; criada pelo app, funciona.
  - wrappers do `codex` e do `pi`, e a extensao cp-state.ts do Pi: idem. Sessao Codex ou Pi
    aberta por voce no terminal nao aparece; criada pelo app, funciona.
  - resurrect/continuum (sessoes sobreviverem a reboot): sao plugins de tmux em bash, e o
    psmux nao roda plugin de tmux. Fechou o Windows, as sessoes se foram.
"@
