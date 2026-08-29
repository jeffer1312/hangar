# hangar - instalacao completa no Windows.
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

# Da pra PERGUNTAR alguma coisa nesta execucao? Medido em 21/08/2026 nesta VM: com o stdin vindo
# de um pipe — que e o caso de `irm ... | iex` chamado por outro processo, e de qualquer execucao
# por SSH/tarefa — `[Console]::IsInputRedirected` volta True e o `Read-Host` responde STRING VAZIA
# na hora, sem esperar ninguem. Era assim que o passo 3/8 gerava um token aleatorio "porque o
# usuario apertou Enter" e seguia adiante sem nunca mostrar o token: a pessoa terminava a
# instalacao sem a credencial. Ler do console real (CONIN$) nao e caminho: o File.Open recusa o
# dispositivo ("FileStream foi solicitado a abrir um dispositivo que nao era um arquivo") e a
# alternativa seria P/Invoke de CreateFile dentro de um instalador.
$script:Interativo = -not [Console]::IsInputRedirected

function Pergunte($texto) {
    if ($Sim) { return $true }
    # Sem entrada interativa nao ha o que perguntar. Antes disto o Read-Host devolvia '' e o valor
    # DEFAULT (sim) valia do mesmo jeito — o comportamento nao muda, o que muda e ele ser escolha
    # escrita em vez de efeito colateral de uma pergunta que ninguem viu.
    if (-not $script:Interativo) { Nota "$texto -> sim (sem entrada interativa, assumindo o padrao)"; return $true }
    $r = Read-Host "$texto [S/n]"
    return ($r -eq '' -or $r -match '^[SsYy]')
}

function Escrever-Texto($caminho, $texto, [switch]$ComBom) {
    <#
      Escrita de texto do instalador. Existe porque `Set-Content`/`Out-File`/`Add-Content` NAO
      escrevem a mesma coisa no PowerShell 5.1 e no 7 — medido nesta VM em 22/08/2026
      (5.1.26100.4202 vs 7.6.5), gravando a mesma string com acento:

        chamada                       5.1                      7.6.5
        Set-Content -Encoding UTF8    UTF-8 COM BOM            UTF-8 sem BOM
        Out-File    -Encoding utf8    UTF-8 COM BOM            UTF-8 sem BOM
        Set-Content (sem -Encoding)   ANSI (cp1252)            UTF-8 sem BOM
        Add-Content (sem -Encoding)   ANSI (cp1252)            UTF-8 sem BOM

      Ou seja: o MESMO instalador produzia arquivos diferentes conforme o PowerShell de quem
      rodou. Aqui o encoding e dito, e o BOM e escolha de quem chama — porque as duas respostas
      existem: arquivo LIDO PELO PowerShell precisa dele (ver Perfis-Do-Usuario), e .env / JSON /
      script com shebang nao podem te-lo (o BOM ja fez o CP_AUTH_TOKEN virar chave invisivel aqui,
      install.ps1:241).
    #>
    $pai = Split-Path -Parent $caminho
    if ($pai) { New-Item -ItemType Directory -Force -Path $pai | Out-Null }
    [System.IO.File]::WriteAllText($caminho, $texto, (New-Object System.Text.UTF8Encoding $ComBom.IsPresent))
}

function Escrever-Lancador($caminho, $texto, [ValidateSet('cmd','sh','vbs')][string]$Tipo) {
    <#
      Escreve um lancador (.cmd/.sh/.vbs) no encoding que o INTERPRETADOR dele entende, e devolve
      $true se o arquivo mudou (pra quem chama dizer "criado" ou "ja atualizado").

      Todos estes arquivos carregam CAMINHO dentro — o do checkout, o do python, o do log em
      %LOCALAPPDATA% (que tem o nome do usuario). Todos eram gravados com `-Encoding ASCII`, e
      ASCII transforma qualquer acento em `?`: num "C:\Users\Joao\..." (com til) o lancador nasce
      apontando pra um caminho que nao existe, roda, e diz "o sistema nao pode encontrar o
      caminho". Falha silenciosa de instalador, que e a classe de bug que este trabalho persegue.

      Qual encoding serve NAO e opiniao — medido em 22/08/2026, cada arquivo executado de verdade
      com um caminho contendo "Joao" com til (console em codepage 850):

        .cmd (cmd.exe)      ASCII FALHOU | ANSI 1252 FALHOU | UTF-8 FALHOU | OEM 850 OK | UTF-8+chcp OK
        .vbs (wscript)      ASCII FALHOU | ANSI 1252 OK     | OEM FALHOU   | UTF-16LE c/ BOM OK
        .sh  (bash do Git)  ASCII FALHOU | ANSI 1252 OK     | UTF-8 sem BOM OK

      Escolhas: `.cmd` vai na codepage OEM do console (o `chcp 65001` tambem funciona, mas ele muda
      a codepage do console DE QUEM CHAMA — efeito colateral visivel num terminal interativo);
      `.vbs` vai em UTF-16LE com BOM, que o WSH detecta sozinho e nao depende da codepage ANSI da
      regiao; `.sh` vai em UTF-8 sem BOM, o padrao do bash (e um BOM antes do `#!` quebra o
      shebang, como o proprio install.ps1 ja documenta mais abaixo).

      Conteudo 100% ASCII sai byte a byte igual ao de antes nos tres casos.
    #>
    $enc = switch ($Tipo) {
        'cmd' { [System.Text.Encoding]::GetEncoding([Console]::OutputEncoding.CodePage) }
        'vbs' { New-Object System.Text.UnicodeEncoding $false, $true }   # UTF-16LE + BOM
        'sh'  { New-Object System.Text.UTF8Encoding $false }
    }
    $bytes = $enc.GetBytes($texto)
    if (Test-Path $caminho) {
        $atual = [System.IO.File]::ReadAllBytes($caminho)
        if ($atual.Length -eq $bytes.Length -and -not (Compare-Object $atual $bytes)) { return $false }
    }
    $pai = Split-Path -Parent $caminho
    if ($pai) { New-Item -ItemType Directory -Force -Path $pai | Out-Null }
    [System.IO.File]::WriteAllBytes($caminho, $bytes)
    return $true
}

function Ler-Texto($caminho) {
    <#
      Le respeitando o que o arquivo E, nao o que a versao do PowerShell chuta. `Get-Content` sem
      BOM assume ANSI no 5.1 e UTF-8 no 7: o mesmo arquivo, duas leituras. Como este instalador
      REESCREVE o perfil inteiro, chutar errado corrompe o que ja estava la (o comentario do
      Set-EnvKey conta a mesma historia com o token acentuado).

      Ordem: BOM manda; sem BOM, tenta UTF-8 ESTRITO (throwOnInvalidBytes) e so entao cp1252 —
      texto valido em UTF-8 quase nunca e cp1252 por acidente, e o contrario nao vale.
    #>
    if (-not (Test-Path $caminho)) { return $null }
    $bytes = [System.IO.File]::ReadAllBytes($caminho)
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        return [System.Text.Encoding]::UTF8.GetString($bytes, 3, $bytes.Length - 3)
    }
    try {
        return (New-Object System.Text.UTF8Encoding $false, $true).GetString($bytes)
    } catch {
        return [System.Text.Encoding]::GetEncoding(1252).GetString($bytes)
    }
}

