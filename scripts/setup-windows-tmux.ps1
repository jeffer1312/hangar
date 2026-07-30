<#
.SYNOPSIS
  Deixa um PC Windows com a mesma config de psmux/tmux da maquina de referencia
  (scroll do mouse funcionando dentro do Claude Code, cores corretas, barra escondida).

.DESCRIPTION
  Instala o psmux (winget) se faltar e escreve o bloco gerenciado no ~/.tmux.conf a partir
  de docs/tmux.conf.windows.example.

  IDEMPOTENTE DE VERDADE: SUBSTITUI o conteudo entre os marcadores em vez de dar append.
  O instalador antigo so anexava, entao cada execucao acrescentava outro bloco identico -
  na maquina de referencia acumulou 15 blocos (14 deles com `set -g status off`).
  Inofensivo, mas o arquivo crescia pra sempre. Aqui o bloco antigo e removido antes de
  escrever o novo, e o que estiver FORA dos marcadores (config sua) nunca e tocado.

  Detalhes do formato, aprendidos no arquivo real desta maquina: ele vem com BOM UTF-8 e
  quebras CRLF. Por isso o regex aceita `\r?\n?` no fim, e por isso NAO se conta bloco com
  padrao ancorado em `^` - o BOM gruda na 1a linha e o `^#` deixa de casar nela (foi assim
  que uma contagem manual deu 14 em vez de 15).

.PARAMETER Apply
  Sem isto o script so MOSTRA o que faria (dry-run). Nada e escrito.

.PARAMETER SkipInstall
  Nao tenta instalar o psmux pelo winget; so mexe no ~/.tmux.conf.

.EXAMPLE
  .\scripts\setup-windows-tmux.ps1              # dry-run: mostra o diff
  .\scripts\setup-windows-tmux.ps1 -Apply       # aplica
#>
[CmdletBinding()]
param(
    [switch]$Apply,
    [switch]$SkipInstall
)

$ErrorActionPreference = 'Stop'

$BeginMark = '# >>> claude-pocket windows-tmux >>>'
$EndMark   = '# <<< claude-pocket windows-tmux <<<'
$Example   = Join-Path (Split-Path -Parent $PSScriptRoot) 'docs\tmux.conf.windows.example'

# PRECEDENCIA (docs/configuration.md do psmux): ele le o PRIMEIRO arquivo que existir, nesta
# ordem - e para. NAO faz merge. Escrever cego no ~/.tmux.conf era um bug: numa maquina que
# tenha ~/.psmux.conf, o .tmux.conf inteiro e IGNORADO e a config parece nao ter efeito.
$Candidatos = @(
    (Join-Path $HOME '.psmux.conf'),
    (Join-Path $HOME '.psmuxrc'),
    (Join-Path $HOME '.tmux.conf'),
    (Join-Path $HOME '.config\psmux\psmux.conf')
)
# PSMUX_CONFIG_FILE / -f vencem tudo. Se estiver setado, e nele que temos de escrever.
$Conf = if ($env:PSMUX_CONFIG_FILE) {
    Write-Host "==> PSMUX_CONFIG_FILE aponta pra: $($env:PSMUX_CONFIG_FILE)" -ForegroundColor Cyan
    $env:PSMUX_CONFIG_FILE
} else {
    # O que o psmux JA le hoje; se nenhum existe, cria o .tmux.conf (o mais familiar).
    $achado = $Candidatos | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($achado) { $achado } else { Join-Path $HOME '.tmux.conf' }
}

# Arquivo de menor precedencia que exista alem do escolhido = config morta que confunde.
$Sombra = $Candidatos | Where-Object { (Test-Path $_) -and ($_ -ne $Conf) }

function Write-Step { param([string]$Msg) Write-Host "==> $Msg" -ForegroundColor Cyan }
function Write-Warn { param([string]$Msg) Write-Host "!!  $Msg" -ForegroundColor Yellow }
function Write-Bad  { param([string]$Msg) Write-Host "XX  $Msg" -ForegroundColor Red }
function Write-Good { param([string]$Msg) Write-Host "ok  $Msg" -ForegroundColor Green }

