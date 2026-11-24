# Testa os wrappers do PowerShell (scripts/shell/{pi,omp,kimi,agy}.ps1 + hangar-open.ps1) sem os CLIs
# de verdade nem o psmux: pi/omp/kimi/agy/tmux/hangar-server falsos num PATH temporario gravam o
# argv como o processo o leu, e o teste confere. No Windows tambem a busca do hangar-server e o
# bloco do perfil. Modelo: scripts/test-wrappers.sh.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-windows-wrappers.ps1
#   pwsh -NoProfile -File scripts/test-windows-wrappers.ps1
$ErrorActionPreference = 'Stop'
$shell = Join-Path $PSScriptRoot 'shell'
$ehWindows = [IO.Path]::DirectorySeparatorChar -eq '\'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ('hangar-ww-' + [guid]::NewGuid().ToString('N'))
$bin = Join-Path $tmp 'bin'
$log = Join-Path $tmp 'log.jsonl'
$ps = (Get-Process -Id $PID).Path
$falhas = New-Object System.Collections.Generic.List[string]
function Check($ok, [string]$msg) { if (-not $ok) { $falhas.Add($msg); Write-Output "FALHOU: $msg" } }
$sep = [string][char]1
function Same($a, $b) { (@($a) -join $sep) -ceq (@($b) -join $sep) }
function Show($a) { '[' + ((@($a) | ForEach-Object { "'$_'" }) -join ', ') + ']' }