function Perfis-Do-Usuario {
    <#
      TODOS os perfis que precisam do bloco, nao so o da versao que esta rodando.

      `$PROFILE.CurrentUserAllHosts` aponta pra pastas DIFERENTES em cada versao (medido aqui):
        5.1 -> ...\Documents\WindowsPowerShell\profile.ps1
        7.x -> ...\Documents\PowerShell\profile.ps1
      Instalar pelo pwsh 7 deixava todo terminal 5.1 — o padrao do Windows — sem o wrapper, e
      nada dizia isso: a pessoa abria o terminal de sempre e a sessao continuava invisivel pro app.

      O caminho da outra versao e DERIVADO do atual (troca so o nome da pasta), pra herdar um
      Documents redirecionado por OneDrive/politica em vez de remontar o caminho na mao. A outra
      versao so entra se ela EXISTE na maquina: o 5.1 vem no Windows; o 7 pode estar so como app
      da Store (medido: winget instala em %LOCALAPPDATA%\Microsoft\WindowsApps\pwsh.exe, que nao
      aparece no PATH de sessao SSH nao-interativa — procurar so por `pwsh` da falso negativo).
    #>
    $atual = $PROFILE.CurrentUserAllHosts
    $alvos = @($atual)
    $pasta = Split-Path -Parent $atual
    $nome = Split-Path -Leaf $pasta
    if ($nome -eq 'WindowsPowerShell') {
        $temSete = (Tem 'pwsh') -or
                   (Test-Path (Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\pwsh.exe')) -or
                   (Test-Path (Join-Path $env:ProgramFiles 'PowerShell\7\pwsh.exe'))
        if ($temSete) { $alvos += (Join-Path (Split-Path -Parent $pasta) 'PowerShell\profile.ps1') }
    } elseif ($nome -eq 'PowerShell') {
        # O 5.1 vem no Windows, entao o perfil dele SEMPRE entra: e o terminal que a pessoa abre
        # por padrao, e o que o proprio app usa pra criar sessao.
        $alvos += (Join-Path (Split-Path -Parent $pasta) 'WindowsPowerShell\profile.ps1')
    }
    return $alvos
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

function Instale-ClaudeCode {
    # NAO vai por winget. O pacote 'Anthropic.ClaudeCode' de la depende de alguem atualizar o
    # manifesto da comunidade, e fica pra tras das versoes que a Anthropic publica - o usuario
    # instalava e ja nascia velho. O instalador oficial baixa do canal de releases deles, confere
    # o SHA256 do binario e chama `claude.exe install`.
    # Conferido em 06/08/2026: claude.ai/install.ps1 redireciona pra
    # downloads.claude.ai/claude-code-releases/bootstrap.ps1.
    # O PATH ele NAO resolve - medido em 21/08/2026 num Windows 11 limpo (Claude Code 2.1.238): o
    # binario cai em ~\.local\bin\claude.exe e o proprio instalador avisa "is not in your PATH",
    # mandando editar nas Propriedades do Sistema. Sem ninguem fazer isso o `Tem 'claude'` abaixo
    # falhava e a instalacao inteira parava no passo 1 com "feche e abra o terminal", que nao
    # resolveria nada. Quem poe no PATH do usuario e este script (mesmo diretorio que o 7b ja usa
    # pro hangar-send).
    $rotulo = 'Claude Code'
    if (Tem 'claude') { Ok $rotulo; return $true }
    if ($SoChecar) { Falta "$rotulo - e o que o app pilota"; $script:pendencias += $rotulo; return $false }
    if ($Update)   { Erro "$rotulo faltando (-Update nao instala dependencia)"; $script:pendencias += $rotulo; return $false }

    Write-Host '  .. instalando Claude Code (instalador oficial da Anthropic)'
    # Em processo FILHO, nao com `iex` aqui dentro: o bootstrap da Anthropic chama `exit 1` nos
    # caminhos de erro dele (Windows 32 bits, download falho, checksum errado). Avaliado no mesmo
    # processo, esse `exit` mataria o install.ps1 inteiro no meio, sem explicacao pro usuario.
    # Com `Nativo` o exit code volta como numero e a decisao continua aqui.
    $rc = Nativo powershell -NoProfile -ExecutionPolicy Bypass -Command `
        "irm https://claude.ai/install.ps1 | iex"
    if ($rc -ne 0) {
        Erro "Claude Code nao instalou (exit $rc)"
        Nota 'manual: irm https://claude.ai/install.ps1 | iex'
        $script:pendencias += $rotulo
        return $false
    }
    $binClaude = Join-Path $HOME '.local\bin'
    if (Test-Path (Join-Path $binClaude 'claude.exe')) {
        $pathUsuario = [Environment]::GetEnvironmentVariable('Path', 'User')
        if ($pathUsuario -notlike "*$binClaude*") {
            [Environment]::SetEnvironmentVariable('Path', "$pathUsuario;$binClaude", 'User')
            Nota "$binClaude adicionado ao PATH do usuario (o instalador da Anthropic nao faz isso)"
        }
    }
    Atualiza-Path
    if (Tem 'claude') { Ok "$rotulo instalado"; return $true }
    Erro "Claude Code instalou mas o comando nao aparece (esperado em $binClaude\claude.exe)"
    Nota 'feche e abra o terminal, e rode o install de novo'
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
Instale-ClaudeCode | Out-Null
Instale 'Python'                'py'     'Python.Python.3.14'   'o backend e Python'        | Out-Null
# 3.14, nao 3.13: backend/pyproject.toml exige >=3.14 (e .python-version = 3.14). Com o 3.13 o
# `uv sync` ate funcionava - baixava um 3.14 gerenciado por conta propria - mas o Python do winget
# virava peso morto, servindo so ao shim python3 do hangar-send. Um Python so pros dois papeis.
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

# ripgrep tambem e OPCIONAL, e o preco de nao ter e ESPECIFICO: quem chama o `rg` e a busca por
# conteudo entre sessoes (a lupa; backend/app/search.py), e sem o binario ela devolve lista VAZIA -
# a tela diz "nenhum resultado" pra uma busca que nunca rodou. No Linux o ripgrep costuma ja estar
# ai; no Windows nao vem com o sistema, e foi assim que a busca ficou muda nesta VM. O backend
# agora tambem registra o aviso no log, mas quem resolve de verdade e instalar.
# NAO usa `Instale`: aquele empurra pra $pendencias e a linha seguinte aborta a instalacao inteira -
# desproporcional pra uma ferramenta que so a lupa usa.
if (-not (Tem 'rg')) {
    Falta 'ripgrep ausente - a busca por conteudo entre sessoes (a lupa) volta sempre vazia'
    if (-not $SoChecar -and (Pergunte '      Instalar o ripgrep agora?')) {
        Write-Host '  .. instalando ripgrep (BurntSushi.ripgrep.MSVC)'
        Nativo winget install --id BurntSushi.ripgrep.MSVC --exact --silent `
            --accept-package-agreements --accept-source-agreements | Out-Null
        Atualiza-Path
        if (Tem 'rg') { Ok 'ripgrep instalado' }
        else { Erro 'ripgrep nao instalou - so a lupa fica vazia; o resto do app funciona' }
    }
} else { Ok 'ripgrep' }

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
function Porta-Do-Env {
    param([string]$Chave, [int]$Default)
    if (Test-Path $envFile) {
        $l = Select-String -Path $envFile -Pattern "^$Chave=(\d+)" -ErrorAction SilentlyContinue |
             Select-Object -First 1
        if ($l) { return [int]$l.Matches[0].Groups[1].Value }
    }
    return $Default
}
function Set-EnvKey {
    param([Parameter(Mandatory)][string]$Chave, [Parameter(Mandatory)][string]$Valor)
    # SUBSTITUI em vez de acrescentar: o `Add-Content` de antes repetia a chave a cada re-execucao, e
    # `Porta-Do-Env` le a PRIMEIRA ocorrencia — um CP_PORT duplicado faria o instalador mirar a porta
    # errada e "parar o servico" sem parar nada. De graca, conserta tambem o arquivo sem quebra de
    # linha no fim, onde o Add-Content grudava a chave nova na ultima linha.
    #
    # WriteAllText com UTF8Encoding($false), e NUNCA `Set-Content -Encoding UTF8`: no PS 5.1 aquele
    # poe BOM (install.ps1:486-490), e o .env e lido pelo pydantic-settings com encoding 'utf8', nao
    # 'utf-8-sig'. O U+FEFF nao e espaco pro regex do python-dotenv, entao a PRIMEIRA chave do arquivo
    # vira "﻿CP_AUTH_TOKEN" e simplesmente some: o token cai pro default 'change-me' e o QR passa
    # a entregar `?token=change-me`. Hoje o arquivo nasce por Add-Content sem -Encoding (ASCII, sem
    # BOM), entao usar Set-Content aqui seria REGRESSAO.
    #
    # -Encoding UTF8 na LEITURA tambem, e pelo motivo espelhado: SEM ele, o Get-Content do PS 5.1
    # sem BOM no arquivo decodifica pela codepage ANSI do sistema (cp1252) - e como esta funcao
    # REESCREVE o arquivo INTEIRO a cada chamada, todo valor acentuado ja gravado (o token
    # memoravel pedido no passo 3/8) ia sendo corrompido de novo a cada execucao do instalador
    # (achado CRITICO da revisao final: "cafezinho" virava lixo de bytes na 2a rodada, e o celular
    # passava a levar 401 sem nada explicar por que).
    $linha = "$Chave=$Valor"
    $linhas = @()
    if (Test-Path $envFile) { $linhas = @(Get-Content -Path $envFile -Encoding UTF8) }
    $achou = $false
    # NUNCA `$novo = foreach (...) {...}`: com $linhas vazio (.env novo, instalacao do zero) o loop
    # roda zero vezes e o foreach-como-expressao vira $null, nao array vazio — dai `@($novo) + $linha`
    # vira [$null, $linha] e sobra uma linha em branco no topo do arquivo pra sempre (nenhuma chamada
    # posterior remove linha vazia, so linha que casa com alguma chave).
    $novo = @()
    foreach ($l in $linhas) {
        if ($l -match "^\s*$([regex]::Escape($Chave))=") {
            if (-not $achou) { $achou = $true; $novo += $linha }   # a 1a vira a boa, as outras somem
        } else { $novo += $l }
    }
    if (-not $achou) { $novo += $linha }
    New-Item -ItemType Directory -Force -Path (Split-Path $envFile) | Out-Null
    [System.IO.File]::WriteAllText($envFile, (($novo -join "`r`n") + "`r`n"),
                                   (New-Object System.Text.UTF8Encoding $false))
}
$temToken = (Test-Path $envFile) -and (Select-String -Path $envFile -Pattern '^CP_AUTH_TOKEN=' -Quiet)
function Token-Aleatorio {
    # RNGCryptoServiceProvider, nao RandomNumberGenerator::Fill: o segundo e .NET Core e nao
    # existe no PowerShell 5.1 que vem no Windows.
    $bytes = New-Object byte[] 24
    (New-Object System.Security.Cryptography.RNGCryptoServiceProvider).GetBytes($bytes)
    return (-join ($bytes | ForEach-Object { $_.ToString('x2') }))
}

function Token-Do-Env {
    # -Encoding UTF8 pelo mesmo motivo do Set-EnvKey: sem ele o Get-Content do PS 5.1 decodifica
    # arquivo sem BOM pela codepage ANSI, e um token acentuado ("cafezinho" com acento) voltaria
    # corrompido — o resumo do fim mostraria uma credencial que nao e a que esta no arquivo.
    if (-not (Test-Path $envFile)) { return $null }
    foreach ($l in @(Get-Content -Path $envFile -Encoding UTF8)) {
        if ($l -match '^\s*CP_AUTH_TOKEN=(.*)$') { return $Matches[1].Trim() }
    }
    return $null
}

if ($temToken) {
    Ok 'backend\.env ja tem CP_AUTH_TOKEN (mantido)'
} elseif ($Sim) {
    Set-EnvKey -Chave 'CP_AUTH_TOKEN' -Valor (Token-Aleatorio)
    Ok 'CP_AUTH_TOKEN aleatorio gerado (modo -Sim nao pergunta)'
} elseif (-not $script:Interativo) {
    # Sem stdin interativo nao da pra perguntar, e o silencio aqui era o pior dos mundos: token
    # sorteado, gravado, e a pessoa terminando a instalacao sem saber qual e. Gera e MOSTRA — aqui
    # e de novo no resumo do fim, que e onde quem rolou a tela vai olhar.
    $novoToken = Token-Aleatorio
    Set-EnvKey -Chave 'CP_AUTH_TOKEN' -Valor $novoToken
    Falta 'entrada nao-interativa (o stdin vem de um pipe): nao da pra perguntar o token'
    Nota 'gerei um aleatorio. ANOTE — ele aparece de novo no fim, e e o que voce digita no celular:'
    Write-Host ""
    Write-Host "      $novoToken" -ForegroundColor Yellow
    Write-Host ""
    Nota 'pra escolher um token que voce lembre, rode o instalador num terminal aberto por voce.'
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
    Set-EnvKey -Chave 'CP_AUTH_TOKEN' -Valor $token
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
# Test-Path .git ANTES de chamar o git: sem repositorio, o `rev-parse` escreve "fatal: not a git
# repository" no stderr, e com $ErrorActionPreference='Stop' isso e excecao TERMINANTE mesmo com
# `2>$null` - medido em 21/08/2026 numa copia sem historico: o instalador morria aqui, no passo 4,
# em vez de cair no "sem git nao da pra saber o que mudou" logo abaixo, que existe pra este caso.
if ((Tem 'git') -and (Test-Path "$raiz\.git")) {
    $commit = (& git -C $raiz rev-parse HEAD 2>$null)
    $sujo = (& git -C $raiz status --porcelain -- frontend 2>$null) -join "`n"
    if ($commit) { $marca = "$commit`n$sujo" }
}

$precisa = $true
if ((Test-Path $dist) -and (Test-Path $modulos)) {
    if ($marca -and (Test-Path $marcaArq)) {
        # `Ler-Texto` e nao `Get-Content -Raw` (que era o que estava aqui): sem -Encoding, o 5.1
        # decodifica arquivo sem BOM pela codepage ANSI e o 7 por UTF-8 — e a marca carrega a saida
        # do `git status --porcelain`, onde entra nome de arquivo acentuado. Lendo errado, a
        # comparacao diz "mudou" e o front e rebuildado a cada execucao, por um motivo que nao tem
        # nada a ver com o git. Mesmo furo do .env e do settings.json, terceira porta.
        # (O `-Raw` original existia porque sem ele o Get-Content devolve ARRAY de linhas e a
        # comparacao com a string falharia sempre; `Ler-Texto` ja devolve o arquivo inteiro.)
        $precisa = ((Ler-Texto $marcaArq) -ne $marca)
    } elseif (-not $marca) {
        # Sem git nao da pra saber o que mudou; rebuildar e a escolha segura.
        $precisa = $true
    }
}

# Definida no nivel do MODULO, nunca dentro de um `if`: o PowerShell nao tem hoisting, entao uma
# funcao declarada num ramo que nao executa simplesmente NAO EXISTE. Ela nasceu dentro do
# `if ($precisa)` do passo do frontend e isso quebrava justamente o caso comum — um `-Update` que
# nao mexe em frontend/ pula o ramo, e a chamada do passo 7 estourava CommandNotFound no meio do
# registro das tarefas, com o backend ja registrado e nao reiniciado.
# `npm ci`. Ver o comentario naquele passo.
$script:wmiMorto = $false

function Tabela-De-Processos([int]$TimeoutSeg = 20) {
    <#
      A tabela de processos (pid, pai, nome, cmdline, nascimento) COM TETO DE TEMPO. `$null` = nao
      consegui ler; quem chama tem que tratar isso como "nao sei quem e quem" e nao matar nada.

      Existe porque o `Get-CimInstance Win32_Process` pode simplesmente NAO VOLTAR. Medido nesta VM
      em 22/08/2026: `Get-CimInstance Win32_Process`, `Get-WmiObject Win32_Process` e ate
      `Get-CimInstance Win32_OperatingSystem` estouraram 25s sem responder, enquanto `Get-Process`
      (que nao passa pelo WMI) devolveu os 438 processos na hora — WMI degradado, nao volume.
      O instalador ficava PENDURADO no passo 4/8, com o front ja derrubado, sem nada na tela: duas
      execucoes travadas em 18 e 26 minutos ate serem mortas na mao. O `-ErrorAction
      SilentlyContinue` que ja estava ali cobre erro, e erro nao era o problema — silencio era.

      UMA varredura por chamada de Pare-Servico (antes eram tres identicas) e nada de cache entre
      passos: a tabela muda enquanto o instalador roda, e decidir matar por uma foto velha e como
      matar por pid reciclado.
    #>
    if ($script:wmiMorto) { return $null }
    $j = Start-Job -ScriptBlock {
        Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
            Select-Object ProcessId, ParentProcessId, Name, CommandLine, CreationDate
    }
    if (Wait-Job $j -Timeout $TimeoutSeg) {
        $tabela = @(Receive-Job $j)
        Remove-Job $j -Force -ErrorAction SilentlyContinue
        if ($tabela.Count -gt 0) { return $tabela }
        return $null
    }
    Stop-Job $j -ErrorAction SilentlyContinue
    Remove-Job $j -Force -ErrorAction SilentlyContinue
    $script:wmiMorto = $true      # falhou uma vez, nao paga o teto de novo a cada passo
    Erro "o WMI desta maquina nao respondeu em ${TimeoutSeg}s (Win32_Process)"
    Nota 'sem a tabela de processos eu NAO derrubo nada — servico velho pode continuar de pe e'
    Nota 'segurar porta ou arquivo. Se algum passo falhar por isso, feche o processo na mao.'
    Nota 'pra consertar o WMI:  winmgmt /verifyrepository   (e, se preciso, /salvagerepository)'
    return $null
}

function Pare-Servico {
    param([string]$Nome, [int]$Porta, [string]$Padrao, [string]$Exe)
    $alvos = @()
    # UMA leitura da tabela pros tres usos abaixo (linhagem, casamento por padrao e BFS dos
    # descendentes). $null = o WMI nao respondeu; o `if` de cada uso degrada pro caminho seguro.
    $tabela = Tabela-De-Processos
    # ANCESTRAIS do instalador, jamais alvos. Medido em 08/08/2026: quem edita um arquivo do front e
    # chama o instalador no MESMO comando deixa o caminho do checkout na cmdline do proprio shell —
    # o casamento por substring pegava esse shell, e como o BFS abaixo desce nos descendentes, matar
    # o pai levava o instalador junto. Foi exatamente o que aconteceu: exit 255 logo depois de
    # imprimir '4/8 Frontend', com o front JA derrubado e o `npm ci` nunca executado, deixando a
    # maquina em 502 ate alguem reerguer a tarefa na mao. O guard `-ne $PID` de antes protegia so o
    # processo atual, nunca a linhagem dele.
    $paisMapa = @{}
    $nascMapa = @{}
    foreach ($p in @($tabela)) {
        if ($null -eq $p) { continue }
        $paisMapa[[int]$p.ProcessId] = [int]$p.ParentProcessId
        $nascMapa[[int]$p.ProcessId] = $p.CreationDate
    }
    # Enumeracao de processo FALHANDO nao pode virar linhagem de um elemento so: o `-ErrorAction
    # SilentlyContinue` engole hiccup do WMI, e com o mapa vazio a protecao degradaria exatamente
    # pro guard antigo (`so o proprio pid`) que deixou o instalador matar o shell pai — de novo, e
    # agora sem ninguem perceber. Sem saber quem e parente de quem, o certo e nao matar nada.
    if ($paisMapa.Count -eq 0) {
        # ${Nome} e nao $Nome: dentro de aspas duplas, um cifrao seguido de dois-pontos e lido como
        # QUALIFICADOR (o mesmo mecanismo de $env:PATH e $script:pendencias), e o espaco depois dos
        # dois-pontos nao e nome valido -> erro de PARSE, que no PowerShell derruba o arquivo INTEIRO
        # antes da primeira linha rodar. Ou seja: um `git pull` deixaria o instalador inutilizavel,
        # sem passo 7, sem registrar nem reiniciar tarefa nenhuma.
        Falta "nao consegui enumerar processos (WMI) — NAO vou parar ${Nome}: sem a arvore de processos, o kill pode derrubar o shell que roda este instalador"
        return 0
    }
    $linhagem = New-Object System.Collections.Generic.HashSet[int]
    $cur = $PID
    $meuNasc = $nascMapa[$PID]
    while ($cur -and $linhagem.Add($cur)) {
        $pai = $paisMapa[$cur]
        # PID e RECICLADO: o ppid e so um numero gravado no nascimento, e se aquele pai morreu o
        # numero pode pertencer hoje a um processo qualquer — inclusive ao Vite que a gente QUER
        # matar, que assim escaparia por coincidencia numerica. Ancestral de verdade nasceu ANTES;
        # quem aparece como "pai" e nasceu DEPOIS e outro processo com o numero reaproveitado.
        if ($pai -and $meuNasc -and $nascMapa[$pai] -and $nascMapa[$pai] -gt $meuNasc) { break }
        $cur = $pai
    }
    # Por PORTA e o criterio mais preciso: quem esta segurando o socket e exatamente quem
    # impediria a instancia nova de subir.
    if ($Porta -gt 0) {
        $alvos += (Get-NetTCPConnection -State Listen -LocalPort $Porta -ErrorAction SilentlyContinue |
                   Select-Object -ExpandProperty OwningProcess)
    }
    # Por PADRAO pega o que ja largou a porta mas continua vivo (meio-termo de um crash).
    #
    # `-Exe` estreita isso pro EXECUTAVEL certo, e nao e refinamento: a cmdline so MENCIONAR o
    # caminho do checkout nao faz de ninguem um servidor. Um `Start-Sleep` com o caminho num
    # COMENTARIO era morto por este filtro — medido —, e o caso banal e o pior: o terminal de onde
    # se roda o instalador (quem editou um arquivo do front no mesmo comando), um editor aberto, um
    # grep. Casando tambem o nome do processo, sobra quem de fato executa o front.
    if ($Padrao) {
        $alvos += (@($tabela) |
                   Where-Object { $_ -and $_.CommandLine -and $_.CommandLine -match $Padrao -and
                                  (-not $Exe -or $_.Name -match $Exe) } |
                   Select-Object -ExpandProperty ProcessId)
    }
    # Fora: o proprio instalador e TODA a linhagem dele (ver o comentario da $linhagem acima).
    $alvos = @($alvos | Where-Object { $_ -and -not $linhagem.Contains([int]$_) } | Select-Object -Unique)
    # E fora TAMBEM o processo da ATUALIZACAO, por reconhecimento direto da cmdline (`app.atualizar`)
    # e nao por linhagem. Ele roda como `...\backend\.venv\Scripts\python.exe -m app.atualizar`, o
    # que casa nos DOIS criterios do filtro acima (caminho do checkout + nome `python`) — e e ele
    # quem esta chamando este instalador. Ou seja: sem esta linha o `-Update` mata quem o invocou,
    # no meio da propria atualizacao. Aconteceu em 25/08/2026 no Windows: o lock e o processo
    # morreram no minuto em que o instalador reportou "instancia anterior derrubada (2 processos)",
    # e a atualizacao ficou congelada na etapa 4 pra sempre. A protecao por LINHAGEM ja existia e
    # nao bastou; reconhecer pelo comando nao depende do mapa de pais estar completo nem do WMI ter
    # respondido. No Linux quem resolve isso e o escopo transiente do systemd, que aqui nao existe.
    $atualizador = @(@($tabela) |
        Where-Object { $_ -and $_.CommandLine -and $_.CommandLine -match 'app\.atualizar' } |
        Select-Object -ExpandProperty ProcessId)
    if ($atualizador.Count -gt 0) {
        $alvos = @($alvos | Where-Object { -not ($atualizador -contains [int]$_) })
    }
    if ($alvos.Count -eq 0) { return 0 }
    # Descendentes RECURSIVOS, nao um nivel so. O backend nasce `uv -> python -> python` e quem
    # segura a porta e o NETO: parar so o pai (ou pai+filhos) deixava a porta presa e a instancia
    # nova colidia igual. Varredura unica da tabela de processos + BFS, em vez de um Get-CimInstance
    # por nivel.
    $mapa = @{}
    foreach ($p in @($tabela)) {
        if ($null -eq $p) { continue }
        if (-not $mapa.ContainsKey([int]$p.ParentProcessId)) { $mapa[[int]$p.ParentProcessId] = @() }
        $mapa[[int]$p.ParentProcessId] += [int]$p.ProcessId
    }
    $todos = New-Object System.Collections.Generic.HashSet[int]
    $fila = New-Object System.Collections.Generic.Queue[int]
    foreach ($a in $alvos) { [void]$fila.Enqueue([int]$a) }
    while ($fila.Count -gt 0) {
        $cur = $fila.Dequeue()
        if ($linhagem.Contains([int]$cur) -or -not $todos.Add($cur)) { continue }
        foreach ($f in ($mapa[$cur] | Where-Object { $_ })) { [void]$fila.Enqueue([int]$f) }
    }
    # Conta o que MORREU, nao o que foi tentado. Com -ErrorAction SilentlyContinue o Stop-Process
    # engole "acesso negado" em silencio, e o contador antigo ($todos.Count) reportava sucesso pra
    # kill que nao aconteceu - a mensagem "instancia anterior derrubada" saia mesmo com o processo
    # velho vivo, que e exatamente o bug que este helper existe pra evitar.
    # Mata TODOS primeiro, espera, e SO ENTAO conta: o `Stop-Process` volta antes de o Windows ter
    # derrubado o processo, entao perguntar `Get-Process` na linha seguinte via o processo ainda
    # vivo e contava 0. Medido 5 de 5 em 08/08/2026: o kill funcionava e o contador dizia que nao,
    # sumindo com a nota de "front derrubado". O comentario original desta contagem fala em nao
    # reportar sucesso falso — ela vinha errando pro lado oposto, escondendo sucesso real.
    foreach ($p in $todos) { Stop-Process -Id $p -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 800
    $mortos = 0
    foreach ($p in $todos) {
        if (-not (Get-Process -Id $p -ErrorAction SilentlyContinue)) { $mortos++ }
    }
    # A porta ficou LIVRE? E o que realmente importa - processo morto com a porta ainda presa
    # (outro dono) faz a instancia nova falhar do mesmo jeito. -State Listen e obrigatorio: sem ele
    # um socket em TIME_WAIT do processo recem-morto ainda conta como ocupada.
    if ($Porta -gt 0) {
        $preso = Get-NetTCPConnection -State Listen -LocalPort $Porta -ErrorAction SilentlyContinue
        if ($preso) { Falta "porta $Porta continua ocupada (pid $($preso.OwningProcess -join ', ')) apos parar $Nome" }
    }
    return $mortos
}

if ($precisa) {
    # Exit code de CADA etapa, e nao roda-e-assume: o comentario abaixo prometia que a marca so era
    # gravada depois do build dar certo, mas nada CONFERIA o resultado - `npm ci` e `npm run build`
    # iam sem checagem. Medido em producao (post-merge de 30/07 07:37): o npm ci nao populou o
    # node_modules\.bin, o build morreu com 'vite' nao e reconhecido, e o passo gravou a marca e
    # imprimiu 'ok buildado' do mesmo jeito. Pior que o falso ok: a marca ENVENENA o cache (fica
    # igual ao HEAD), entao toda rodada seguinte PULA o build e o dist velho fica pra sempre - o
    # dist servido era de 21h antes, sem as mudancas de aparencia que ja estavam no repo.
    # Nativo() em vez de chamada crua: com ErrorActionPreference='Stop', um aviso qualquer do npm no
    # stderr derrubaria o instalador inteiro (ver o docstring dele).
    # Chamada LITERAL, e NAO pelo Nativo: no Windows `npm` resolve PRIMEIRO pro shim npm.ps1
    # (ExternalScript, antes do npm.cmd), e o Nativo passa argumento por SPLATTING
    # (`@($args[1..])`) -- o npm.ps1 monta o $NPM_ARGS dele indexando $args e estoura
    # IndexOutOfRangeException na linha 47 dele. Medido: foi exatamente assim que este passo
    # ABORTOU a instalacao inteira. O Nativo existe pra programa NATIVO; npm no Windows nao e um.
    # O preference vira 'Continue' so aqui, pelo mesmo motivo do Nativo: um aviso do npm no stderr
    # nao pode virar erro terminante. A saida do npm fica VISIVEL de proposito -- foi ela que
    # denunciou o "'vite' nao e reconhecido" que este conserto passou a tratar.

    # DERRUBA O FRONT ANTES DO `npm ci`, e isto nao e zelo: o `npm ci` APAGA o node_modules antes de
    # reinstalar, e o Windows nao deixa apagar binario nativo mapeado em processo vivo. Medido em
    # 08/08/2026 nesta maquina: com o Vite de pe, o npm ci apagou quase tudo e morreu em
    # `EPERM: operation not permitted, unlink ...\@rolldown\binding-win32-x64-msvc\
    # rolldown-binding.win32-x64-msvc.node` (errno -4048), deixando node_modules pela metade e SEM o
    # `.bin` inteiro. Ninguem viu na hora, porque o Vite seguiu rodando da imagem ja em memoria; o
    # estrago so apareceu quando a maquina suspendeu e o processo morreu — dali em diante nada
    # reergueu o front, e como o `tailscale serve` daquela instalacao publica a raiz no 5173, o
    # celular passou a ver 502 em TUDO, com o backend vivo o tempo inteiro.
    # Ou seja: o instalador se auto-sabotava, e o sintoma aparecia horas depois, longe da causa.
    # Por PADRAO, sem a porta, e de proposito: o criterio de porta mata QUEM ESTIVER na 5173, e aqui
    # a pergunta nao e "quem ocupa a porta" (isso so importa no passo 7, pra instancia nova subir) —
    # e "quem segura os arquivos DESTE checkout". Passando a porta, um Vite de outro projeto do
    # usuario morreria num `install.ps1` que ele rodou por outro motivo.
    $mortosFront = Pare-Servico -Nome 'frontend (antes do npm ci)' -Porta 0 `
                                -Padrao ([regex]::Escape("$raiz\frontend")) -Exe 'node|npm|vite'
    if ($mortosFront -gt 0) { Nota "front derrubado antes do npm ci ($mortosFront processo(s))" }
    $tBuild = Get-Date
    $eapAnterior = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    Push-Location "$raiz\frontend"
    try {
        # Sem --silent no -Update: e o modo que o BOTAO Atualizar do app usa, e a caixinha da tela
        # mostra esta saida ao vivo. Com --silent o npm nao imprime NADA, entao durante o minuto de
        # `npm ci` a tela ficava sem barra andando e sem log novo — identica a uma travada, que foi
        # exatamente a leitura de quem estava olhando (25/08/2026). No modo interativo o --silent
        # continua, que e onde ele foi posto pra nao poluir o terminal de quem instala.
        # Atribuicao DIRETA, nunca `$x = if (...) { @('--silent') }`: o valor sai do bloco pelo
        # pipeline, que desembrulha o array de um elemento, e $quieto virava a STRING '--silent'.
        # Splat de string enumera CARACTERE (`-` `-` `s` `i` `l` `e` `n` `t`) — medido na VM
        # Windows: o npm recebia `-` como nome de script (`Missing script: "-"`), o `npm ci` saia
        # ruidoso, e antes disso as sobras caiam no `vite build` (`Unused args: 'l','e','n','t'`).
        $quieto = @()
        if (-not $Update) { $quieto = @('--silent') }
        npm ci @quieto
        $rcCi = $LASTEXITCODE
        if ($rcCi -eq 0) {
            # A flag vai ANTES do nome do script: no npm 11 `npm run build --silent` nao e mais
            # consumida pelo npm, ela e repassada ao script e chega no `vite build`, que morre com
            # CACError (medido na VM Windows, node 24.15 / npm 11).
            npm run @quieto build
            $rcBuild = $LASTEXITCODE
        } else {
            $rcBuild = -1
        }
    } finally {
        Pop-Location
        $ErrorActionPreference = $eapAnterior
    }
    # EVIDENCIA POSITIVA, nao ausencia de erro: exit 0 e necessario mas nao basta (build pode sair 0
    # sem escrever nada). Exige que o index.html do dist tenha nascido DEPOIS do inicio do build.
    $distNovo = (Test-Path $dist) -and ((Get-Item $dist).LastWriteTime -ge $tBuild)
    if ($rcCi -ne 0) {
        Erro "npm ci falhou (exit $rcCi) - frontend NAO buildado"
        Nota 'rodar na mao:  cd frontend ; npm ci ; npm run build'
        $script:pendencias += 'frontend'
    } elseif ($rcBuild -ne 0) {
        Erro "npm run build falhou (exit $rcBuild) - dist NAO atualizado"
        Nota 'rodar na mao:  cd frontend ; npm run build'
        $script:pendencias += 'frontend'
    } elseif (-not $distNovo) {
        # Saiu 0 mas nao produziu arquivo: o caso que a checagem por exit code sozinha deixa passar.
        Erro 'npm run build saiu 0 mas o dist\index.html nao foi reescrito - build NAO confiavel'
        Nota 'conferir na mao:  cd frontend ; npm run build'
        $script:pendencias += 'frontend'
    } else {
        # A marca so e gravada com o build VERIFICADO: marca de build que falhou faz a proxima
        # rodada pular um dist quebrado, que e exatamente o estrago descrito acima.
        # Escrever-Texto (sem BOM) em vez de `Set-Content -Encoding UTF8`, que poe BOM no 5.1 e
        # nao poe no 7: a marca e comparada com o que o git devolve, e arquivo que muda de bytes
        # conforme quem rodou o instalador e pegadinha esperando acontecer.
        if ($marca) { Escrever-Texto $marcaArq $marca }
        Ok 'buildado em frontend\dist\'
    }
} else {
    Ok 'frontend ja buildado e atualizado (nada mudou no git desde o ultimo build)'
}

# -- Janela nativa (Electron, shell\) -----------------------------------------
# So as DEPENDENCIAS, nunca o `npm run dist`. O `git pull` traz o `main.cjs` novo, e quem roda o
# app a partir do repo ja o executa no proximo start — mas se o `package-lock.json` do shell mudar
# (Electron novo, dependencia nova), a janela roda com dependencia velha e nada avisa. Empacotar
# (NSIS/AppImage) e outra coisa: leva minutos e produz um INSTALADOR, que alguem ainda tem que
# instalar — publicacao, nao atualizacao, e nao cabe num botao que roda sozinho.
$shellDir = "$raiz\shell"
if (Test-Path "$shellDir\package.json") {
    # Compara com `node_modules\.package-lock.json`, que o npm reescreve a CADA instalacao — e nao
    # com a PASTA node_modules, cuja data nao acompanha o que aconteceu dentro dela (medido: pasta
    # de 16/08 com lock de 22/08, o que faria o `npm ci` rodar em toda atualizacao, a toa).
    $marcaShell = "$shellDir\node_modules\.package-lock.json"
    $lock = "$shellDir\package-lock.json"
    $precisaShell = (-not (Test-Path $marcaShell)) -or
                    ((Test-Path $lock) -and ((Get-Item $lock).LastWriteTime -gt (Get-Item $marcaShell).LastWriteTime))
    if ($precisaShell) {
        Titulo 'Janela nativa (Electron)'
        $eapAnterior = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        Push-Location $shellDir
        try {
            # Mesmo motivo do $quieto acima: o `if` desembrulha o array e o splat vira caractere.
            $quietoShell = @()
            if (-not $Update) { $quietoShell = @('--silent') }
            npm ci @quietoShell
            $rcShell = $LASTEXITCODE
        } finally {
            Pop-Location
            $ErrorActionPreference = $eapAnterior
        }
        if ($rcShell -eq 0) {
            Ok 'dependencias da janela instaladas'
        } else {
            # Nao derruba a atualizacao: o app funciona no navegador sem a janela nativa.
            Falta "npm ci do shell\ falhou (exit $rcShell) - a janela nativa pode nao abrir"
            Nota 'rodar na mao:  cd shell ; npm ci'
            # Marca canonica (nao traduzida, nao colorida): e como o motor da atualizacao sabe que
            # algo ficou pra tras sem o instalador precisar falhar inteiro. Sem ela, a tela dizia
            # "Atualizado" com a janela nativa quebrada, e a unica pista era uma linha amarela
            # perdida no log.
            Write-Host '##HANGAR-AVISO## a janela nativa (Electron) ficou com dependencias desatualizadas'
        }
    } else {
        Ok 'janela nativa ja com as dependencias em dia'
    }
}

# -- 5/8 Wrapper do claude ---------------------------------------------------
# Sem ele um `claude` que VOCE abre no terminal e invisivel pro app: nao tem --session-id (o
# backend nao sabe qual transcript e daquela sessao) e nao vive num pane (nao ha estado nem
# input). Sessao criada PELO app funciona de qualquer jeito; isto e sobre a outra direcao.
Titulo '5/8 Wrapper do claude (sessao aberta por voce aparece no app)'
$marca = '# >>> hangar >>>'
$marcaFim = '# <<< hangar <<<'

# Marcadores de blocos NOSSOS que ficaram pra tras em instalacoes antigas. Sao os dois nomes que
# este projeto ja teve; qualquer outro marcador no perfil da pessoa NAO entra nesta lista e nao e
# tocado. Nao e faxina: hoje o bloco legado tambem faz `. claude.ps1`, entao um perfil com os dois
# carrega o wrapper DUAS vezes a cada terminal novo.
$MarcasLegadas = @(
    @{ Ini = '# >>> claude-cockpit >>>'; Fim = '# <<< claude-cockpit <<<' },
    @{ Ini = '# >>> claude-pocket >>>';  Fim = '# <<< claude-pocket <<<'  }
)

function Instalar-Bloco-No-Perfil($perfil) {
    <#
      Poe (ou atualiza) o bloco do wrapper NUM perfil. Devolve um texto curto pro log.

      MESMA FORMA do scripts/setup-windows-tmux.ps1, de proposito: regex em modo singleline com os
      marcadores escapados, remove TODAS as ocorrencias (nossas e as legadas conhecidas) e reescreve
      uma so. Dois jeitos diferentes de fazer isto no mesmo repo seria pior que qualquer um dos dois.
      O que esta FORA dos nossos marcadores nao e tocado — o perfil e da pessoa e pode ter meia vida
      de configuracao ali.

      O encoding e UTF-8 COM BOM, e essa e a unica escolha que funciona nas DUAS versoes. Medido
      aqui em 22/08/2026 com um caminho de repo contendo acento (C:\...\Joao com til), fazendo cada
      PowerShell carregar o mesmo perfil:

        perfil gravado como   PowerShell 5.1        PowerShell 7.6.5
        ANSI (cp1252)         carrega               FALHA (caminho vira "Jo?o")
        UTF-8 SEM BOM         FALHA                 carrega
        UTF-8 COM BOM         carrega               carrega

      E o que o instalador fazia antes era exatamente o pior caso: `Add-Content`/`Set-Content` sem
      -Encoding gravam ANSI no 5.1 e UTF-8 sem BOM no 7 — cada versao escrevia o formato que a
      OUTRA nao le. Aqui o BOM e desejado; no .env e no settings.json ele e veneno (install.ps1:241).
    #>
    $texto = Ler-Texto $perfil
    if ($null -eq $texto) { $texto = '' }
    $bloco = @($marca,
               ". `"$raiz\scripts\shell\claude.ps1`"",
               ". `"$raiz\scripts\shell\claude-conta.ps1`"",
               $marcaFim) -join "`r`n"

    # Marca de abertura SEM a de fechamento: arquivo mexido na mao. Nao adivinha onde o bloco
    # termina — o regex abaixo tambem nao casaria, e ai o bloco novo entraria embaixo do meio-bloco
    # velho. Dizer isso e melhor que reescrever o perfil de alguem por palpite.
    if ($texto.Contains($marca) -and -not $texto.Contains($marcaFim)) {
        return 'MEXIDO NA MAO (marca de fim ausente) - nao toquei'
    }

    $padrao = '(?s)' + [regex]::Escape($marca) + '.*?' + [regex]::Escape($marcaFim) + '\r?\n?'
    $nossos = ([regex]::Matches($texto, $padrao)).Count
    $limpo = [regex]::Replace($texto, $padrao, '')

    $legados = 0
    foreach ($m in $MarcasLegadas) {
        $pl = '(?s)' + [regex]::Escape($m.Ini) + '.*?' + [regex]::Escape($m.Fim) + '\r?\n?'
        $legados += ([regex]::Matches($limpo, $pl)).Count
        $limpo = [regex]::Replace($limpo, $pl, '')
    }

    # Cauda normalizada antes de concatenar (mesma nota do setup-windows-tmux): depois de arrancar
    # os blocos o texto ja pode terminar em quebra de linha, e somar outra deixaria linha em branco
    # acumulando a cada execucao — que e como se descobre que a funcao nao e idempotente.
    $limpo = $limpo.TrimEnd("`r", "`n")
    $novoTexto = if ($limpo) { $limpo + "`r`n`r`n" + $bloco + "`r`n" } else { $bloco + "`r`n" }

    # Reescreve mesmo quando o CONTEUDO ja esta certo: o arquivo pode estar em ANSI ou em UTF-8 sem
    # BOM (escrito por uma versao anterior deste instalador, ou pela outra versao do PowerShell), e
    # ai o bloco existe mas o terminal nao consegue LER o caminho.
    Escrever-Texto $perfil $novoTexto -ComBom

    if ($legados -gt 0) { return "bloco no lugar; $legados bloco(s) legado(s) colapsado(s)" }
    if ($nossos -gt 1)  { return "bloco no lugar; $nossos copias colapsadas em 1" }
    if ($nossos -eq 1)  { return 'ja presente (encoding normalizado)' }
    return 'bloco adicionado'
}


$perfis = Perfis-Do-Usuario
$jaTem = $false
foreach ($pf in $perfis) {
    if ((Test-Path $pf) -and ((Ler-Texto $pf) -match [regex]::Escape($marca))) { $jaTem = $true }
}
if ($jaTem -or (Pergunte '  Instalar (recomendado)?')) {
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
        # TODOS os perfis, nao so o da versao que esta rodando: instalar pelo pwsh 7 deixava o
        # terminal 5.1 — o padrao do Windows, e o que o proprio app usa — sem o wrapper, calado.
        foreach ($pf in $perfis) {
            $r = Instalar-Bloco-No-Perfil $pf
            if ($r -like 'MEXIDO*') { Falta "$pf : $r" } else { Ok "$pf : $r" }
        }
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
# Delegado pro scripts/setup-windows-tmux.ps1 - este trecho ESCREVIA o bloco aqui mesmo, e os dois
# brigavam pelo mesmo marcador: o script novo apagava o bloco legado e o -Update seguinte reescrevia.
# Alem disso o codigo daqui era o AUTOR das duplicatas (15 blocos na maquina de referencia): ele
# procurava o marcador com `$atual -contains $ini`, match EXATO de linha, e gravava com
# `Set-Content -Encoding UTF8`, que no PS 5.1 poe BOM. O BOM gruda na 1a linha, o -contains passa a
# falhar nela e o ramo de substituicao vira ramo de APPEND - mais um bloco a cada execucao.
# O script delegado nao tem nenhum dos dois problemas: substitui entre marcadores, grava UTF8 SEM
# BOM, respeita a precedencia do psmux (.psmux.conf > .psmuxrc > .tmux.conf > .config\psmux\) em vez
# de escrever cego no ~/.tmux.conf, e ainda diagnostica o ambiente (tmux e mesmo o psmux? e Windows
# Terminal?) e confere o resultado lendo do servidor. Ele tambem colapsa os blocos legados que este
# trecho deixou pra tras. Ver docs/tmux.conf.windows.example.
$setupTmux = "$raiz\scripts\setup-windows-tmux.ps1"
if (-not (Test-Path $setupTmux)) {
    Falta "setup-windows-tmux.ps1 nao encontrado - config do multiplexador nao aplicada"
} else {
    # -SkipInstall: o psmux ja foi garantido no passo de dependencias la em cima.
    & $setupTmux -Apply -SkipInstall
    if ($LASTEXITCODE -eq 0) {
        Ok 'config do multiplexador aplicada (scroll do mouse, cores, barra escondida)'
        Nota 'Vale nas sessoes NOVAS - a config e lida na criacao da sessao.'
    } else {
        # O script sai != 0 quando o AMBIENTE tem problema que a config nao conserta (tmux que nao e
        # o psmux, terminal que nao e o WT). Nao aborta a instalacao: o resto do app funciona.
        Falta 'config do multiplexador aplicada COM RESSALVAS - veja o diagnostico acima'
    }
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
            # Ler-Texto e nao `Get-Content -Raw`: sem BOM o Get-Content assume ANSI no 5.1 e
            # UTF-8 no 7, e como este bloco REESCREVE o arquivo inteiro, o chute errado corrompia
            # todo acento que ja estava la (mesma historia do token no Set-EnvKey).
            $bruto = Ler-Texto $settingsClaude
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
            # SEM BOM, e isto nao e preferencia: `Set-Content -Encoding UTF8` poe BOM no 5.1 e
            # nao poe no 7 (medido), e quem le este arquivo e o Claude Code, em Node — medido
            # aqui que `JSON.parse` de um arquivo com BOM levanta
            # "Unexpected token, is not valid JSON". Ou seja, instalar pelo 5.1 podia deixar o
            # settings.json do Claude ilegivel pra ele.
            Escrever-Texto $settingsClaude ($cfg | ConvertTo-Json -Depth 20)
            Ok 'statusline configurada no ~/.claude/settings.json'
            Nota 'Vale nas sessoes NOVAS do Claude Code.'
            # Mesmo aviso do Linux: o caminho do node fica CRAVADO no settings. Trocar de versao
            # de node quebra a statusline em silencio - o app volta a dizer "medicao indisponivel".
            Nota 'Se voce trocar a versao do node, rode este instalador de novo.'
        }
    }
}

# -- 5d/8 Publicar o backend no Tailscale -------------------------------------
# FORA do passo 6 de proposito: aquele e pulado inteiro no -Update (install.ps1:564), e -Update e o
# caminho do hook post-merge, ou seja como as maquinas ja instaladas se atualizam. Uma migracao que
# so roda na instalacao interativa nunca alcanca quem precisa dela.
Titulo '5d/8 Publicar o backend no Tailscale'
$script:cpPublicUrl = $null
$portaBack = Porta-Do-Env 'CP_PORT' 8765
# Em FUNCAO, nao inline: o passo 6 chama de novo depois de instalar/logar o Tailscale, pra que
# quem instalou por aqui nao termine com o QR em 127.0.0.1 e precise lembrar de re-rodar tudo.
# Le $portaBack/$envFile do escopo do script e grava $script:cpPublicUrl - mesmo contrato de antes.
function Publica-Tailscale {
    if (-not (Tem 'tailscale')) {
        Nota 'tailscale nao instalado - pulando (o acesso de fora fica por sua conta)'
    } else {
        $eapAnt = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'   # chamada nativa: stderr nao pode virar excecao
        try {
            $tsJson = $null
            # Com timeout: este passo roda tambem DESATENDIDO (-Update, via o hook post-merge - ver o
            # comentario do passo 5d) e um tailscaled travado nao pode pendurar um `git pull` pra
            # sempre. Achado MINOR da revisao final.
            $jobTs = Start-Job -ScriptBlock { & tailscale status --json 2>$null }
            if (Wait-Job $jobTs -Timeout 5) {
                try { $tsJson = (Receive-Job $jobTs | Out-String | ConvertFrom-Json) } catch { }
            } else {
                Stop-Job $jobTs -ErrorAction SilentlyContinue
                Nota 'tailscale status nao respondeu em 5s - pulando deteccao'
            }
            Remove-Job $jobTs -Force -ErrorAction SilentlyContinue
            $dns = $null
            if ($tsJson -and $tsJson.Self -and $tsJson.Self.DNSName) { $dns = $tsJson.Self.DNSName.TrimEnd('.') }
            if (-not $dns) {
                Nota 'tailscale sem nome de no (nao logado?) - rode `tailscale up` e re-rode este instalador'
            } else {
                # FILTRA por :443. O `Web` do serve status e um mapa "host:porta" -> Handlers, e esta
                # maquina pode ter OUTRO slot publicado — o preview de projeto usa o 10000
                # (backend/app/tunnel.py:11). Varrer todos e guardar o ultimo `/` faria o proxy do 10000
                # passar por "ja publica o backend", e o 443 nunca ser configurado. O mesmo filtro existe
                # em tunnel.py:71-73, e e por isso que ele existe la.
                $proxy443 = $null
                try {
                    $sv = (& tailscale serve status --json 2>$null | Out-String | ConvertFrom-Json)
                    if ($sv -and $sv.Web) {
                        foreach ($p in $sv.Web.PSObject.Properties) {
                            if ($p.Name -notmatch ':443$') { continue }
                            foreach ($h in $p.Value.Handlers.PSObject.Properties) {
                                if ($h.Name -eq '/') { $proxy443 = $h.Value.Proxy }
                            }
                        }
                    }
                } catch { }
                # Compara PORTA, nao string: o tailscale normaliza o alvo, entao "localhost:8765" e
                # "http://127.0.0.1:8765" descrevem a mesma coisa e uma comparacao literal diria que
                # precisa reconfigurar a cada rodada. Mesmo criterio do tunnel._port_from_proxy.
                $portaAtual = $null
                if ($proxy443 -match ':(\d+)/?$') { $portaAtual = [int]$Matches[1] }
                if ($portaAtual -eq $portaBack) {
                    Ok "tailscale ja publica o backend (porta $portaBack)"
                    $script:cpPublicUrl = "https://$dns"
                } elseif ($proxy443 -and $portaAtual -ne 5173) {
                    # Handler que NAO fomos nos que criamos: avisa e nao toca. E NUNCA `serve reset`,
                    # que derrubaria o slot do preview de projeto e o que o dono tenha feito a mao.
                    Falta "tailscale ja publica '$proxy443' na raiz - NAO vou sobrescrever; ajuste na mao se quiser o backend ali"
                } else {
                    $saida = (& tailscale serve --bg --https=443 "localhost:$portaBack" 2>&1 | Out-String)
                    if ($LASTEXITCODE -eq 0) {
                        Ok "tailscale publicando o backend (localhost:$portaBack)"
                        $script:cpPublicUrl = "https://$dns"
                    } else {
                        # A saida diz a causa real (permissao, HTTPS nao habilitado no tailnet); sem ela
                        # sobraria chutar numa lista de tres. Se for permissao, o caminho e o mesmo que o
                        # bloco de firewall ja ensina: abrir um PowerShell como Administrador.
                        Falta "tailscale serve falhou: $($saida.Trim())"
                        Nota 'Se falou em permissao/acesso negado: abra um PowerShell como Administrador e rode este instalador de novo.'
                    }
                }
                if ($script:cpPublicUrl) {
                    # E isto que conserta o QR do BACKEND tambem: com public_url preenchido, pairing_url
                    # (backend/app/config.py:211) ignora porta e bind e usa este endereco.
                    #
                    # So GRAVA se o valor mudou. Sem este check, a rodada idempotente (linha "tailscale
                    # ja publica") reescrevia o .env e dizia "gravado" toda vez, mesmo sem nada ter
                    # mudado - sugeria uma escrita que nao aconteceu. Mesmo padrao Select-String -Quiet
                    # de $temToken acima.
                    $jaTinhaEsseValor = (Test-Path $envFile) -and (Select-String -Path $envFile `
                        -Pattern "^CP_PUBLIC_URL=$([regex]::Escape($script:cpPublicUrl))\s*$" -Quiet)
                    if ($jaTinhaEsseValor) {
                        Ok "CP_PUBLIC_URL=$($script:cpPublicUrl) ja registrado em backend\.env"
                    } else {
                        Set-EnvKey -Chave 'CP_PUBLIC_URL' -Valor $script:cpPublicUrl
                        Ok "CP_PUBLIC_URL=$($script:cpPublicUrl) gravado em backend\.env"
                    }
                }
            }
        } finally { $ErrorActionPreference = $eapAnt }
    }
}
Publica-Tailscale

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
    Get-NetFirewallRule -DisplayName "hangar $_" -ErrorAction SilentlyContinue
}
if ($regras.Count -eq 2) {
    Ok 'portas 8765 e 5173 ja liberadas no firewall'
} elseif (Pergunte '  Liberar as portas 8765 e 5173 no firewall pra rede LOCAL?') {
    if (EhAdmin) {
        foreach ($p in 8765, 5173) {
            $nome = "hangar $p"
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
        Nota 'New-NetFirewallRule -DisplayName "hangar 8765" -Direction Inbound -Action Allow -Protocol TCP -LocalPort 8765 -Profile Private'
        Nota 'New-NetFirewallRule -DisplayName "hangar 5173" -Direction Inbound -Action Allow -Protocol TCP -LocalPort 5173 -Profile Private'
    }
}

# So aqui, nunca no bloco 5b: esta pergunta e do dono, e o -Update roda sozinho pelo post-merge.
if (-not $script:cpPublicUrl) {
    Write-Host '  Sem Tailscale configurado. De onde voce vai usar?'
    Write-Host '    [1] So nesta maquina  - o app de desktop. Sem QR (nao ha o que ler do celular).'
    Write-Host '    [2] Rede de casa      - celular no mesmo Wi-Fi.'
    # -Sim honra o padrao (opcao 1, so nesta maquina) sem perguntar - achado da revisao final:
    # `install.ps1 -Sim` (modo documentado no cabecalho) ficava parado esperando tecla aqui, porque
    # este Read-Host era cru e nao olhava pra $Sim como o resto do arquivo (via `Pergunte`).
    $escolha = if ($Sim) { '1' } else { Read-Host '  1 ou 2 (Enter = 1)' }
    if ($escolha -eq '2') {
        # 0.0.0.0, NUNCA 'auto': resolve_bind_ip (backend/app/config.py:199-201) troca 'auto' pelo
        # IP de LAN detectado e SO, entao o uvicorn passa a escutar SO naquela interface - o
        # `tailscale serve` que o passo 5d publica em cima de "localhost:$portaBack" levaria recusa
        # de conexao (502 no celular), porque o loopback deixaria de responder. '0.0.0.0' e o UNICO
        # valor onde o loopback continua valendo (escuta em TODAS as interfaces, 127.0.0.1 inclusa)
        # - mesmo comentario em backend/app/pi_inbox.py:184, achado CRITICO da revisao final.
        Set-EnvKey -Chave 'CP_LAN_BIND_IP' -Valor '0.0.0.0'
        Ok 'CP_LAN_BIND_IP=0.0.0.0 gravado - o backend passa a escutar em todas as interfaces (LAN inclusa)'
        # O IP entra no CP_PUBLIC_URL porque, sem ele, pairing_url monta a URL sobre o front_port
        # (config.py:215) = 5173, e o Vite escuta so em loopback: o QR sairia apontando pra uma porta
        # onde nada responde na LAN. Com a chave gravada, o curto-circuito de config.py:211 usa este
        # endereco e o QR passa a valer.
        # NAO pegar -First 1 da lista crua de Get-NetIPAddress: a ordem ali e enumeracao interna do
        # Windows, nao prioridade de rota, e uma maquina com Docker Desktop/WSL/Hyper-V/VPN (perfil
        # comum de quem instala isto) tem um vEthernet que passa nos mesmos filtros (DHCP interno,
        # nao e 127./169.254., nao e WellKnown) e costuma vir ANTES da Wi-Fi real. Gravaria um IP
        # interno tipo 172.x com "gravado" na tela - o QR morto que esta task existe pra evitar, so
        # que pelo IP em vez da porta. A interface certa e a da rota padrao (0.0.0.0/0): essa e a
        # que de fato sai pra rede.
        $idxRota = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue |
                   Sort-Object RouteMetric | Select-Object -First 1 -ExpandProperty InterfaceIndex
        $ipLan = $null
        if ($idxRota) {
            $ipLan = (Get-NetIPAddress -AddressFamily IPv4 -InterfaceIndex $idxRota -ErrorAction SilentlyContinue |
                      Where-Object { $_.IPAddress -notmatch '^(127\.|169\.254\.)' } |
                      Select-Object -First 1 -ExpandProperty IPAddress)
        }
        if ($ipLan) {
            $novaUrl = "http://${ipLan}:$portaBack"
            # So GRAVA se o valor mudou - mesmo padrao Select-String -Quiet do bloco Tailscale
            # acima (linha ~673): sem isto, "gravado" apareceria toda vez, mesmo reescrevendo o
            # .env com o valor que ja estava la.
            $jaTinhaEsseValor = (Test-Path $envFile) -and (Select-String -Path $envFile `
                -Pattern "^CP_PUBLIC_URL=$([regex]::Escape($novaUrl))\s*$" -Quiet)
            if ($jaTinhaEsseValor) {
                Ok "CP_PUBLIC_URL=$novaUrl ja registrado em backend\.env"
            } else {
                Set-EnvKey -Chave 'CP_PUBLIC_URL' -Valor $novaUrl
                Ok "CP_PUBLIC_URL=$novaUrl gravado em backend\.env"
            }
            $script:cpPublicUrl = $novaUrl
            Nota 'ATENCAO: esse endereco e o IP que o seu roteador deu a esta maquina, e ele PODE MUDAR'
            Nota '(reinicio do roteador, DHCP renovando). Quando mudar, o QR e o link param de funcionar:'
            Nota 'rode este instalador de novo pra regravar o endereco novo.'
        } else {
            Falta 'nao achei um IP de rede local - o QR fica sem endereco valido; use Tailscale ou fixe o IP'
        }
        Nota 'O token do .env vira a UNICA tranca: quem estiver no Wi-Fi e souber o token roda comando como voce.'
    } else {
        Ok 'ficando so em 127.0.0.1'
    }
}

# Login + publicacao na MESMA rodada. `tailscale up` e INTERATIVO (imprime um link, abre o
# navegador e bloqueia ate a pessoa autenticar) - nao ha como automatizar, so esperar. Por isso
# e pergunta, e no -Sim nao roda (desatendido nao pode ficar parado esperando login). Teto de 5 min
# pra um login abandonado nao segurar o instalador pra sempre. Depois chama o passo 5d de novo
# (Publica-Tailscale), que grava CP_PUBLIC_URL e deixa o QR do final ja com o endereco do tailnet.
# Antes disto, quem instalava o Tailscale por aqui terminava com o QR em 127.0.0.1 e precisava
# lembrar de rodar o instalador inteiro de novo.
function Loga-E-Publica-Tailscale {
    if ($Sim) {
        Nota 'Falta logar: rode `tailscale up` e depois este instalador de novo (o passo 5d grava o endereco sozinho).'
        return
    }
    if (-not (Pergunte '  Logar no Tailscale agora? (abre o navegador; o instalador espera voce autenticar)')) {
        Nota 'Depois: `tailscale up` e este instalador de novo - o passo 5d grava CP_PUBLIC_URL sozinho.'
        return
    }
    $eapAnt = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'   # nativo: o link de login sai no stderr
    try {
        # Em job, pelo timeout - e a saida e mostrada na hora em que chega, porque o link de login
        # e o que a pessoa precisa ver ANTES de o comando terminar.
        $job = Start-Job -ScriptBlock { & tailscale up 2>&1 }
        $fim = (Get-Date).AddMinutes(5)
        while ($job.State -eq 'Running' -and (Get-Date) -lt $fim) {
            Receive-Job $job | ForEach-Object { Nota "  $_" }
            Start-Sleep -Milliseconds 500
        }
        Receive-Job $job | ForEach-Object { Nota "  $_" }
        if ($job.State -eq 'Running') {
            Stop-Job $job -ErrorAction SilentlyContinue
            Falta 'tailscale up nao concluiu em 5 min - termine o login e rode este instalador de novo'
        }
        Remove-Job $job -Force -ErrorAction SilentlyContinue
    } finally { $ErrorActionPreference = $eapAnt }
    # O proprio 5d diz "sem nome de no" se o login nao aconteceu; nao ha o que checar antes.
    Publica-Tailscale
    if (-not $script:cpPublicUrl) {
        Nota 'Ainda sem endereco do tailnet - depois de logar, rode este instalador de novo.'
    }
}

if (Tem 'tailscale') {
    Ok 'Tailscale ja instalado'
    if (-not $script:cpPublicUrl) {
        Nota 'CP_PUBLIC_URL nao gravado ainda (veja o passo 5d acima) - provavelmente falta `tailscale up`.'
        Loga-E-Publica-Tailscale
    }
} elseif (Pergunte '  Instalar o Tailscale? (VPN pessoal - acesso de fora de casa)') {
    # Id com MAIUSCULAS: o `--exact` do winget diferencia caixa, e 'tailscale.tailscale' nao casa
    # nada. Medido: os outros seis ids do instalador estavam certos, so este errado.
    if (Instale 'Tailscale' 'tailscale' 'Tailscale.Tailscale' 'acesso remoto') {
        Nota 'Instale o Tailscale tambem no celular (mesma conta).'
        Loga-E-Publica-Tailscale
    }
}

}

# -- 7/8 Subir sozinho no logon ----------------------------------------------
# Equivalente possivel dos servicos systemd do Linux. Nao e servico do Windows (isso exigiria
# admin e rodaria fora da sua sessao, sem acesso ao seu ~\.claude): e tarefa agendada no logon.
Titulo '7/8 Subir junto com o Windows'
# Portas: o Pare-Servico abaixo precisa saber QUEM segurar pra derrubar, e matar por porta ERRADA
# derruba processo alheio. As duas saem do .env (mesma fonte que o backend usa), com o default do
# config.py como piso. A do FRONT estava cravada em 5173 enquanto a do back era lida do arquivo -
# incoerencia que custava caro: quem tivesse outro Vite em 5173 via o processo dele morrer.
$portaBack  = Porta-Do-Env 'CP_PORT' 8765
# O FRONT nao sai do .env. Tentei ler CP_FRONT_PORT por simetria com o backend e estava ERRADO:
# `front_port` (config.py:95) e "where the PWA is served ... used for QR pairing" - ele monta a URL
# do QR (config.py:215), NAO configura o Vite. O dev server sobe sempre em 5173 (vite.config.ts:
# server sem `port`, logo o default; e preview.port fixo em 5173 com strictPort). Quem puser
# CP_FRONT_PORT=8080 (ex: atras de um Caddy) faria o instalador mirar a 8080 e passar longe do Vite
# real - ou seja, nao derrubaria a instancia velha, que e a razao de existir do Pare-Servico.
# A fonte da verdade e o vite.config.ts; se ele mudar, muda aqui junto.
$portaFront = 5173

$tarefas = @(
    # Padrao ANCORADO no caminho deste checkout. 'app\.main' cru casava com QUALQUER processo cuja
    # linha de comando contivesse app.main - e este repo tem worktrees em .claude/worktrees/ com
    # checkout completo, entao o instalador do checkout principal matava o backend de uma worktree
    # rodando em outra porta. O ramo do frontend ja fazia certo; o do backend nao.
    # ExeProc e o nome do processo VIVO, diferente de Exe (o lancador): o backend nasce `uv` e quem
    # fica segurando a porta e o `python` neto. Serve pro filtro por PADRAO do Pare-Servico — sem ele,
    # o casamento e substring pura da linha de comando, e QUALQUER processo que so MENCIONE este
    # caminho vira alvo (um editor, um grep, o terminal de onde se chamou o instalador).
    @{ Nome = 'hangar-backend';  Exe = 'uv';  Args = 'run python -m app.main'; Dir = "$raiz\backend"
       Porta = $portaBack;  Padrao = [regex]::Escape("$raiz\backend"); ExeProc = 'uv|python' },
    # `run preview`, NAO `run dev`: o passo 2 acabou de gerar o frontend\dist e subir o dev
    # server aqui serviria desenvolvimento numa instalacao de producao - a mesma incoerencia que
    # o services-setup.sh do Linux ja corrigiu. O bloco `preview` do vite.config.ts usa a MESMA
    # porta 5173 com o mesmo proxy /api, entao a origem nao muda e ninguem perde localStorage
    # (cp_servers, tema, layout). Pra mexer no layout com recarga ao vivo: pare a tarefa e rode
    # `npm run dev` na mao.
    @{ Nome = 'hangar-frontend'; Exe = 'npm'; Args = 'run preview';            Dir = "$raiz\frontend"
       Porta = $portaFront; Padrao = [regex]::Escape("$raiz\frontend"); ExeProc = 'node|npm|vite' }
)

# Derruba a instancia VELHA antes de subir a nova.
#
# Sem isto o `-Update` saia dizendo "ok" com o processo ANTIGO ainda no ar, servindo codigo
# antigo. O encadeamento: o .vbs roda `Run(..., 0, False)` - nao espera -, entao a TAREFA
# termina na largada e fica `State=Ready` mesmo com o servidor vivo e desgarrado. Nesse estado
# o `Start-ScheduledTask` nao e ignorado (a tarefa nao esta rodando): ele sobe uma SEGUNDA
# instancia, que colide na porta e morre, enquanto a velha sobrevive. Medido nesta maquina: um
# -Update deixou o backend servindo codigo de 26 minutos antes, e as correcoes ja no disco
# pareciam nao ter efeito - so valeram depois de matar os processos na mao.
# Vale pros DOIS agora: o frontend serve o build (`npm run preview`), entao mudanca em .svelte
# so aparece depois de `npm run build` + reinicio da tarefa - nao ha mais HMR pra disfarcar. O
# backend nunca teve, porque CP_RELOAD e off por padrao (config.py).
# Ja registrado -> RE-REGISTRA sem perguntar, em vez de pular. A tarefa guarda o caminho do
# executavel e o diretorio DENTRO dela; um `git pull` que mova o repo, ou um uv que mude de
# lugar, deixa a tarefa apontando pro nada - e "ja registrada" esconderia isso. Register-...
# -Force sobrescreve.
$jaAgendado = Get-ScheduledTask -TaskName $tarefas[0].Nome -ErrorAction SilentlyContinue
$registrou = $jaAgendado -or (Pergunte '  Registrar backend e frontend pra subir no seu logon?')
# As duas nascem FORA do try de proposito. `$subiu` e lido la embaixo (install.ps1:1842) pra
# decidir a pendencia 'backend no ar'; enquanto ele morava dentro do try, uma excecao no registro
# deixava a variavel INDEFINIDA -> $false -> pendencia inventada com a porta 8765 aberta e o
# backend respondendo. `$iniciou` diz se alguma tarefa chegou a ser (re)iniciada: sem isso, esperar
# 40s pela porta depois de um estouro que nem chegou no Start-ScheduledTask so mede o processo
# ORFAO que ficou de pe, e reportar "ok" ali seria pior que a pendencia falsa.
$subiu = $false
$iniciou = $false
if ($registrou) {
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
            $log = Join-Path $env:LOCALAPPDATA "hangar\$($t.Nome).log"
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $log) | Out-Null
            # -EncodedCommand (base64 UTF-16LE) em vez de -Command com aspas: o PowerShell NAO
            # escapa com barra invertida, e a string aninhada quebrava o New-ScheduledTaskAction
            # ("nao e possivel localizar um parametro posicional"). Codificado nao ha o que escapar.
            $interno = "& '$exe' $($t.Args) *>&1 | Out-File -FilePath '$log' -Encoding utf8"
            $b64 = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($interno))

            # Por que um .vbs e nao `powershell -WindowStyle Hidden` direto: esse parametro nao
            # impede a janela de EXISTIR - o console e criado e so depois escondido, e lancado pelo
            # Agendador ele fica na barra de tarefas. Medido: duas janelas abertas e paradas.
            # O wscript nao tem console proprio, e o Run(..., 0, False) inicia ja oculto e nao
            # espera. Alternativa seria rodar a tarefa "esteja o usuario logado ou nao", mas ai ela
            # cai na sessao 0 e o servidor do multiplexador nasceria fora da sessao do usuario.
            $vbs = Join-Path (Split-Path -Parent $log) "$($t.Nome).vbs"
            $linhaVbs = 'CreateObject("WScript.Shell").Run "powershell -NoProfile ' +
                        "-ExecutionPolicy Bypass -EncodedCommand $b64" + '", 0, False'
            # O .vbs carrega o caminho do LOG, que fica em %LOCALAPPDATA% — ou seja, no perfil do
            # usuario, que pode ter acento no nome. Ver Escrever-Lancador.
            Escrever-Lancador $vbs ($linhaVbs + "`r`n") 'vbs' | Out-Null
            $acao = New-ScheduledTaskAction -Execute 'wscript.exe' `
                -Argument "`"$vbs`"" -WorkingDirectory $t.Dir
            $gatilho = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
            $cfg = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries `
                        -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)
            # Register em try PROPRIO, e o unico ponto deste passo que precisa de permissao de
            # ESCRITA. O XML em C:\Windows\System32\Tasks\<nome> pertence a quem registrou: um
            # install.ps1 rodado ELEVADO deixa as tres tarefas com dono BUILTIN\Administradores, e
            # o usuario fica so com Read+Synchronize. O botao Atualizar do app roda NAO elevado,
            # entao o `-Force` volta Acesso negado. Medido em 26/08/2026: essa unica excecao pulava
            # o Pare-Servico, o Start-ScheduledTask e a checagem de porta DE UMA VEZ - o backend
            # seguia no ar com 4 commits e ~8h de atraso (o pior estado que o CLAUDE.md descreve:
            # codigo novo no disco, processo velho no ar) enquanto a tela dizia so "nao deu pra
            # registrar as tarefas".
            # Tarefa que JA existe nao precisa de registro pra ser reiniciada: parar e iniciar sao
            # permitidos sem elevacao (medido na hangar-vigia: Start-ScheduledTask OK, LastRunTime
            # avancou). Entao o registro vira RESSALVA e o restart continua. Se a tarefa NAO existe
            # nao ha o que reaproveitar - rethrow, que e o caso que o catch de fora ja cobria.
            $reaproveitou = $false
            try {
                Register-ScheduledTask -TaskName $t.Nome -Action $acao -Trigger $gatilho `
                    -Settings $cfg -Force | Out-Null
            } catch {
                if (-not (Get-ScheduledTask -TaskName $t.Nome -ErrorAction SilentlyContinue)) { throw }
                $reaproveitou = $true
                Falta "sem permissao pra re-registrar $($t.Nome) - reaproveitando a tarefa existente ($_)"
                # A ressalva importa: a tarefa guarda o caminho do .vbs e o diretorio DENTRO dela,
                # entao a que sobrou aponta pro estado de quando foi registrada. Se o repo mudou de
                # lugar (ou o uv), reaproveitar sobe o caminho ANTIGO - e isso nao da pra consertar
                # sem elevacao.
                Nota "  ela ainda aponta pro caminho de quando foi registrada; se o repo mudou de lugar, so um re-registro conserta"
                Nota "  numa janela ELEVADA:  Unregister-ScheduledTask -TaskName $($t.Nome) -Confirm:`$false"
                Nota '  e depois rode este instalador de novo SEM elevacao (ele recria a tarefa com o seu usuario como dono)'
            }
            # Registrar NAO inicia: o gatilho e "no logon", entao sem isto nada sobe ate o
            # proximo login e a pessoa abre o navegador numa porta morta logo apos instalar.
            # O equivalente no Linux (`systemctl --user enable --now`) liga na hora - o `--now`
            # e justamente esta metade, e ela tinha ficado de fora aqui.
            $mortos = Pare-Servico -Nome $t.Nome -Porta $t.Porta -Padrao $t.Padrao -Exe $t.ExeProc
            if ($mortos -gt 0) { Nota "  instancia anterior derrubada ($mortos processo(s)) antes de subir" }
            Start-ScheduledTask -TaskName $t.Nome -ErrorAction SilentlyContinue
            $iniciou = $true
            if ($reaproveitou) { Ok "tarefa $($t.Nome) reaproveitada e reiniciada" } else { Ok "tarefa $($t.Nome) registrada e iniciada" }
        }
    } catch {
        Falta "nao deu pra registrar as tarefas: $_"
        Nota 'Sem isso, o backend so roda enquanto o terminal estiver aberto.'
    }

    # Iniciar nao e subir: a tarefa ja morreu na largada por bug de codificacao, e o instalador
    # dizia "iniciada" e seguia. Confere a porta antes de afirmar qualquer coisa.
    # -State Listen e $portaBack, nao 8765 cravado. Sem o -State Listen um socket em TIME_WAIT do
    # processo que o Pare-Servico acabou de matar ja contava como "subiu", e o instalador declarava
    # sucesso apontando pro cadaver - o bug original (instancia velha) de volta, agora com uma
    # mensagem verde na frente afirmando o contrario.
    #
    # FORA do try acima: enquanto estava dentro, qualquer excecao no registro pulava esta checagem
    # inteira e `$subiu` ficava indefinido - o gate de install.ps1:1842 lia $false e somava a
    # pendencia 'backend no ar' com a porta ABERTA. Um "Acesso negado" no registro matava o restart
    # e a verificacao juntos, e a atualizacao falhava em silencio justamente no que importa.
    if ($iniciou) {
        # 40s, nao 15s: o boot medido nesta maquina e ~15s (comentario da vigia, mais abaixo), e o
        # caminho feliz sai no primeiro acerto - o teto so paga quando o boot for mais lento (disco,
        # antivirus, primeira sincronizacao do uv). Achado IMPORTANTE da revisao final: com 15s, um
        # boot de 16s virava alarme falso, entrava em $pendencias e derrubava o instalador inteiro
        # (exit 1, bloco vermelho do hook dizendo "A ATUALIZACAO NAO RODOU") quando o backend so
        # estava alguns segundos atrasado.
        foreach ($i in 1..40) {
            if (Get-NetTCPConnection -State Listen -LocalPort $portaBack -ErrorAction SilentlyContinue) { $subiu = $true; break }
            Start-Sleep -Seconds 1
        }
        if ($subiu) {
            Ok "backend respondendo em 127.0.0.1:$portaBack"
        } else {
            Falta 'o backend NAO subiu em 40s - o app nao vai conectar'
            Nota "veja o porque:  Get-Content `"$env:LOCALAPPDATA\hangar\hangar-backend.log`" -Tail 30"
        }
    } else {
        # Nao chegou nem a iniciar (o catch acima disparou antes do Start-ScheduledTask). Nao se
        # olha a porta aqui de proposito: o que estivesse escutando seria o processo ORFAO da
        # instalacao anterior, e chamar isso de "backend respondendo" e a mentira que este passo
        # inteiro existe pra nao contar. Fica sem $subiu -> a pendencia la embaixo e VERDADEIRA.
        Falta 'nenhuma tarefa chegou a ser iniciada - o que estiver na porta e a instancia ANTIGA'
    }
    Nota 'Log (inclui o QR de pareamento):'
    Nota "  $env:LOCALAPPDATA\hangar\hangar-backend.log"
    Nota 'Remover depois: Unregister-ScheduledTask -TaskName hangar-backend'

    # Vigia registrada em try/catch PROPRIO, separado do de cima: achado IMPORTANTE da revisao
    # final. Antes, os dois viviam sob o MESMO try, e uma falha AQUI (na vigia) saia com a mensagem
    # "nao deu pra registrar as tarefas: ..." mesmo com backend E frontend ja registrados e ja
    # respondendo (Ok impresso linhas acima) - e como nada disto entrava em $pendencias, o script
    # ainda fechava em "Pronto". A vigia falhar e real (menos grave que o backend nao subir, mas
    # ainda assim: sem ela, um crash so reergue no proximo logon) e tem que aparecer como o que e.
    try {
    # Vigia: a tarefa dos servicos dispara no LOGON, e suspensao mata o processo sem passar por
    # logoff/logon - nada reergue, e o dono descobre pelo 502 no celular, longe do PC (foi o que
    # aconteceu em 08/08/2026). NAO trocamos o gatilho por conta de servico: isso tiraria o backend
    # da sessao interativa, que e o que lhe da o clipboard - o caminho de envio do Windows
    # (backend/app/tmux.py, paste_via_clipboard). Consertaria o boot e quebraria o envio.
    #
    # Pelo wscript, igual as outras tarefas, e NAO por `powershell -WindowStyle Hidden`: aquele
    # parametro nao impede o console de EXISTIR (medido acima: duas janelas paradas na barra).
    # Numa tarefa que roda a cada 5 minutos, isso seria uma piscada de janela pra sempre.
    #
    # A vigia so pode disparar `Start-ScheduledTask` se NENHUM backend deste checkout ja estiver
    # subindo - sem isto ela pode criar uma SEGUNDA instancia colidindo na porta: o .vbs usa
    # Run(...,0,False), que nao espera, entao a tarefa termina na largada e fica `State=Ready`
    # muito antes do `uv run python -m app.main` abrir a porta. Um boot lento (disco, antivirus,
    # primeira sincronizacao do uv) passando dos 5 min do tick seguinte via a porta ainda fechada
    # e chama `Start-ScheduledTask` de novo.
    #
    # UNIAO dos dois criterios, nao intersecao - medido na maquina real em 09/08/2026: a arvore
    # do backend e uv.exe -> python.exe (.venv\Scripts, cita o caminho do checkout) -> python.exe
    # (interpretador do uv em AppData\Roaming\uv\..., NUNCA cita o checkout). So o uv.exe do topo
    # e o NETO que segura a porta citam `-m app.main`; so o do MEIO cita o caminho. Filtrar por
    # um so dos dois criterios perde o processo que importa (o neto que segura o socket, ou o
    # pai). E o filtro de NOME (-Name -match 'uv|python') fica pra nao contar espectador nenhum -
    # terminal/editor/grep que so MENCIONE "app.main" ou o caminho numa linha de comando alheia -
    # o mesmo cuidado que Pare-Servico ja tem com -Exe (nunca so caminho, sempre caminho + nome).
    $vigiaPadraoAppMain = [regex]::Escape('app.main')
    # .Replace("'","''") no CAMINHO e no LOG: os dois entram crus dentro de literais de aspas
    # SIMPLES do template abaixo, e os dois vem do sistema de arquivos (o segundo via
    # $env:LOCALAPPDATA) - um perfil com apostrofo no nome (C:\Users\O'Brien\...) fecharia a
    # aspa simples cedo e quebraria o script da vigia. '' e o escape de aspa simples do
    # PowerShell dentro de string de aspas simples.
    $vigiaPadraoCaminho = $tarefas[0].Padrao.Replace("'", "''")   # regex do checkout, ja escapado (acima)
    $vigiaExeProc = $tarefas[0].ExeProc        # 'uv|python'
    # O front nao tem o par de criterios do backend (nao ha um `-m app.main` equivalente): o que
    # identifica o processo dele e o caminho do checkout. Mesmo `.Replace("'","''")` do de cima,
    # pelo mesmo motivo — o caminho entra CRU dentro de um literal de aspas simples, e um perfil
    # com apostrofo (C:\Users\O'Brien\...) fecharia a aspa cedo e quebraria a vigia inteira.
    $vigiaPadraoFront = $tarefas[1].Padrao.Replace("'", "''")
    $vigiaExeFront = $tarefas[1].ExeProc       # 'node|npm|vite'
    $vigiaLog = (Join-Path $env:LOCALAPPDATA "hangar\hangar-vigia.log").Replace("'", "''")   # mesmo lugar dos outros .log
    # Here-string de aspas SIMPLES (@'...'@): zero interpolacao, entao `$_`/`$candidatos`/etc
    # sobrevivem literais sem precisar de crase nenhuma - o script so vira real quando o
    # `.Replace()` abaixo troca os tokens, e `.Replace()` e substituicao LITERAL (nao regex),
    # entao as barras invertidas de $vigiaPadraoCaminho (saida de [regex]::Escape) nao viram
    # sequencia de escape de ninguem.
    #
    # IDADE, nao so existencia: processo do checkout vivo NAO E garantia de que esta subindo -
    # no Windows nao existe zumbi, e um `uv`/`python` deste checkout PRESO (trava de rede num
    # `uv sync`, deadlock, I/O pendurado) fica no Get-CimInstance pra sempre, casa o criterio, e
    # a vigia original (so existencia) nunca mais chamaria Start-ScheduledTask - silenciosamente
    # pior que a versao agressiva demais de antes, porque e o caso que ninguem percebe. Um
    # processo so poupa o restart se nasceu ha MENOS de 10 min (bem acima do boot medido, ~15s;
    # bem abaixo de "pendurado"). CreationDate e o mesmo campo que Pare-Servico ja usa pra
    # comparar nascimento de processo (install.ps1, $nascMapa). Passado o limite, dispara MESMO
    # ASSIM e registra no log que havia processo velho sem porta aberta - o problema tem que
    # aparecer, nao sumir.
    # O FRONT tambem entra na vigia. Ate aqui ela so olhava a porta do backend e so reerguia o
    # `hangar-backend`; o `hangar-frontend` ficava sem rede nenhuma - `RestartCount` e 0 nas tres
    # tarefas (conferido com Get-ScheduledTask), entao o vite morto so voltava no proximo logon,
    # e quem acessa pelo Tailscale ve a PAGINA fora do ar com a API respondendo. Mesmo criterio
    # do backend, inclusive a heuristica de idade: processo do checkout vivo NAO prova que subiu.
    # A funcao existe pra os dois nao virarem duas copias que divergem no proximo conserto.
    $vigiaTemplate = @'
& {
    function Reergue($porta, $padrao, $exe, $tarefa) {
        if (Get-NetTCPConnection -State Listen -LocalPort $porta -ErrorAction SilentlyContinue) { return }
        $candidatos = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object { $_.CommandLine -and $_.CommandLine -match $padrao -and $_.Name -match $exe })
        $limite = (Get-Date).AddMinutes(-10)
        $recentes = @($candidatos | Where-Object { $_.CreationDate -and $_.CreationDate -gt $limite })
        if ($recentes.Count -gt 0) { return }
        if ($candidatos.Count -gt 0) { Write-Output "$(Get-Date -Format 's') vigia: processo(s) de $tarefa vivo(s) ha mais de 10 min sem a porta aberta - pode estar pendurado; reiniciando mesmo assim" }
        Start-ScheduledTask -TaskName $tarefa
    }
    Reergue __PORTA__ '(__APPMAIN__|__CAMINHO__)' '__EXE__' 'hangar-backend'
    Reergue __PORTAFRONT__ '__CAMINHOFRONT__' '__EXEFRONT__' 'hangar-frontend'
} *>&1 | Out-File -FilePath '__LOG__' -Append -Encoding utf8
'@
    # Numa linha so, sem quebra: um `.Replace(...)` iniciando a linha seguinte arrisca ser lido
    # como dot-sourcing pelo parser (mesmo com crase antes), e nenhuma das duas formas de quebra
    # de linha do resto do arquivo (crase, ou deixar parentese/vírgula aberto) cobre encadeamento
    # de metodo com seguranca - a mais simples e nao quebrar.
    $vigiaPs = $vigiaTemplate.Replace('__APPMAIN__', $vigiaPadraoAppMain).Replace('__CAMINHO__', $vigiaPadraoCaminho).Replace('__EXE__', $vigiaExeProc).Replace('__PORTA__', "$portaBack").Replace('__CAMINHOFRONT__', $vigiaPadraoFront).Replace('__EXEFRONT__', $vigiaExeFront).Replace('__PORTAFRONT__', "$portaFront").Replace('__LOG__', $vigiaLog)
    $vigiaEnc = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($vigiaPs))
    $vigiaVbs = Join-Path $env:LOCALAPPDATA "hangar\hangar-vigia.vbs"   # mesmo lugar dos outros .vbs
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $vigiaVbs) | Out-Null
    Escrever-Lancador $vigiaVbs @"
CreateObject("WScript.Shell").Run "powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand $vigiaEnc", 0, False
"@ 'vbs' | Out-Null
    # NAO e -AtLogOn puro (era a versao anterior) - MEDIDO na maquina real em 09/08/2026 que a
    # Repetition so comeca a CONTAR a partir do disparo do gatilho, e registrar a tarefa nao
    # dispara nada sozinho: registrada as 02:28, com o ultimo logon interativo as 19:53 do dia
    # ANTERIOR, o gatilho -AtLogOn ja tinha passado e nunca mais ia dispar por conta propria -
    # LastRunTime ficou em 30/11/1999 (nunca rodou), NextRunTime VAZIO, e nem iniciar a tarefa a
    # mao arma a repeticao. E exatamente o tipo de coisa que alguem "simplifica" de volta achando
    # AtLogon mais direto - nao e, ele so dispara em logon FUTURO, e reboot/retomada de suspensao
    # nao geram logon nenhum.
    #
    # -Once com horario no FUTURO IMEDIATO arma a serie na hora do REGISTRO, sem depender de
    # logon: e o unico jeito medido de deixar NextRunTime preenchido logo depois de
    # Register-ScheduledTask. -RepetitionDuration ([TimeSpan]::MaxValue) e recusado pelo
    # Agendador no PowerShell 5.1 - 9999 dias (~27 anos) e "pra sempre" na pratica sem cair
    # nessa rejeicao.
    $vigiaOnce = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) `
        -RepetitionInterval (New-TimeSpan -Minutes 5) -RepetitionDuration (New-TimeSpan -Days 9999)
    # -AtLogOn continua registrado JUNTO, nao no lugar - cinto e suspensorio: cobre o logon
    # interativo de verdade (login de manha, por exemplo), enquanto o -Once repetido acima e
    # quem cobre reboot e retomada de suspensao, os dois casos que nao passam por logon e que
    # sao justamente o motivo da vigia existir.
    $vigiaLogon = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
    # -Settings com bateria: o default e DisallowStartIfOnBatteries=$true, e a maquina que suspende
    # e justamente o notebook - a vigia ficaria morta exatamente quando e necessaria, e o teste na
    # tomada passaria. As tarefas existentes ja passam estes dois (acima).
    $vigiaSet = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    Register-ScheduledTask -TaskName 'hangar-vigia' `
        -Action (New-ScheduledTaskAction -Execute 'wscript.exe' -Argument "`"$vigiaVbs`"") `
        -Trigger $vigiaOnce, $vigiaLogon -Settings $vigiaSet `
        -Principal (New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive) -Force | Out-Null
    # CONFERE o NextRunTime, nao anuncia so por ter registrado - achado IMPORTANTE da revisao final:
    # este exato ponto ja mediu ERRADO 2x nesta maquina (o gatilho antigo -AtLogOn puro deixava
    # NextRunTime vazio, comentario acima sobre a medicao de 09/08/2026). So afirma com o campo
    # preenchido; vazio quer dizer que a vigia pode nao disparar sozinha, e isso tem que aparecer.
    $infoVigia = Get-ScheduledTask -TaskName 'hangar-vigia' -ErrorAction SilentlyContinue |
                 Get-ScheduledTaskInfo -ErrorAction SilentlyContinue
    if ($infoVigia -and $infoVigia.NextRunTime) {
        Ok "vigia registrada (proximo tick: $($infoVigia.NextRunTime), depois a cada 5 min)"
    } else {
        Falta 'vigia registrada mas NextRunTime veio vazio - ela pode nao disparar sozinha'
        $script:pendencias += 'vigia'
    }
    # Dispara AGORA, uma vez: com o backend ja no ar isso e inofensivo (a vigia so reage se a porta
    # estiver fechada), e prova de graca que o .vbs e o base64 realmente executam.
    Start-ScheduledTask -TaskName 'hangar-vigia' -ErrorAction SilentlyContinue
    } catch {
        Falta "nao deu pra registrar a vigia: $_"
        Nota 'Sem ela, um crash do backend so reergue no proximo logon (nao a cada 5 min).'
        $script:pendencias += 'vigia'
    }
} else {
    Nota 'pulado - rodando na mao, fechar o terminal derruba o backend'
}

# -- 7b/8 hangar-send + skills ---------------------------------------------------
# O hangar-send e bash falando com o backend por HTTP - nada nele exige unix. Faltavam tres coisas
# no Windows, e sao estas que este passo resolve:
#   1. um `python3` que exista (o instalador do Python cria python.exe e py.exe, e o hangar-send
#      chama python3 dez vezes pra ler JSON);
#   2. um lancador que o PowerShell enxergue, ja que o script nao tem extensao;
#   3. rodar o proprio install-hangar-send.sh - e ele quem cria o link, as skills e o bloco de
#      protocolo no ~/.claude/CLAUDE.md. Duplicar esse texto aqui daria duas fontes da verdade,
#      e a que diverge silenciosamente e sempre a copia.
Titulo '7b/8 hangar-send (recado e pareamento entre sessoes)'
$bash = $null
if (Tem 'git') {
    # O git fica em ...\cmd\git.exe; o bash mora em ...\bin\bash.exe do mesmo Git for Windows.
    $gitDir = Split-Path -Parent (Split-Path -Parent (Get-Command git).Source)
    $cand = Join-Path $gitDir 'bin\bash.exe'
    if (Test-Path $cand) { $bash = $cand }
}
if (-not $bash) {
    Falta 'bash do Git for Windows nao encontrado - hangar-send fica de fora'
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
        Falta 'nenhum Python real encontrado (so o atalho da Store) - hangar-send ficaria sem JSON'
        Nota 'instale com:  winget install --id Python.Python.3.13'
    } else {
        # C:\Windows\py.exe -> /c/Windows/py.exe, que e a forma que o bash do MSYS executa.
        $pyMsys = ($pyExe -replace '\\', '/') -replace '^([A-Za-z]):', '/$1'
        $arg = if ((Split-Path -Leaf $pyExe) -ieq 'py.exe') { ' -3' } else { '' }
        $shim = Join-Path $binUsuario 'python3'
        $corpoShim = "#!/bin/sh`n" +
                     "# Gerado por hangar/install.ps1 - o hangar-send chama python3.`n" +
                     "exec '$pyMsys'$arg `"`$@`"`n"
        if (Escrever-Lancador $shim $corpoShim 'sh') { Ok "atalho python3 -> $pyExe$arg" }
        else { Ok 'atalho python3 ja atualizado' }
    }

    # (2) lancador pro PowerShell: o script nao tem extensao, entao o Windows nao o executa
    # sozinho. O .cmd entrega tudo pro bash e repassa os argumentos.
    $lancador = Join-Path $binUsuario 'hangar-send.cmd'
    # PATH com o nosso bin NA FRENTE: o Windows tem um python3.exe proprio no atalho da
    # Microsoft Store (%LOCALAPPDATA%\Microsoft\WindowsApps), que vem antes no PATH e responde
    # "Python nao foi encontrado". Sem a precedencia, o atalho que acabamos de escrever nunca e
    # alcancado - medido, o install-hangar-send.sh falhava mesmo com o atalho correto no lugar.
    $conteudo = "@echo off`r`n" +
                "set `"PATH=%USERPROFILE%\.local\bin;%PATH%`"`r`n" +
                "`"$bash`" `"$raiz\scripts\hangar-send`" %*`r`n"
    if (Escrever-Lancador $lancador $conteudo 'cmd') { Ok "lancador hangar-send.cmd criado em $binUsuario" }
    else { Ok 'lancador hangar-send.cmd ja atualizado' }

    # (2b) lancador pro hangar-conta (helper de contas do claude-conta): sem ele o claude-conta.ps1
    # falha com "hangar-conta nao e reconhecido" antes de abrir o Claude.
    # Pelo PYTHON, nao pelo bash: o hangar-conta e um script Python (`#!/usr/bin/env python3`), e
    # `bash arquivo` NAO honra shebang - le o arquivo como shell e estoura no docstring. Foi assim
    # que este lancador nasceu quebrado (copia do hangar-send.cmd, que e bash de verdade). O mesmo
    # $pyExe/$arg do shim acima, que ja descarta o atalho da Store.
    $lancadorConta = Join-Path $binUsuario 'hangar-conta.cmd'
    if (-not $pyExe) {
        Falta 'hangar-conta.cmd nao criado - precisa de um Python real (ver acima)'
    } else {
        $conteudoConta = "@echo off`r`n" +
                         "`"$pyExe`"$arg `"$raiz\scripts\hangar-conta`" %*`r`n"
        if (Escrever-Lancador $lancadorConta $conteudoConta 'cmd') { Ok "lancador hangar-conta.cmd criado em $binUsuario" }
        else { Ok 'lancador hangar-conta.cmd ja atualizado' }
    }

    # (2c) lancador pro hangar-engine (motores de modelo). Sem ele o backend monta o comando do pane
    # como `hangar-engine --exec <motor> -- claude ...`, o pane morre no ato e o `tmux new-session`
    # devolve 0 assim mesmo: medido nesta VM, rc=0 na criacao e 3s depois a sessao ja nao existe.
    # O app reportava "sessao criada" e ela sumia calada. Hoje o backend recusa alto quando este
    # lancador falta (registry._exigir_cp_engine) — este bloco e o outro lado do conserto, o que
    # faz o motor de fato FUNCIONAR no Windows.
    # Pelo PYTHON e nao pelo bash, mesmo motivo do hangar-conta: o hangar-engine e script Python
    # (`#!/usr/bin/env python3`) e `bash arquivo` nao honra shebang.
    $lancadorEngine = Join-Path $binUsuario 'hangar-engine.cmd'
    if (-not $pyExe) {
        Falta 'hangar-engine.cmd nao criado - precisa de um Python real (ver acima)'
    } else {
        $conteudoEngine = "@echo off`r`n" +
                          "`"$pyExe`"$arg `"$raiz\scripts\hangar-engine`" %*`r`n"
        if (Escrever-Lancador $lancadorEngine $conteudoEngine 'cmd') { Ok "lancador hangar-engine.cmd criado em $binUsuario" }
        else { Ok 'lancador hangar-engine.cmd ja atualizado' }
    }

    # (3) PATH do usuario, pra `hangar-send` funcionar de qualquer terminal (e pro bash achar o shim).
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
        $saida = & $bash '-lc' "export PATH='$binMsys':`$PATH; cd '$rota' && ./scripts/install-hangar-send.sh" 2>&1
    } finally { $ErrorActionPreference = $anterior }
    if ($LASTEXITCODE -eq 0) {
        # O `ln -s` do Git Bash COPIA em vez de linkar, e o hangar-send se localiza pelo proprio
        # caminho: `dirname $(realpath $0)/../backend/.env`. Com a copia em ~/.local/bin, ele
        # procura o .env em ~/.local/backend/ - que nao existe, e o --list falha dizendo que nao
        # acha o backend. Substituimos a copia por um lancador que chama o script NO REPO com
        # caminho absoluto: dentro dele, $0 volta a ser o do repo e a busca acerta.
        $cpSendSh = Join-Path $binUsuario 'hangar-send'
        # PATH aqui TAMBEM, nao so no hangar-send.cmd: quem chama por este caminho e o Git Bash
        # (a ferramenta Bash de uma sessao Claude no Windows usa ele), e sem a precedencia o
        # python3 volta a ser o atalho da Microsoft Store. Os dois pontos de entrada precisam
        # da mesma garantia - consertar so um deles foi o que deixou o bug de pe.
        $corpoCp = "#!/bin/sh`n" +
                   "# Gerado por hangar/install.ps1 - ver comentario no instalador.`n" +
                   "PATH='$binMsys':`$PATH; export PATH`n" +
                   "exec '$rota/scripts/hangar-send' `"`$@`"`n"
        if (Escrever-Lancador $cpSendSh $corpoCp 'sh') {
            Ok 'hangar-send do ~/.local/bin aponta pro script do repo'
        }
        Ok 'hangar-send + skills instalados'
        Nota 'teste (em terminal NOVO):  hangar-send --list'
    } else {
        Falta 'install-hangar-send.sh falhou:'
        $saida | Select-Object -Last 12 | ForEach-Object { Nota "  $_" }
        Nota "rodar na mao:  & '$bash' -lc 'cd $rota && ./scripts/install-hangar-send.sh'"
    }
}

# -- Passos de atualizacao: marcar como ja feitos ----------------------------
# Uma instalacao do ZERO ja satisfaz todo passo de docs\atualizacoes\ -- eles existem pra levar uma
# maquina ANTIGA ate aqui. Sem esta marca, o primeiro Atualizar no app rodaria a historia inteira de
# passos, todos ja cumpridos por este instalador. No -Update NAO se marca nada: ali a maquina e
# justamente a antiga, e os passos precisam rodar.
if (-not $Update) {
    Titulo 'Passos de atualizacao'
    $marcou = $false
    try {
        & uv run --directory "$raiz\backend" python -c "from app import atualizacoes; atualizacoes.marcar_todos()" 2>&1 | Out-Null
        $marcou = ($LASTEXITCODE -eq 0)
    } catch { $marcou = $false }
    if ($marcou) { Ok 'marcados como ja aplicados (instalacao nova)' }
    else { Nota 'nao consegui marcar agora -- o app resolve no primeiro Atualizar' }
}

# -- 7c/8 Atualizar sozinho no proximo git pull ------------------------------
# Hook post-merge: roda depois de todo `git pull` bem-sucedido e re-aplica o que o pull NAO atualiza
# sozinho (wrapper, build do front, tarefa agendada, config do multiplexador). No Linux quem instala
# e o install.sh (bloco HOOK la); no Windows NINGUEM instalava -- o -Update existia e era documentado
# (ver o cabecalho deste arquivo), mas o gatilho nunca era criado, entao TODO pull deixava codigo novo
# com wrapper/front/tarefa velhos e nada avisava: so aplicava quem lembrasse de rodar `-Update` na mao.
# Medido nesta maquina: .git/hooks tinha so os .sample. O CORPO do hook ja era cross-platform (ele
# ramifica por `uname -s` e chama ESTE script no MINGW), entao faltava apenas a INSTALACAO.
# ponytail: o corpo vive em scripts/post-merge.hook, FONTE UNICA pros dois instaladores -- inline nos
# dois arquivos, as copias divergiriam, que e a familia de bug mais caro deste projeto.
Titulo '7c/8 Atualizar sozinho no proximo git pull'
$hookAlvo = "$raiz\.git\hooks\post-merge"
$hookFonte = "$raiz\scripts\post-merge.hook"
$hookMarca = 'hangar-post-merge-hook'
if ($Update) {
    # O proprio hook pode ser quem esta chamando: nao se reinstala no meio da propria execucao.
    Nota 'pulado no -Update (o hook pode ser o proprio chamador)'
} elseif (-not (Test-Path "$raiz\.git")) {
    Falta 'sem .git (copia sem historico?) - hook de atualizacao indisponivel'
} elseif (-not (Test-Path $hookFonte)) {
    Falta 'scripts\post-merge.hook nao encontrado - hook nao instalado'
} elseif ((Test-Path $hookAlvo) -and (Select-String -Path $hookAlvo -Pattern $hookMarca -Quiet)) {
    Ok 'hook de atualizacao ja instalado'
    Nota "desligar:  del `"$hookAlvo`""
} elseif (Test-Path $hookAlvo) {
    # Hook de terceiro (do usuario ou de outra ferramenta): nunca sobrescrever.
    Falta 'ja existe um .git\hooks\post-merge que nao e nosso - nao vou mexer nele'
    Nota 'pra somar, acrescente a linha:  powershell -ExecutionPolicy Bypass -File install.ps1 -Update'
} else {
    Nota 'Ele so roda no pull, que e voce quem da. Nada nele pede senha.'
    if (Pergunte "Deixar o proximo 'git pull' ja se atualizar sozinho?") {
        # LF e SEM BOM, obrigatorio: o bash do Git le o shebang literalmente, entao CRLF vira
        # "#!/usr/bin/env bash\r" -> "bad interpreter", e um BOM antes do #! quebra igual. Set-Content
        # e Out-File do PS 5.1 produzem CRLF (e utf8 COM BOM), por isso a escrita vai pelo .NET.
        # O .gitattributes ja forca LF no arquivo do repo; a normalizacao aqui cobre checkout antigo.
        $hookTexto = ([System.IO.File]::ReadAllText($hookFonte)) -replace "`r`n", "`n"
        New-Item -ItemType Directory -Force -Path (Split-Path $hookAlvo) | Out-Null
        [System.IO.File]::WriteAllText($hookAlvo, $hookTexto, (New-Object System.Text.UTF8Encoding($false)))
        # Confere o RESULTADO em vez de confiar na escrita: hook com CRLF/BOM falha CALADO no pull
        # (o git nem reclama), e reportar "instalei" sem verificar e o erro que este projeto pagou
        # caro em outros caminhos. Deu ruim -> remove, pra nao deixar hook quebrado no lugar.
        $hb = [System.IO.File]::ReadAllBytes($hookAlvo)
        $temBom = ($hb.Length -ge 3 -and $hb[0] -eq 0xEF -and $hb[1] -eq 0xBB -and $hb[2] -eq 0xBF)
        $temCr = ($hb -contains 0x0D)
        if ($temBom -or $temCr) {
            Erro "hook gravado com $(if ($temBom) { 'BOM' } else { 'CRLF' }) - o bash do Git recusaria; removido"
            Remove-Item $hookAlvo -Force
            $script:pendencias += 'hook post-merge'
        } else {
            Ok "hook instalado - o proximo 'git pull' ja se atualiza sozinho"
            Nota "desligar:  del `"$hookAlvo`""
        }
    } else {
        Nota 'pulado - depois de um git pull, rode:  powershell -ExecutionPolicy Bypass -File install.ps1 -Update'
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
if (-not $morreu) {
    # O kill-session do psmux nao derruba o pane. Como a sessao e NOSSA e sabemos o pane_pid,
    # matamos o processo direto - sem `tmux kill-server`, que levaria junto as sessoes do usuario.
    # Limpar o proprio lixo e obrigacao do teste; deixar isso pra pessoa e transferir trabalho.
    $pids = & tmux list-panes -t "=$sessao" -F '#{pane_pid}' 2>$null
    foreach ($pane in $pids) {
        if ($pane -match '^\d+$') { Stop-Process -Id $pane -Force -ErrorAction SilentlyContinue }
    }
    Start-Sleep -Seconds 2
    $morreu = ((Nativo tmux has-session -t "=$sessao") -ne 0)
    if ($morreu) { Nota 'kill-session nao funcionou; a sessao de teste foi encerrada pelo PID' }
}
if ($morreu) {
    Ok 'o multiplexador mata sessao'
} else {
    # NAO aborta: tudo o que o app precisa pra FUNCIONAR ja passou. Mas tem que aparecer, porque
    # o sintoma no uso e sessao apagada que reaparece na lista.
    Falta "a sessao de teste '$sessao' nao morreu em 15s - apagar sessao pelo app pode deixar zumbi"
    Nota "limpar na mao:  tmux kill-session -t '=$sessao'"
}

# -- Confere e entrega o acesso -----------------------------------------------
# CONFERE, nao anuncia — mesma regra do `Pronto` condicional. So quando ha tarefa registrada: quem
# recusou o passo 7 nao deve esperar 20s por algo que ninguem pediu pra subir.
# Nao repete o poll: $subiu ja e o resultado do mesmo `Get-NetTCPConnection -State Listen
# -LocalPort $portaBack` la em cima (install.ps1:906-911), so mesmo $registrou/$jaAgendado - um
# segundo loop pagaria os mesmos ate ~40s de novo e imprimiria uma segunda mensagem (Ok/Erro) sobre o
# MESMO fato que o Ok/Falta de la em cima ja anunciou. $subiu so alimentava a tela; aqui ele passa
# a alimentar $pendencias tambem, que e o que falta pro gate do fim do script barrar de verdade.
$vivo = $false
if ($jaAgendado -or $registrou) {
    $vivo = $subiu
    if (-not $vivo) { $script:pendencias += 'backend no ar' }
}

# O QR do backend NAO aparece nesta maquina: print_pairing so desenha se sys.stdout.isatty()
# (backend/app/main.py:56) e a tarefa roda por wscript, sem console. Entao quem desenha e o
# instalador, que ESTA num terminal. Quatro detalhes, todos ja pagos neste arquivo:
#  - `Nativo` engole a saida (install.ps1:65), entao a chamada aqui e direta;
#  - `uv run` escreve rotina no stderr ("Resolved N packages"), e com $ErrorActionPreference='Stop'
#    isso vira NativeCommandError -> o preference baixa pra Continue em volta (install.ps1:387-402);
#  - sem UTF-8 no console os blocos do QR viram '?' na codepage OEM;
#  - o .env e lido em caminho relativo (backend/app/config.py:83), logo roda de dentro de backend\.
# Restrito ao modo INTERATIVO (-not $Update): achado MINOR da revisao final. O -Update roda pelo
# hook post-merge, e este bloco imprimiria a URL COM TOKEN no scrollback de TODO `git pull`, alem
# de pagar um `uv run` que ninguem pediu (o QR nao serve pra nada num pull desatendido - nao ha
# celular olhando o terminal naquele momento).
$qrMostrado = $false
if ($vivo -and -not $Update) {
    $eapAnt = $ErrorActionPreference
    $encAnt = $null
    $pyioAnt = $env:PYTHONIOENCODING
    Push-Location "$raiz\backend"
    try {
        $ErrorActionPreference = 'Continue'
        $encAnt = [Console]::OutputEncoding
        [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
        $env:PYTHONIOENCODING = 'utf-8'
        & uv run python -c "from app.config import settings; from app.main import print_pairing; print_pairing(settings)"
        # Confere $LASTEXITCODE - achado MINOR da revisao final: sem isto, um print_pairing que
        # falha (exit != 0, sem excecao nenhuma porque o preference aqui e 'Continue') fazia o QR
        # sumir em silencio, sem nenhum Falta/Nota explicando o motivo.
        if ($LASTEXITCODE -eq 0) {
            $qrMostrado = $true
        } else {
            Falta "nao consegui desenhar o QR (uv run saiu $LASTEXITCODE)"
        }
    } catch {
        Nota "nao consegui desenhar o QR: $_"
    } finally {
        if ($encAnt) { [Console]::OutputEncoding = $encAnt }
        $env:PYTHONIOENCODING = $pyioAnt
        $ErrorActionPreference = $eapAnt
        Pop-Location
    }
}

# Nunca no -Update: ele roda do hook post-merge, e um `git pull` que abre janela de navegador e
# hostil. try/catch porque $ErrorActionPreference='Stop' (install.ps1:24) transformaria "sem
# navegador padrao" em aborto do ultimo passo.
if ($vivo -and -not $Update) {
    # Com o TOKEN na URL, o mesmo mecanismo do QR: o app le o `?token=`, grava a credencial (e o
    # cookie cp_token que o SSE usa) e APAGA o parametro do historico da aba. Sem isto o instalador
    # abria a tela de login e mandava digitar na mao um token de 48 caracteres no proprio PC onde
    # ele acabou de ser gerado.
    #
    # O token continua OBRIGATORIO em todo acesso, inclusive no loopback: nao ha isencao por IP e
    # nao e isso que este bloco faz. O motivo de nao ter isencao esta em auth.py — este backend cria
    # sessao que roda comando na maquina, e QUALQUER pagina aberta no navegador consegue falar com
    # 127.0.0.1. O que muda aqui e so a conveniencia de nao digitar no PC; o celular segue digitando
    # (ou lendo o QR).
    $tokenAgora = Token-Do-Env
    $base = if ($script:cpPublicUrl) { $script:cpPublicUrl } else { "http://127.0.0.1:$portaBack" }
    $abrir = if ($tokenAgora) { "$base/?token=$([uri]::EscapeDataString($tokenAgora))" } else { $base }
    try { Start-Process $abrir | Out-Null; Ok "abri $base no navegador (ja autenticado)" }
    catch { Nota "abra na mao: $abrir" }
}

# -- Fim ---------------------------------------------------------------------
# `Pronto` SO quando nada falhou. Antes, um passo com X no meio ainda terminava com 'Pronto' e o
# texto de boas-vindas abaixo — e quem le a ultima linha acredita nela. Foi assim que o `npm ci`
# quebrado de 08/08/2026 passou por atualizacao bem-sucedida: o front velho ainda estava no ar
# servindo da memoria, entao nem a tela desmentia. Falha silenciosa e falha silenciosa em qualquer
# lugar, inclusive num instalador.
if ($pendencias.Count -gt 0) {
    Titulo "NAO terminou: $(($pendencias | Select-Object -Unique) -join ', ')"
    Write-Host @"
  Um ou mais passos falharam e estao listados acima com X. O que ja estava no ar continua no ar —
  e e por isso que isto precisa ser dito alto: a tela pode seguir funcionando servindo o build
  ANTERIOR, e a instalacao parecer boa por horas, ate a proxima vez que o processo cair.

  Resolva o que esta na lista e rode de novo:  .\install.ps1 -Update
"@
    exit 1
}
Titulo 'Pronto'
# So menciona o QR se ele de fato foi desenhado (achado MINOR da revisao final): quem escolheu
# "so nesta maquina" no passo 6/8 ou rodou em -Update nunca viu QR nenhum, e a frase ficava
# afirmando algo que nao aconteceu.
$linhaQr = ''
if ($qrMostrado) { $linhaQr = "`n  O QR acima ja leva o token: ler com a camera do celular abre o app JA conectado." }
Write-Host @"
  Abra a interface em http://127.0.0.1:$portaBack - o proprio backend serve o build que este
  instalador gerou, entao ali tem tela e API no mesmo endereco.
  O http://localhost:5173 tambem sobe: e o 'vite preview' servindo o MESMO build (a tarefa
  agendada roda preview, nao dev - sem recarga ao vivo). Ele escuta SO em 127.0.0.1
  (vite.config.ts) - do celular se chega pelo Tailscale, nao pelo IP da LAN direto.
  Pra mexer no layout com recarga ao vivo: pare a tarefa hangar-frontend e rode 'npm run dev'.

  Rodar na mao (se voce pulou o passo 7):
      cd backend  ; `$env:CP_LAN_BIND_IP='0.0.0.0' ; uv run python -m app.main
      cd frontend ; npm run dev
$linhaQr
  Guarde: quem tiver essa URL entra sem senha. Ela fica no historico do navegador desta
  maquina, e num navegador logado em conta o historico sincroniza pra nuvem do fornecedor.
  Guia completo (Tailscale, instalar como PWA, cada tela): docs\USAGE.md

  O que este Windows ainda NAO tem:
  - wrappers do `codex`, do `pi` e do `kimi`, e a extensao hangar-state.ts do Pi. Sessao Codex, Pi
    ou Kimi aberta por voce no terminal nao aparece; criada pelo app, funciona.
  - resurrect/continuum abaixo, e mais nada desta lista: motor de modelo (tela Motores /
    `CP_ENGINE`) PASSOU a funcionar aqui - o hangar-engine roda o comando por subprocess no Windows
    (o exec com env crasha la, medido) e o passo 7b instala o hangar-engine.cmd.
  - resurrect/continuum (sessoes sobreviverem a reboot): sao plugins de tmux em bash, e o
    psmux nao roda plugin de tmux. Fechou o Windows, as sessoes se foram.
"@

# -- Resumo (o que a pessoa precisa ter na mao quando a janela fechar) --------
# A janela do instalador FECHA sozinha quando ele foi aberto com duplo clique ou por
# `irm ... | iex` num processo proprio, e ate aqui a unica copia do token era uma linha no meio da
# saida. Este bloco existe pra ser a ULTIMA coisa na tela: token, endereco local e endereco do
# Tailscale, os tres juntos.
$tokenFim = Token-Do-Env
Write-Host ""
Write-Host "  ---------------------------------------------------------------" -ForegroundColor Cyan
Write-Host "   RESUMO" -ForegroundColor Cyan
if ($tokenFim) {
    Write-Host "   token   : " -NoNewline; Write-Host $tokenFim -ForegroundColor Yellow
    Write-Host "             (e o que voce digita no celular; fica em backend\.env)"
} else {
    Write-Host "   token   : nao consegui ler de backend\.env - veja o passo 3/8 acima" -ForegroundColor Red
}
Write-Host "   local   : http://127.0.0.1:$portaBack"
if ($script:cpPublicUrl) { Write-Host "   celular : $($script:cpPublicUrl)" }
else { Write-Host "   celular : nao publicado no Tailscale (passo 6/8 pulado ou 'so nesta maquina')" }
Write-Host "  ---------------------------------------------------------------" -ForegroundColor Cyan
Write-Host ""

# Pausa SO com console interativo: com o stdin vindo de um pipe (irm|iex chamado por outro
# processo, SSH, tarefa agendada) o Read-Host voltaria na hora e a pausa nao seguraria nada; e no
# -Update ela travaria um `git pull` esperando por uma tecla que ninguem vai apertar.
if ($script:Interativo -and -not $Update) {
    Read-Host '  Enter pra fechar' | Out-Null
}