# UNICO ponto de chamada a executavel externo. Com $ErrorActionPreference='Stop' (topo do arquivo),
# um nativo que escreva UMA linha em stderr faz o PS 5.1 embrulhar cada linha num ErrorRecord
# (NativeCommandError) e ABORTAR o script - mesmo com `2>$null`, e mesmo com exit code 0.
# Isso ja aconteceu de verdade aqui: rodando o instalador com o servidor tmux fora do ar, o script
# morreu na leitura de verificacao com "tmux.exe : psmux: no server running on session 'jeffer1312'".
# O pior e ONDE isso caia: dentro do diagnostico que existe justamente pra detectar um tmux errado,
# ou seja a checagem se autodestruia no unico cenario em que era necessaria.
# 'Continue' em vez de try/catch por chamada: o try/catch pegaria a excecao mas ainda perderia a
# saida ja emitida, e teria de ser repetido em todo lugar. Aqui stderr vira TEXTO normal, que e o
# que a gente quer ler. $global:UltimoExit guarda o codigo pra quem precisa decidir por ele.
function Invoke-Nativo {
    param([Parameter(Mandatory)][scriptblock]$Bloco)
    $antes = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $saida = & $Bloco 2>&1 | Out-String
        $global:UltimoExit = $LASTEXITCODE
        return $saida.Trim()
    } catch {
        $global:UltimoExit = -1
        return "$_"
    } finally {
        $ErrorActionPreference = $antes
    }
}

# ---------------------------------------------------------------------------------------
# Diagnostico do AMBIENTE. Escrever a config nao adianta se o `tmux` que roda nem for o
# psmux, ou se o terminal nao mandar evento de roda. Numa maquina "ja instalada" e
# EXATAMENTE aqui que mora a causa: a config fica certa e o scroll continua morto, o que
# faz parecer que a instalacao funcionou quando nao funcionou.
# ---------------------------------------------------------------------------------------
function Test-Ambiente {
    $problemas = @()

    # 1. QUEM responde por `tmux`. Este e o item numero 1: um tmux de MSYS2/Cygwin/Git-for-
    # Windows antes no PATH atende primeiro, e nele `mouse` nasce OFF (no psmux nasce ON).
    $tmuxCmd = Get-Command tmux -ErrorAction SilentlyContinue
    if (-not $tmuxCmd) {
        $problemas += 'tmux nao esta no PATH'
        Write-Bad 'tmux: nao encontrado no PATH'
    } else {
        # `tmux -V` do psmux imprime DUAS linhas ("tmux 3.3.7" + "psmux 3.3.7 (hash)").
        $ver = Invoke-Nativo { tmux -V }
        if ($ver -match 'psmux') {
            Write-Good "tmux -> psmux  ($($tmuxCmd.Source))"
            Write-Host  "    $($ver -replace '\r?\n', ' | ')"
        } else {
            Write-Bad "tmux NAO e o psmux: $($tmuxCmd.Source)"
            Write-Host "    versao reportada: $ver"
            Write-Host "    Um tmux de MSYS2/Cygwin/Git-for-Windows esta antes no PATH. Nele o"
            Write-Host "    mouse nasce OFF e o scroll no Claude Code nao funciona."
            Write-Host "    Conserto: ponha o diretorio do psmux ANTES no PATH do usuario, ex.:"
            Write-Host '      $p = "$env:LOCALAPPDATA\Microsoft\WinGet\Links"'
            Write-Host '      [Environment]::SetEnvironmentVariable("Path", "$p;" + [Environment]::GetEnvironmentVariable("Path","User"), "User")'
            Write-Host "    (abra um terminal NOVO depois). NAO faco isso automatico: mexer no PATH"
            Write-Host "    do usuario pode quebrar outras ferramentas que dependem daquele tmux."
            $problemas += 'tmux resolve pra um binario que nao e o psmux'
        }
    }

    # 2. Emulador. O conhost (janela crua) nao manda os mesmos eventos de roda do WT.
    if ($env:WT_SESSION) {
        Write-Good 'terminal: Windows Terminal'
    } else {
        Write-Warn 'terminal: NAO parece Windows Terminal (WT_SESSION vazio)'
        Write-Host "    No conhost.exe os eventos de roda nao chegam iguais. Rode no Windows Terminal."
        $problemas += 'terminal pode nao ser o Windows Terminal'
    }

    # 3. Config morta: arquivo de MAIOR precedencia que o escolhido nunca deveria existir
    # sem a gente saber - e a explicacao classica de "editei e nao mudou nada".
    foreach ($s in $Sombra) {
        Write-Warn "config ignorada pelo psmux (perde na precedencia): $s"
    }

    return $problemas
}