New-Item -ItemType Directory -Force $bin | Out-Null
# O gravador le o argv do proprio processo, nao o $args: e o que um executavel de verdade receberia.
$rec = Join-Path $bin 'rec.ps1'
[IO.File]::WriteAllText($rec, @'
$argv = [Environment]::GetCommandLineArgs()
$i = [Array]::IndexOf($argv, '__HANGAR_FAKE__')
$quem = $argv[$i + 1]
$resto = @()
if ($argv.Count -gt $i + 2) { $resto = @($argv[($i + 2)..($argv.Count - 1)]) }
# Le o stdin so do CLI do caso do pipeline: nos outros ele pode ser o terminal (ou o ssh) e a leitura nao terminaria.
$stdin = ''
if ($env:HT_READ_STDIN -ceq $quem) { $stdin = [Console]::In.ReadToEnd() }
$linha = [ordered]@{ who = $quem; args = $resto; pwd = "$env:PWD"; pi_session = "$env:CP_PI_SESSION"; omp_profile = "$env:OMP_PROFILE"; stdin = $stdin }
[IO.File]::AppendAllText($env:HT_LOG, (ConvertTo-Json $linha -Compress -Depth 3) + [char]10)
$codigo = 0
$primeiro = ''
if ($resto.Count) { $primeiro = $resto[0] }
if ($quem -eq 'hangar-server' -and $primeiro -eq 'open-cli') {
    if ($env:HT_OPEN_NAME) { [Console]::Out.WriteLine($env:HT_OPEN_NAME) }
    $codigo = [int]$env:HT_OPEN_RC
}
if ($quem -eq 'tmux' -and $primeiro -eq 'has-session') {
    $codigo = 1
    if ($env:HT_TAKEN -and ($resto -ccontains ('=' + $env:HT_TAKEN))) { $codigo = 0 }
}
if ($quem -eq 'tmux' -and $primeiro -eq 'list-panes') { $codigo = [int]$env:HT_PANE_RC }
exit $codigo
'@)
foreach ($nome in @('pi', 'omp', 'kimi', 'agy', 'tmux', 'hangar-server')) {
    if ($ehWindows) {
        # .cmd como o npm instala: o PowerShell 7.3+ volta ao modo antigo de argumentos com ele.
        $oem = [Text.Encoding]::GetEncoding([Globalization.CultureInfo]::CurrentCulture.TextInfo.OEMCodePage)
        [IO.File]::WriteAllText((Join-Path $bin "$nome.cmd"),
            "@`"$ps`" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$rec`" __HANGAR_FAKE__ $nome %*`r`n@exit /b %ERRORLEVEL%`r`n", $oem)
    } else {
        $f = Join-Path $bin $nome
        [IO.File]::WriteAllText($f, "#!/bin/sh`nexec '$ps' -NoProfile -NonInteractive -File '$rec' __HANGAR_FAKE__ $nome `"`$@`"`n")
        & chmod +x $f
    }
}
$servidorFalso = Join-Path $bin 'hangar-server'
if ($ehWindows) { $servidorFalso += '.cmd' }

$caminhoAntes = $env:PATH
$env:PATH = $bin + [IO.Path]::PathSeparator + $env:PATH
$env:CP_RUST_SERVER_BIN = $servidorFalso
$env:HT_LOG = $log
$env:PI_CODING_AGENT_DIR = Join-Path $tmp 'omp-agent'
$pasta = Join-Path $tmp ('Projeto A' + [char]0x00E7 + [char]0x00E3 + 'o')
New-Item -ItemType Directory -Force $pasta | Out-Null
Push-Location -LiteralPath $pasta
try {
    $cwd = (Get-Location).ProviderPath
    $slug = '--' + $cwd.TrimStart('/', '\').Replace('/', '-').Replace('\', '-').Replace(':', '-') + '--'

    . (Join-Path $shell 'hangar-open.ps1')
    foreach ($w in @('pi', 'omp', 'kimi', 'agy')) { . (Join-Path $shell "$w.ps1") }
    # A entrada do teste nao e terminal: quem decide "interativo" aqui e o caso.
    $script:interativo = $true
    function Test-HangarInteractive { $script:interativo }

    function Reset {
        if (Test-Path -LiteralPath $log) { Remove-Item -LiteralPath $log -Force }
        foreach ($v in @('TMUX', 'TMUX_PANE', 'CP_PI_SESSION', 'OMP_PROFILE', 'HT_TAKEN', 'HT_READ_STDIN')) { Remove-Item "Env:$v" -ErrorAction SilentlyContinue }
        $env:HT_OPEN_RC = '0'; $env:HT_OPEN_NAME = 'sessao-hangar'; $env:HT_PANE_RC = '0'
        $script:interativo = $true
    }
    function Calls {
        if (-not (Test-Path -LiteralPath $log)) { return @() }
        @(Get-Content -LiteralPath $log -Encoding UTF8 | Where-Object { $_ } | ForEach-Object { $_ | ConvertFrom-Json })
    }
    function Of($who) { @(Calls | Where-Object { $_.who -eq $who }) }
    function OpenCalls { @(Of 'hangar-server' | Where-Object { @($_.args)[0] -eq 'open-cli' }) }
    function TmuxCalls($verbo) { @(Of 'tmux' | Where-Object { @($_.args)[0] -eq $verbo }) }
    function Run([string]$cli, [object[]]$lista) {
        try { & $cli @lista } catch { $falhas.Add("$cli $(Show $lista): $($_.Exception.Message)"); Write-Output "FALHOU: $cli $(Show $lista): $_" }
    }
    # So o que o caminho cru do wrapper passou: subcomando, flag de impressao, retomada e dentro do psmux.
    function CheckRaw([string]$cli, [object[]]$lista, [object[]]$esperado, [string]$caso) {
        $raw = @(Of $cli)
        Check ($raw.Count -eq 1) "$cli ($caso): $($raw.Count) chamada(s) do binario, esperado 1"
        if ($raw.Count -eq 1) { Check (Same $raw[0].args $esperado) "$cli ($caso): argv $(Show $raw[0].args), esperado $(Show $esperado)" }
        Check (@(OpenCalls).Count -eq 0) "$cli ($caso): abriu pelo Hangar"
        Check (@(TmuxCalls 'new-session').Count -eq 0) "$cli ($caso): abriu sessao no psmux"
    }

    $casos = @(
        @{ cli = 'pi'; provider = 'pi'; sub = @('install', 'x'); print = @('oi', '-p'); resume = @('-c') },
        @{ cli = 'omp'; provider = 'omp'; sub = @('models'); print = @('--mode', 'rpc'); resume = @('--resume', 'r1') },
        @{ cli = 'kimi'; provider = 'kimi'; sub = @('export', 'x'); print = @('--version'); resume = @('-S', 's1') },
        @{ cli = 'agy'; provider = 'antigravity'; sub = @('models'); print = @('--output-format', 'json'); resume = @('--conversation', 'c1') }
    )
    # Aspas, barra no fim com espaco e argumento vazio: o PowerShell 5.1 estragaria os tres.
    $dificil = @('diga "oi" agora', 'C:\pasta com espaco\', '', 'x\"y')

    foreach ($c in $casos) {
        $cli = $c.cli
        Reset; Run $cli $c.sub; CheckRaw $cli $c.sub $c.sub 'subcomando'
        Reset; Run $cli $c.print; CheckRaw $cli $c.print $c.print 'flag que imprime'
        Check (@(Of 'hangar-server').Count -eq 0) "$cli (flag que imprime): chamou o hangar-server"
        Reset; Run $cli $c.resume; CheckRaw $cli $c.resume $c.resume 'retomada'

        # Fora do psmux e em terminal: open-cli com os argumentos e attach no nome que ele imprimiu.
        Reset; $env:HT_OPEN_NAME = "s-$cli"; Run $cli @('--model', 'm1')
        $open = @(OpenCalls)
        Check ($open.Count -eq 1) "$cli (abrir): $($open.Count) open-cli, esperado 1"
        if ($open.Count -eq 1) {
            Check (Same $open[0].args @('open-cli', $c.provider, '--model', 'm1')) "$cli (abrir): open-cli $(Show $open[0].args)"
            Check ($open[0].pwd -ceq $cwd) "$cli (abrir): PWD do open-cli '$($open[0].pwd)', esperado '$cwd'"
        }
        $attach = @(TmuxCalls 'attach-session')
        Check ($attach.Count -eq 1 -and (Same $attach[0].args @('attach-session', '-t', "s-$cli"))) "$cli (abrir): attach $(Show $attach.args)"
        Check (@(Of $cli).Count -eq 0) "$cli (abrir): rodou o binario cru"

        Reset; Run $cli $dificil
        $open = @(OpenCalls)
        Check ($open.Count -eq 1 -and (Same $open[0].args (@('open-cli', $c.provider) + $dificil))) "$cli (aspas): open-cli $(Show $open.args)"

        # Saida 100: sessao no psmux com o nome da pasta, o ambiente do provider e o terminal anexado.
        Reset; $env:HT_OPEN_RC = '100'; Run $cli @('oi')
        $nova = @(TmuxCalls 'new-session')
        Check ($nova.Count -eq 1) "$cli (100): $($nova.Count) new-session, esperado 1"
        if ($nova.Count -eq 1) {
            $a = @($nova[0].args)
            Check (Same $a[0..5] @('new-session', '-s', 'Projeto-Acao', '-c', $cwd, '-e')) "$cli (100): $(Show $a)"
            Check ($a[6] -ceq 'CP_SESSION_NAME=Projeto-Acao') "$cli (100): sem CP_SESSION_NAME: $(Show $a)"
            $resto = @($a[7..($a.Count - 1)])
            switch ($cli) {
                'pi' {
                    $id = ([string]$resto[1]) -replace '^CP_PI_SESSION=', ''
                    Check ((Same $resto @('-e', "CP_PI_SESSION=$id", 'pi', '--session-id', $id, 'oi')) -and $id) "pi (100): $(Show $resto)"
                }
                'omp' {
                    $id = ([string]$resto[1]) -replace '^CP_PI_SESSION=', ''
                    Check ((Same $resto[0..3] @('-e', "CP_PI_SESSION=$id", 'omp', '--session')) -and $resto[5] -ceq 'oi' -and $id) "omp (100): $(Show $resto)"
                    $alvo = [string]$resto[4]
                    Check ((Split-Path -Parent $alvo) -ceq [IO.Path]::Combine($env:PI_CODING_AGENT_DIR, 'sessions', $slug)) "omp (100): pasta do --session $alvo"
                    Check ((Split-Path -Leaf $alvo) -cmatch ('^\d{4}-\d\d-\d\dT\d\d-\d\d-\d\d-000Z_' + [regex]::Escape($id) + '\.jsonl$')) "omp (100): arquivo do --session $alvo"
                }
                'kimi' { Check (Same $resto @('-e', 'KIMI_CODE_TUI_FULL_SCREEN=1', 'kimi', 'oi')) "kimi (100): $(Show $resto)" }
                'agy' { Check (Same $resto @('agy', 'oi')) "agy (100): $(Show $resto)" }
            }
        }
        Check (@(Of $cli).Count -eq 0) "$cli (100): rodou o binario cru fora do psmux"

        # Ja dentro do psmux vivo: cru, com o id da sessao no pi/omp.
        Reset; $env:TMUX = 'psmux-1,1,0'; $env:TMUX_PANE = '%1'; Run $cli @('oi')
        $raw = @(Of $cli)
        Check ($raw.Count -eq 1 -and @(OpenCalls).Count -eq 0) "$cli (dentro do psmux): $($raw.Count) chamada(s) crua(s), $(@(OpenCalls).Count) open-cli"
        if ($raw.Count -eq 1) {
            $a = @($raw[0].args)
            switch ($cli) {
                'pi' { Check ((Same $a @('--session-id', $raw[0].pi_session, 'oi')) -and $raw[0].pi_session) "pi (dentro do psmux): $(Show $a), CP_PI_SESSION '$($raw[0].pi_session)'" }
                'omp' {
                    Check ($a.Count -eq 3 -and $a[0] -ceq '--session' -and $a[2] -ceq 'oi' -and $raw[0].pi_session -and
                           (Split-Path -Leaf $a[1]).EndsWith("_$($raw[0].pi_session).jsonl")) "omp (dentro do psmux): $(Show $a), CP_PI_SESSION '$($raw[0].pi_session)'"
                }
                default { Check (Same $a @('oi')) "$cli (dentro do psmux): $(Show $a)" }
            }
        }

        # TMUX herdado de pane morto: limpa e abre pelo Hangar.
        Reset; $env:TMUX = 'psmux-1,1,0'; $env:TMUX_PANE = '%9'; $env:HT_PANE_RC = '1'; Run $cli @('oi')
        Check (@(OpenCalls).Count -eq 1 -and -not $env:TMUX) "$cli (TMUX morto): nao abriu pelo Hangar ou o TMUX ficou"
    }

    # Entrada redirecionada: cru com o id, como dentro do psmux.
    Reset; $script:interativo = $false; Run 'pi' @('oi')
    $raw = @(Of 'pi')
    Check ($raw.Count -eq 1 -and (Same $raw[0].args @('--session-id', $raw[0].pi_session, 'oi')) -and @(OpenCalls).Count -eq 0) "pi (entrada redirecionada): $(Show $raw.args)"

    # Pipeline: o texto chega ao stdin do CLI cru, e nada abre pelo Hangar, com ou sem a flag que imprime.
    Reset; $env:HT_READ_STDIN = 'pi'
    try { 'linha 1', 'linha 2' | pi -p } catch { $falhas.Add("pi (pipeline): $($_.Exception.Message)") }
    $raw = @(Of 'pi')
    Check ($raw.Count -eq 1 -and (Same $raw[0].args @('-p')) -and ([string]$raw[0].stdin) -match 'linha 1\r?\nlinha 2' -and @(OpenCalls).Count -eq 0) "pi (pipeline): $(Show $raw.args), stdin '$($raw.stdin)'"
    Reset; $env:HT_READ_STDIN = 'kimi'
    try { 'pergunta' | kimi } catch { $falhas.Add("kimi (pipeline): $($_.Exception.Message)") }
    $raw = @(Of 'kimi')
    Check ($raw.Count -eq 1 -and ([string]$raw[0].stdin) -match 'pergunta' -and @(OpenCalls).Count -eq 0) "kimi (pipeline sem flag): $(Show $raw.args), stdin '$($raw.stdin)'"

    # Nome da pasta ja em uso: o sufixo -2.
    Reset; $env:HT_OPEN_RC = '100'; $env:HT_TAKEN = 'Projeto-Acao'; Run 'kimi' @()
    $nova = @(TmuxCalls 'new-session')
    Check ($nova.Count -eq 1 -and @($nova[0].args)[2] -ceq 'Projeto-Acao-2') "kimi (nome em uso): $(Show $nova.args)"

    # Perfil do omp: OMP_PROFILE para o open-cli e para o pane, e a raiz do --session e a do perfil.
    Reset; $env:HT_OPEN_RC = '100'; Run 'omp' @('--profile', 'perf', 'oi')
    $open = @(OpenCalls)
    Check ($open.Count -eq 1 -and $open[0].omp_profile -ceq 'perf') "omp (perfil): OMP_PROFILE do open-cli '$($open.omp_profile)'"
    $nova = @(TmuxCalls 'new-session')
    if ($nova.Count -eq 1) {
        $a = @($nova[0].args)
        Check (Same $a[7..10] @('-e', "CP_PI_SESSION=$(([string]$a[8]) -replace '^CP_PI_SESSION=', '')", '-e', 'OMP_PROFILE=perf')) "omp (perfil): $(Show $a)"
        $alvo = [string]$a[13]
        Check ((Split-Path -Parent $alvo) -ceq [IO.Path]::Combine($HOME, '.omp', 'profiles', 'perf', 'agent', 'sessions', $slug)) "omp (perfil): --session $alvo"
    } else { Check $false "omp (perfil): $($nova.Count) new-session" }

    if ($ehWindows) {
        # A busca do hangar-server (a mesma do claude.ps1) roda noutro processo: o $HOME vem do USERPROFILE.
        $casa = Join-Path $tmp 'casa'
        function Lookup([hashtable]$vars) {
            $antes = @{}
            foreach ($k in @('USERPROFILE', 'HOME', 'HOMEDRIVE', 'HOMEPATH', 'CP_RUST_SERVER_BIN')) { $antes[$k] = [Environment]::GetEnvironmentVariable($k) }
            try {
                # O 5.1 monta o $HOME do HOMEDRIVE + HOMEPATH; o 7, do USERPROFILE.
                $env:USERPROFILE = $casa; $env:HOME = $casa
                $env:HOMEDRIVE = [IO.Path]::GetPathRoot($casa).TrimEnd('\'); $env:HOMEPATH = $casa.Substring($env:HOMEDRIVE.Length)
                Remove-Item Env:CP_RUST_SERVER_BIN -ErrorAction SilentlyContinue
                foreach ($k in $vars.Keys) { Set-Item "Env:$k" $vars[$k] }
                $helper = (Join-Path $shell 'hangar-open.ps1').Replace("'", "''")
                "$(& $ps -NoProfile -NonInteractive -Command ". '$helper'; Find-HangarServer")".Trim()
            } finally { foreach ($k in $antes.Keys) { [Environment]::SetEnvironmentVariable($k, $antes[$k]) } }
        }
        $baixado = Join-Path $casa '.hangar\bin\hangar-server.exe'
        New-Item -ItemType Directory -Force (Split-Path $baixado) | Out-Null
        [IO.File]::WriteAllText($baixado, '')
        # Mais novo que qualquer build da copia: o baixado vence o empate e o mais velho.
        (Get-Item -LiteralPath $baixado).LastWriteTimeUtc = (Get-Date).ToUniversalTime().AddYears(5)
        Check ((Lookup @{}) -ieq $baixado) "lookup: sem current, esperado o baixado $baixado"
        Check ((Lookup @{ CP_RUST_SERVER_BIN = $servidorFalso }) -ieq $servidorFalso) 'lookup: CP_RUST_SERVER_BIN nao venceu'
        Check ((Lookup @{ CP_RUST_SERVER_BIN = (Join-Path $tmp 'nao-existe.exe') }) -eq '') 'lookup: CP_RUST_SERVER_BIN errado caiu para outro'
        $doCurrent = Join-Path $casa '.hangar\current\bin\hangar-server.exe'
        New-Item -ItemType Directory -Force (Split-Path $doCurrent) | Out-Null
        [IO.File]::WriteAllText($doCurrent, '')
        Check ((Lookup @{}) -ieq $doCurrent) "lookup: com current, esperado $doCurrent"

        # Bloco do perfil: o ajudante primeiro, os sete wrappers, cada um guardado por Test-Path; idempotente.
        $raiz = Split-Path -Parent $PSScriptRoot
        . (Join-Path $PSScriptRoot 'windows-wrappers.ps1')
        $perfil = Join-Path $tmp 'profile.ps1'
        [IO.File]::WriteAllText($perfil, "Set-Alias ll Get-ChildItem`r`n")
        Instalar-Bloco-No-Perfil $perfil | Out-Null
        $uma = [IO.File]::ReadAllBytes($perfil)
        Instalar-Bloco-No-Perfil $perfil | Out-Null
        $duas = [IO.File]::ReadAllBytes($perfil)
        Check (($uma -join ',') -eq ($duas -join ',')) 'perfil: a segunda gravacao mudou o arquivo'
        Check ($duas[0] -eq 0xEF -and $duas[1] -eq 0xBB -and $duas[2] -eq 0xBF) 'perfil: sem BOM'
        $texto = Ler-Texto $perfil
        Check ($texto.StartsWith('Set-Alias ll Get-ChildItem')) 'perfil: o que era da pessoa sumiu'
        $nomes = @([regex]::Matches($texto, 'shell\\([a-z-]+)\.ps1"\) \{ \. "') | ForEach-Object { $_.Groups[1].Value })
        Check (Same $nomes @('hangar-open', 'claude', 'claude-conta', 'codex', 'pi', 'omp', 'kimi', 'agy')) "perfil: wrappers $(Show $nomes)"
    }

} finally {
    Pop-Location
    $env:PATH = $caminhoAntes
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
if ($falhas.Count) { Write-Output "test-windows-wrappers: $($falhas.Count) falha(s)"; exit 1 }
Write-Output 'OK: wrappers pi/omp/kimi/agy (cru, abrir, aspas, psmux com a pasta, dentro do psmux)'
