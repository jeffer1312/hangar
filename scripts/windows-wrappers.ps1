# Wrappers do Hangar no perfil do PowerShell (Windows). Uma copia so, com dois donos: o
# install.ps1 (passo 5/8) e o botao Consertar do painel de saude (scripts\setup-windows-wrappers.ps1).
# Dot-source: as funcoes usam $raiz, Ok, Nota e Falta de quem chama.

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
        $temSete = [bool](Get-Command pwsh -ErrorAction SilentlyContinue) -or
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

$MarcaWrapper = '# >>> hangar >>>'
$MarcaWrapperFim = '# <<< hangar <<<'

# Marcadores de blocos NOSSOS que ficaram pra tras em instalacoes antigas. Sao os dois nomes que
# este projeto ja teve; qualquer outro marcador no perfil da pessoa NAO entra nesta lista e nao e
# tocado. Nao e faxina: hoje o bloco legado tambem faz `. claude.ps1`, entao um perfil com os dois
# carrega o wrapper DUAS vezes a cada terminal novo.
$MarcasLegadas = @(
    @{ Ini = '# >>> claude-cockpit >>>'; Fim = '# <<< claude-cockpit <<<' },
    @{ Ini = '# >>> claude-pocket >>>';  Fim = '# <<< claude-pocket <<<'  }
)

function Raiz-Do-Perfil {
    <#
      O bloco aponta pelo ~\.hangar\current quando ele e esta mesma copia: a pasta da versao some na
      poda, e o current acompanha a troca. Outra copia, ou alvo relativo, fica com a raiz de quem roda.
    #>
    $atual = Join-Path $HOME '.hangar\current'
    if (-not (Test-Path -LiteralPath $atual)) { return $raiz }
    $item = Get-Item -LiteralPath $atual
    $alvo = if ($item.LinkType) { @($item.Target)[0] } else { $item.FullName }
    if (-not $alvo -or -not [IO.Path]::IsPathRooted($alvo)) { return $raiz }
    # Caminho que o .NET nao normaliza fica com a raiz de quem roda, sem derrubar o passo.
    try {
        $de = [IO.Path]::GetFullPath($alvo).TrimEnd('\')
        $para = [IO.Path]::GetFullPath($raiz).TrimEnd('\')
    } catch { return $raiz }
    if ($de -ieq $para) { return $atual }
    return $raiz
}

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
    # O ajudante comum primeiro: os wrappers usam as funcoes dele. So entra o que existe nesta copia,
    # e o Test-Path no perfil segura a volta para uma versao que ainda nao tinha o arquivo.
    $raizPerfil = Raiz-Do-Perfil
    $linhas = @($MarcaWrapper)
    foreach ($w in @('hangar-open', 'claude', 'claude-conta', 'codex', 'pi', 'omp', 'kimi', 'agy')) {
        $arq = "$raizPerfil\scripts\shell\$w.ps1"
        if (Test-Path -LiteralPath $arq -PathType Leaf) { $linhas += "if (Test-Path -LiteralPath `"$arq`") { . `"$arq`" }" }
    }
    $linhas += $MarcaWrapperFim
    $bloco = $linhas -join "`r`n"

    # Marca de abertura SEM a de fechamento: arquivo mexido na mao. Nao adivinha onde o bloco
    # termina — o regex abaixo tambem nao casaria, e ai o bloco novo entraria embaixo do meio-bloco
    # velho. Dizer isso e melhor que reescrever o perfil de alguem por palpite.
    if ($texto.Contains($MarcaWrapper) -and -not $texto.Contains($MarcaWrapperFim)) {
        return 'MEXIDO NA MAO (marca de fim ausente) - nao toquei'
    }

    $padrao = '(?s)' + [regex]::Escape($MarcaWrapper) + '.*?' + [regex]::Escape($MarcaWrapperFim) + '\r?\n?'
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

function Instalar-Wrappers($perfis) {
    <# Libera a ExecutionPolicy onde precisa e grava o bloco em cada perfil. Devolve $true
       se todos os perfis ficaram com o bloco. Usa Ok/Nota/Falta de quem chama. #>
    $tudoCerto = $true
    # O Windows vem com ExecutionPolicy = Restricted, que recusa carregar QUALQUER perfil. Escrever
    # o bloco assim mesmo nao so deixaria o wrapper sem carregar: todo terminal novo passaria a
    # cuspir um PSSecurityException por causa de um arquivo que nos criamos. Medido nesta maquina.
    # RemoteSigned no escopo CurrentUser nao precisa de admin e e o que qualquer ferramenta de
    # PowerShell pede: script local roda, script baixado da internet so assinado.
    $podeEscrever = $true
    # A politica e POR INTERPRETADOR: rodando no pwsh 7 (RemoteSigned de fabrica) o Get daqui nao
    # ve o Windows PowerShell 5.1 em Restricted — e o 5.1 e o terminal padrao, onde o perfil e o
    # `codex.ps1` do npm morriam com PSSecurityException mesmo depois de instalar.
    $interpretes = @(@{ nome = 'Windows PowerShell 5.1'; exe = 'powershell.exe' })
    if (Get-Command pwsh -ErrorAction SilentlyContinue) { $interpretes += @{ nome = 'PowerShell 7'; exe = 'pwsh' } }
    $restritos = @($interpretes | Where-Object {
        (& $_.exe -NoProfile -Command 'Get-ExecutionPolicy' 2>$null) -eq 'Restricted' })
    if ($restritos.Count -gt 0) {
        Nota ("ExecutionPolicy=Restricted em: " + (($restritos | ForEach-Object { $_.nome }) -join ', ') + ". Nenhum perfil carrega la.")
        # Sem perguntar: RemoteSigned no escopo do usuario e o que o wrapper, o `codex.ps1` do
        # npm e qualquer ferramenta de PowerShell exigem; a pergunta so deixava instalacao sem
        # terminal (-Sim, pelo app) com o wrapper de fora, calada.
        Nota 'Liberando script local pro seu usuario (RemoteSigned, sem admin).'
        if ($true) {
            foreach ($i in $restritos) {
                & $i.exe -NoProfile -Command 'Set-ExecutionPolicy -Scope CurrentUser RemoteSigned -Force' 2>$null
                # Prova relendo: o Set roda noutro processo e uma politica travada por GPO falha
                # la sem chegar aqui — "Ok" sem reler escreveria o perfil que todo terminal recusaria.
                $agora = (& $i.exe -NoProfile -Command 'Get-ExecutionPolicy' 2>$null)
                if ($agora -eq 'Restricted' -or -not $agora) {
                    $podeEscrever = $false
                    Falta "$($i.nome): ExecutionPolicy continua Restricted (GPO?) - wrapper NAO instalado"
                } else {
                    Ok "$($i.nome): ExecutionPolicy do usuario = $agora"
                }
            }
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
            if ($r -like 'MEXIDO*') { Falta "$pf : $r"; $tudoCerto = $false } else { Ok "$pf : $r" }
        }
        Nota 'Vale nos terminais NOVOS - este aqui ainda esta com o perfil antigo.'
    }
    if (-not $podeEscrever) { $tudoCerto = $false }
    return $tudoCerto
}