if (-not (Test-Path $Example)) { throw "referencia nao encontrada: $Example" }

Write-Step 'Diagnostico do ambiente'
$problemasAntes = Test-Ambiente
Write-Host ''

# --- 1. psmux -------------------------------------------------------------------------
if (-not $SkipInstall) {
    $psmux = Get-Command psmux -ErrorAction SilentlyContinue
    if ($psmux) {
        Write-Step "psmux ja instalado: $($psmux.Source)"
        # JA INSTALADO: reportar se ha upgrade, mas NUNCA atualizar sozinho. Trocar o binario
        # do psmux com sessoes VIVAS derruba o servidor e leva junto as sessoes do claude-pocket
        # (incl. a que esta rodando este script). Atualizacao e decisao do usuario, com as
        # sessoes fechadas.
        $linha = (Invoke-Nativo { winget list --id marlocarlo.psmux }) -split "`r?`n" |
                 Where-Object { $_ -match 'marlocarlo\.psmux' } | Select-Object -First 1
        if ($linha -and ($linha -match '\d+\.\d+\.\d+\s+\d+\.\d+\.\d+')) {
            Write-Warn "ha versao mais nova disponivel: $($linha.Trim())"
            Write-Host  '    Atualize com as sessoes FECHADAS:  winget upgrade --id marlocarlo.psmux'
        }
    } elseif ($Apply) {
        Write-Step 'Instalando psmux (winget marlocarlo.psmux)'
        winget install --id marlocarlo.psmux --accept-package-agreements --accept-source-agreements
    } else {
        Write-Step '[dry-run] instalaria psmux via winget (marlocarlo.psmux)'
    }
}

# --- 2. bloco gerenciado no ~/.tmux.conf ----------------------------------------------
# Le como UMA string (nao array): o -replace com (?s) precisa enxergar o arquivo inteiro
# pra casar um bloco multi-linha. -Raw tambem preserva o arquivo byte a byte fora do bloco.
$atual = if (Test-Path $Conf) { Get-Content $Conf -Raw -Encoding UTF8 } else { '' }

$corpo = Get-Content $Example -Raw -Encoding UTF8
$bloco = "$BeginMark`n$corpo`n$EndMark"

# Remove TODAS as ocorrencias do bloco (o append antigo pode ter deixado varias) - e por isso
# que o regex nao e ancorado e roda em modo singleline. [regex]::Escape: os marcadores tem
# caracteres que valem regex (>, <, -).
$padrao = '(?s)' + [regex]::Escape($BeginMark) + '.*?' + [regex]::Escape($EndMark) + '\r?\n?'
$antes  = ([regex]::Matches($atual, $padrao)).Count
$limpo  = [regex]::Replace($atual, $padrao, '')

# Tambem tira o bloco LEGADO do instalador antigo (marcador generico `claude-pocket`), que e
# quem duplicou. Sem isto, um `set -g status off` velho continuaria valendo junto do novo.
$padraoLegado = '(?s)# >>> claude-pocket >>>.*?# <<< claude-pocket <<<\r?\n?'
$legado = ([regex]::Matches($limpo, $padraoLegado)).Count
$limpo  = [regex]::Replace($limpo, $padraoLegado, '')

