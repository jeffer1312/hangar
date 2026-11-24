# Ajudante comum dos wrappers do PowerShell (claude, pi, omp, kimi, agy) - o equivalente do
# scripts/hangar-open e do scripts/hangar-server-lookup. Acha o hangar-server, abre a sessao pelo
# `hangar-server open-cli` e anexa o terminal; sem o Hangar, abre no psmux com o nome da pasta.
#
# Carregado antes dos wrappers no bloco do $PROFILE (scripts/windows-wrappers.ps1).
# Compativel com Windows PowerShell 5.1 (nada de operador ternario nem ??).

# A regra unica dos atalhos (scripts/hangar-server-lookup): CP_RUST_SERVER_BIN vence e nao cai para
# outro; sem ela, o bin\ da versao em uso pelo caminho real do current; senao o mais novo entre o
# build desta copia e o baixado (empate fica com a copia). $null quando nao ha.
function Find-HangarServer {
    $escolhido = "$env:CP_RUST_SERVER_BIN".Trim()
    if ($escolhido) {
        if ($escolhido -eq '~' -or $escolhido.StartsWith('~\') -or $escolhido.StartsWith('~/')) { $escolhido = $HOME + $escolhido.Substring(1) }
        if (Test-Path -LiteralPath $escolhido -PathType Leaf) { return $escolhido }
        return $null
    }
    # A troca no Windows deixa so o `current.old` por um instante: espera o link novo ate 1 s.
    $atual = Join-Path $HOME '.hangar\current'
    for ($espera = 0; $espera -lt 20 -and -not (Test-Path -LiteralPath $atual) -and (Test-Path -LiteralPath "$atual.old"); $espera++) { Start-Sleep -Milliseconds 50 }
    $doPacote = Join-Path $atual 'bin\hangar-server.exe'
    if (Test-Path -LiteralPath $doPacote -PathType Leaf) {
        # Pelo caminho real da versao (o `cd -P` do sh); alvo relativo nao e seguido.
        $item = Get-Item -LiteralPath $atual
        $alvo = if ($item.LinkType) { @($item.Target)[0] } else { $null }
        $real = if ($alvo -and [IO.Path]::IsPathRooted($alvo)) { $alvo } else { $item.FullName }
        return (Join-Path $real 'bin\hangar-server.exe')
    }
    $melhor = $null
    foreach ($c in @((Join-Path $PSScriptRoot '..\..\crates\target\release\hangar-server.exe'),
                     (Join-Path $HOME '.hangar\bin\hangar-server.exe'))) {
        if (Test-Path -LiteralPath $c -PathType Leaf) {
            $item = Get-Item -LiteralPath $c
            if (-not $melhor -or $item.LastWriteTimeUtc -gt $melhor.LastWriteTimeUtc) { $melhor = $item }
        }
    }
    if ($melhor) { return $melhor.FullName }
    return $null
}

# Funcao propria para o teste trocar: o wrapper so abre sessao com a entrada no terminal.
function Test-HangarInteractive { -not [Console]::IsInputRedirected }

function Get-HangarTmux {
    (Get-Command tmux -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
}

# Algum argumento casa o padrao? Maiuscula conta, como no `case` do sh (-v e -V sao flags diferentes).
function Test-HangarArg([object[]]$lista, [string]$padrao) {
    foreach ($a in $lista) { if ([string]$a -cmatch $padrao) { return $true } }
    return $false
}

# O PowerShell 5.1 (e o 7 ate o 7.2) nao escapa a aspa de dentro do argumento ao chamar um
# executavel, e o 7.3+ no modo Windows volta a esse jeito para .cmd/.bat: o prompt `diga "oi"`
# chegaria partido. Nesses casos o escape vai aqui, nas regras do CommandLineToArgvW.
function Test-HangarLegacyArgs([string]$exe) {
    $modo = Get-Variable PSNativeCommandArgumentPassing -ValueOnly -ErrorAction SilentlyContinue
    if ($null -eq $modo -or [string]$modo -eq 'Legacy') { return $true }
    return ([string]$modo -eq 'Windows' -and [IO.Path]::GetExtension($exe) -match '^\.(cmd|bat)$')
}

function ConvertTo-HangarNativeArgs([string]$exe, [object[]]$lista) {
    $legado = Test-HangarLegacyArgs $exe
    $saida = New-Object System.Collections.Generic.List[string]
    foreach ($item in $lista) {
        $a = [string]$item
        if ($legado) {
            # Vazio some no modo antigo; aspa ganha \ e as barras antes dela dobram. Com espaco o
            # PowerShell poe aspas em volta, e so o 5.1 nao dobra a barra do fim antes delas (o 7
            # ja dobra no NativeCommandParameterBinder; dobrar aqui tambem chegaria \\).
            if ($a -eq '') { $a = '""' }
            else {
                $a = $a -replace '(\\*)"', '$1$1\"'
                if ($a -match '\s' -and $PSVersionTable.PSEdition -ne 'Core') { $a = $a -replace '(\\+)$', '$1$1' }
            }
        }
        $saida.Add($a)
    }
    return ,$saida.ToArray()
}

# Roda o executavel com os argumentos inteiros. A saida vai direto ao terminal: nunca chamar
# atribuindo o resultado, senao a TUI perde o console.
function Invoke-HangarNative([string]$exe, [object[]]$lista, [object[]]$entrada = $null) {
    $argv = ConvertTo-HangarNativeArgs $exe $lista
    # So o que veio pelo pipeline vai ao stdin; sem ele o console fica inteiro com a TUI.
    if ($null -ne $entrada) { $entrada | & $exe @argv } else { & $exe @argv }
}

# A ponte de skills do Claude para o pi/omp/kimi/agy, refeita na largada como o hangar-skills-sync do
# POSIX. Silenciosa e sem falhar a abertura; o erro fica num arquivo para quem procurar.
function Sync-HangarSkills {
    $servidor = Find-HangarServer
    if (-not $servidor) { return }
    $anterior = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $saida = @(& $servidor harness-setup skills 2>&1 | ForEach-Object { "$_" })
        $codigo = $LASTEXITCODE
        if ($codigo -ne 0) {
            $erro = Join-Path ([IO.Path]::GetTempPath()) 'hangar-skills-sync.err'
            [IO.File]::AppendAllText($erro, ((@($saida) + "harness-setup skills saiu com $codigo") -join [Environment]::NewLine) + [Environment]::NewLine)
        }
    } catch { } finally { $ErrorActionPreference = $anterior }
    $global:LASTEXITCODE = 0
}

# TMUX herdado pode estar MORTO (terminal reaproveitado de um pane que ja fechou): sem pane vivo,
# limpa e segue pelo caminho "fora do multiplexador".
function Clear-HangarStaleTmux {
    if (-not $env:TMUX) { return }
    # Local: com 'Stop' no perfil, o stderr do psmux redirecionado viraria excecao.
    $ErrorActionPreference = 'Continue'
    $vivo = $false
    $tmux = Get-HangarTmux
    if ($env:TMUX_PANE -and $tmux) {
        & $tmux list-panes -t $env:TMUX_PANE 2>&1 | Out-Null
        $vivo = ($LASTEXITCODE -eq 0)
    }
    if (-not $vivo) { $env:TMUX = $null; $env:TMUX_PANE = $null }
}

# Abre a sessao pelo lancador do Hangar e anexa o terminal nela. Deixa em $LASTEXITCODE: 100 =
# rodar sem o Hangar (argumento fora do pedido, servidor ausente ou antigo); outro != 0 = erro ja
# mostrado, sem segunda abertura. Chamar como comando solto, nunca atribuindo: o attach e a TUI.
function Open-HangarSession([string]$provider, [string]$cli, [object[]]$lista) {
    $servidor = Find-HangarServer
    if (-not $servidor) {
        [Console]::Error.WriteLine("hangar: hangar-server nao encontrado; abrindo o $cli sem o Hangar")
        $global:LASTEXITCODE = 100; return
    }
    $argv = ConvertTo-HangarNativeArgs $servidor (@('open-cli', $provider) + @($lista))
    # O open-cli prefere o PWD ao diretorio do processo, e o PowerShell nao o atualiza: um PWD
    # herdado (Git Bash) abriria a sessao noutra pasta.
    $tinhaPwd = Test-Path Env:PWD
    $pwdAntes = $env:PWD
    $env:PWD = (Get-Location).ProviderPath
    try {
        $saida = @(& $servidor @argv)
        $codigo = $LASTEXITCODE
    } finally {
        if ($tinhaPwd) { $env:PWD = $pwdAntes } else { Remove-Item Env:PWD -ErrorAction SilentlyContinue }
    }
    # 2 = hangar-server antigo, sem o open-cli: nada foi enviado.
    if ($codigo -eq 2) {
        [Console]::Error.WriteLine("hangar: o hangar-server nao abriu a sessao (saida 2); abrindo o $cli sem o Hangar")
        $global:LASTEXITCODE = 100; return
    }
    if ($codigo -ne 0) { $global:LASTEXITCODE = $codigo; return }
    $nome = @($saida | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) | Select-Object -Last 1
    if (-not $nome) {
        [Console]::Error.WriteLine("hangar: resposta do hangar-server sem o nome da sessao")
        $global:LASTEXITCODE = 1; return
    }
    $tmux = Get-HangarTmux
    if (-not $tmux) {
        [Console]::Error.WriteLine("hangar: sessao '$nome' aberta, mas o tmux (psmux) nao esta no PATH para anexar")
        $global:LASTEXITCODE = 1; return
    }
    # Nome cru: o psmux nao honra `=` em todo comando e o nome ja e exato la.
    & $tmux attach-session -t $nome
    # 100 aqui faria o wrapper abrir uma segunda vez.
    if ($LASTEXITCODE -eq 100) { $global:LASTEXITCODE = 1 }
}

# Sem o Hangar o CLI ficaria invisivel pro app: sessao no psmux com o nome da pasta, unica, e o
# terminal anexado nela. $extraEnv = 'NOME=valor' por -e; $comando = programa e argumentos.
function Open-HangarFolderSession([string[]]$extraEnv, [object[]]$comando) {
    $tmux = Get-HangarTmux
    if (-not $tmux) { Write-Error 'tmux (psmux) nao encontrado no PATH'; return }
    $pasta = (Get-Location).ProviderPath
    # FormD + descarte da marca = acento vira ASCII, mesma regra do backend (app/names.py).
    $base = Split-Path -Leaf $pasta
    $base = -join ($base.Normalize([Text.NormalizationForm]::FormD).ToCharArray() | Where-Object {
        [Globalization.CharUnicodeInfo]::GetUnicodeCategory($_) -ne 'NonSpacingMark'
    })
    $base = ($base -replace '[^A-Za-z0-9_-]', '-').Trim('-')
    if (-not $base) { $base = 'session' }
    $ErrorActionPreference = 'Continue'
    $nome = $base; $n = 2
    while ($true) {
        # `=`: match exato; sem ele "proj-2" viva responderia por "proj".
        & $tmux has-session -t "=$nome" 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { break }
        $nome = "$base-$n"; $n++
    }
    # No psmux o pane herda o ambiente de quem chama; o -e garante o que a sessao precisa.
    # CP_SESSION_NAME: identidade da sessao pro hangar-send de dentro do pane.
    $cmd = @('new-session', '-s', $nome, '-c', $pasta, '-e', "CP_SESSION_NAME=$nome")
    foreach ($e in @($extraEnv)) { if ($e) { $cmd += @('-e', $e) } }
    Invoke-HangarNative $tmux ($cmd + @($comando))
}