# NORMALIZA a cauda antes de concatenar. Antes era `$limpo += "\n"` seguido de "$limpo`n$bloco" —
# duas fontes de newline pro mesmo ponto de junção. Depois de arrancar o bloco, `$limpo` JA termina
# em \n (o regex consome o \r?\n final), e a interpolacao somava outro: o arquivo crescia 1 byte a
# cada execucao, `$novo -eq $atual` NUNCA dava true e o script se achava desatualizado pra sempre.
# Isso arrastava um estrago pior no backup logo abaixo (ver o Copy-Item). MEDIDO antes do fix:
# run1 7412b, run2 7413b, run3 7414b, run4 7415b — +1 byte por run.
# TrimEnd em vez de checar o ultimo char: a cauda pode ter \r\n, varios \n ou espaco, e o objetivo e
# uma saida DETERMINISTICA (mesma entrada logica -> mesmos bytes), nao preservar o que veio.
$limpo = $limpo.TrimEnd("`r", "`n", " ", "`t")
$novo = if ($limpo) { "$limpo`n`n$bloco`n" } else { "$bloco`n" }

Write-Step "Alvo: $Conf"
Write-Host "    (escolhido pela precedencia do psmux: .psmux.conf > .psmuxrc > .tmux.conf > .config\psmux\psmux.conf)"
foreach ($s in $Sombra) {
    Write-Warn "existe tambem $s - o psmux IGNORA esse arquivo (perde pra $([System.IO.Path]::GetFileName($Conf))). Nada sera escrito nele."
}
Write-Host "    blocos gerenciados encontrados : $antes"
Write-Host "    blocos LEGADOS encontrados     : $legado"
if ($legado -gt 1) { Write-Warn "$legado blocos legados duplicados - serao colapsados em 1 (bug do append antigo)" }

# `$escrever` em vez de `exit 0`: os dois desvios abaixo (nada a fazer / dry-run) SAIAM do script
# antes da verificacao e do relatorio de ambiente. Um PC com o arquivo ja certo mas com `tmux`
# resolvendo pro MSYS2 recebia "nada a fazer" em verde e terminava com o scroll morto - exatamente
# o "sairia dizendo pronto" que o diagnostico foi criado pra eliminar. Agora o fluxo e sempre o
# mesmo ate o fim; so a ESCRITA e condicional.
$escrever = $true
if ($novo -eq $atual) {
    Write-Step 'Config ja esta atualizada (nada a escrever).'
    $escrever = $false
} elseif (-not $Apply) {
    Write-Step '[dry-run] escreveria o bloco abaixo. Rode com -Apply pra valer.'
    Write-Host ($bloco -split "`n" | Select-Object -First 12 | Out-String)
    Write-Host '    [...]'
    $escrever = $false
}

if ($escrever -and (Test-Path $Conf)) {
    # NAO sobrescreve um .bak que ja existe. O `-Force` de antes destruia o unico backup do usuario:
    # a partir do 2o -Apply o .bak passava a conter a versao JA modificada por nos, e o arquivo
    # original sumia de vez. MEDIDO antes do fix: run1 bak=33b (original), run2 bak=7412b (a nossa
    # saida do run1). Somado a nao-idempotencia acima, bastavam DOIS -Apply.
    # O .bak e o unico registro do que existia ANTES de nos - vale mais que um backup recente. Quem
    # quiser um snapshot do estado atual copia na mao; quem rodar o instalador duas vezes nao pode
    # perder o original por isso.
    $bak = "$Conf.bak"
    if (Test-Path $bak) {
        Write-Step "Backup ja existe (preservado, e o estado PRE-instalador): $bak"
    } else {
        Copy-Item $Conf $bak
        Write-Step "Backup: $bak"
    }
}

if ($escrever) {
    # UTF8 sem BOM: o psmux le o conf como texto; um BOM no inicio vira lixo na 1a diretiva. E foi
    # BOM que criou as 15 duplicatas no install.ps1 (Set-Content -Encoding UTF8 poe BOM no PS 5.1,
    # o BOM gruda na 1a linha e o match do marcador passa a falhar nela).
    [System.IO.File]::WriteAllText($Conf, $novo, (New-Object System.Text.UTF8Encoding $false))
    Write-Step 'Escrito.'
}

# --- 3. aplicar na hora, se houver servidor de pe -------------------------------------
if ($escrever -and (Get-Command tmux -ErrorAction SilentlyContinue)) {
    # Sem servidor rodando o source-file sai != 0, e isso NAO e erro (o conf vale no proximo start).
    # Via Invoke-Nativo: o stderr do psmux ("no server running") viraria NativeCommandError.
    Invoke-Nativo { tmux source-file $Conf } | Out-Null
    Write-Step 'Config recarregada nas sessoes vivas (se havia servidor de pe).'
}

Write-Host ''
# --- 4. VERIFICACAO: o que o servidor REALMENTE ficou ----------------------------------
# Escrever o arquivo nao prova nada - a config pode nao ter sido lida (precedencia), o
# servidor pode estar velho, o binario pode ser outro. Aqui a gente le de volta do servidor
# vivo. Sem isto o script sairia dizendo "Pronto" com o scroll ainda morto.
Write-Step 'Verificacao (lendo do servidor, nao do arquivo)'
$esperado = @{ 'mouse' = 'on'; 'cursor-blink' = 'off'; 'automatic-rename' = 'off'; 'status' = 'off' }
$falhou = @()
if (Get-Command tmux -ErrorAction SilentlyContinue) {
    foreach ($k in $esperado.Keys) {
        $lido = Invoke-Nativo { tmux show -g $k }
        $val  = ($lido -split '\s+')[-1]
        if ($val -eq $esperado[$k]) { Write-Good "$k = $val" }
        else { Write-Bad "$k = '$val' (esperado '$($esperado[$k])')"; $falhou += $k }
    }
    if ($falhou.Count -gt 0) {
        Write-Warn 'Opcoes acima nao pegaram. Causas em ordem: (1) o servidor psmux ja estava'
        Write-Host '    de pe e le a config no START - feche TODAS as sessoes e abra de novo;'
        Write-Host '    (2) existe config de maior precedencia sobrescrevendo (avisos acima);'
        Write-Host '    (3) o `tmux` nao e o psmux (ver diagnostico no inicio).'
    }
} else {
    Write-Warn 'tmux nao encontrado - nada a verificar.'
}

Write-Host ''
if ($problemasAntes.Count -gt 0) {
    Write-Bad "A config foi escrita, MAS o ambiente tem $($problemasAntes.Count) problema(s) que ela NAO conserta:"
    foreach ($p in $problemasAntes) { Write-Host "    - $p" }
    Write-Host '    Sem resolver isso, o scroll continua sem funcionar mesmo com a config certa.'
} else {
    Write-Step 'Pronto. Ambiente e config conferidos.'
}
Write-Host '    Teste final: abra o Claude Code numa sessao NOVA e role a roda do mouse.'

# EXIT EXPLICITO. Ao trocar os `exit 0` pela flag $escrever (pra nada-a-fazer e dry-run nao pularem
# a verificacao), o script ficou SEM nenhum exit - e quem chama passou a ler LIXO: o $LASTEXITCODE
# que sobrasse do ultimo executavel nativo rodado aqui dentro, na pratica o ultimo `tmux show -g`.
# O install.ps1 decide por esse valor (`if ($LASTEXITCODE -eq 0)`), entao o ramo "aplicada COM
# RESSALVAS" nunca disparava por merito proprio. O $global:UltimoExit do Invoke-Nativo tambem nao
# resolvia: ninguem le, e nao atravessa o limite de processo/escopo de quem invoca o script.
# Contrato daqui pra frente, o que o install.ps1 ja espera: 0 = ambiente e config OK; 1 = a config
# foi escrita mas ALGO nao esta bom (ambiente que a config nao conserta, ou opcao que nao pegou no
# servidor). Nao ha ramo de "erro fatal" separado: falha dura ja aborta via throw.
if ($problemasAntes.Count -gt 0 -or $falhou.Count -gt 0) { exit 1 }
exit 0
