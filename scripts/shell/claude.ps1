# Wrapper do `claude` pro PowerShell - equivalente do scripts/shell/claude.posix.sh.
#
# Faz todo `claude` interativo aberto no terminal nascer como sessao do Hangar, igual as do app:
# fora do multiplexador, o lancador do Hangar (`hangar-server open`) abre a sessao no psmux e o
# terminal anexa nela. --session-id/--resume/-c/--continue, modo -p, entrada redirecionada, ja
# dentro do psmux ou subcomando -> claude cru; argumento que o pedido nao cobre ou sem
# hangar-server -> sessao no psmux com o nome da pasta.
#
# Instalado pelo install.ps1, que adiciona um bloco marcado no $PROFILE chamando este arquivo.
# Compativel com Windows PowerShell 5.1 (nada de operador ternario nem API de .NET Core).

function claude {
    # Motor de modelo (Kimi, gateway proprio): mesma regra do wrapper POSIX - quando CP_ENGINE
    # esta setado quem executa e o hangar-engine, que aplica o env e da exec no claude.
    # -CommandType Application: pega o BINARIO e nunca esta funcao (senao recursao infinita).
    # Por tipo, nao por sufixo: o instalador nativo poe claude.exe, o npm poe claude.cmd.
    $claudeExe = (Get-Command claude -CommandType Application -ErrorAction SilentlyContinue |
                  Select-Object -First 1).Source
    if (-not $claudeExe) { Write-Error 'claude nao encontrado no PATH'; return }

    $pre = @()
    if ($env:CP_ENGINE) { $pre = @('hangar-engine', '--exec', $env:CP_ENGINE, '--') }

    $modoPrint = $false
    foreach ($a in $args) { if ($a -eq '-p' -or $a -eq '--print') { $modoPrint = $true } }

    # Plugins do Hangar por --plugin-dir, igual as sessoes que o backend abre: so assim ficam por
    # fora dos plugins do marketplace e o do Hangar espelha no app a faixa que os mods desenham. Os
    # caminhos vem do arquivo que o backend grava, um por linha e o do Hangar primeiro (este wrapper
    # nao sabe onde o repositorio mora); o backend so o grava quando o claude aceita a flag. Modo
    # -p nao tem faixa e segue como estava.
    $plug = @()
    $plugFile = Join-Path $HOME '.hangar\plugin-dir'
    if (-not $modoPrint -and (Test-Path -LiteralPath $plugFile -PathType Leaf)) {
        foreach ($linha in @(Get-Content -LiteralPath $plugFile -Encoding UTF8)) {
            $plugDir = ([string]$linha).Trim()
            if ($plugDir -and (Test-Path -LiteralPath $plugDir -PathType Container)) {
                $plug += @('--plugin-dir', $plugDir)
            } elseif ($plugDir) {
                [Console]::Error.WriteLine("hangar: plugin-dir '$plugDir' nao existe; a faixa dos mods nao vai para o app")
            }
        }
    }

    # Ja veio com id/retomada explicita? Nao inventa outro - repassa como esta. Injetar um
    # --session-id por cima de um --resume abriria uma conversa NOVA no lugar da pedida.
    foreach ($a in $args) {
        if ($a -match '^(--session-id|--resume)(=|$)' -or $a -eq '-c' -or $a -eq '--continue') {
            if ($pre.Count -eq 0) { & $claudeExe @plug @args }
            else { $r = @($pre[1..($pre.Count - 1)]) + @('claude') + $plug + $args; & $pre[0] @r }
            return
        }
    }

    $id = [guid]::NewGuid().ToString()

    if ($env:TMUX -and $env:TMUX_PANE) {
        # TMUX herdado pode estar MORTO (terminal reaproveitado de um pane que ja fechou).
        # list-panes sai != 0 pra pane inexistente; stale -> limpa e abre pelo Hangar.
        tmux list-panes -t $env:TMUX_PANE 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { $env:TMUX = $null; $env:TMUX_PANE = $null }
    }

    # Fora do multiplexador e interativo: a sessao nasce pelo lancador do Hangar
    # (`hangar-server open`), igual as do app, e o terminal anexa nela. Mesmas regras do
    # claude.posix.sh / scripts/hangar-open: argumento que o pedido nao cobre, ou sem
    # hangar-server -> sessao no psmux abaixo; recusa -> erro, sem segunda abertura.
    if (-not $env:TMUX -and -not $modoPrint -and -not [Console]::IsInputRedirected) {
        $pedido = [ordered]@{ provider = 'claude'; cwd = (Get-Location).ProviderPath; headless = $false }
        if ($env:CLAUDE_CONFIG_DIR) { $pedido.config_dir = $env:CLAUDE_CONFIG_DIR }
        if ($env:CP_ENGINE) { $pedido.engine = $env:CP_ENGINE }
        $chaves = @{ '--model' = 'model'; '--effort' = 'effort'; '--permission-mode' = 'permission_mode' }
        $cobre = $true
        # Subcomando do claude (`claude --help`, secao Commands) so vale como PRIMEIRO argumento e
        # roda cru: virar sessao faria dele o prompt inicial. Mesma lista do scripts/hangar-open.
        $subcomandos = @('agents', 'attach', 'auth', 'auto-mode', 'doctor', 'gateway', 'import', 'install',
                         'logs', 'mcp', 'plugin', 'plugins', 'purge', 'respawn', 'rm', 'setup-token', 'stop',
                         'kill', 'ultrareview', 'update', 'upgrade')
        if ($args.Count -gt 0 -and $subcomandos -ccontains [string]$args[0]) { $cobre = $false }
        $barrou = $null
        for ($i = 0; $cobre -and $i -lt $args.Count; $i++) {
            $a = [string]$args[$i]
            $flag, $valor = $a -split '=', 2
            if ($chaves.ContainsKey($flag)) {
                if ($null -eq $valor) {
                    $i++
                    if ($i -ge $args.Count) { $barrou = $a; break }
                    $valor = [string]$args[$i]
                }
                $pedido[$chaves[$flag]] = $valor
            } elseif ($a -eq '--dangerously-skip-permissions') {
                $pedido.permission_mode = 'bypassPermissions'
            } elseif ($a.StartsWith('-') -or $pedido.Contains('initial_prompt')) {
                $barrou = $a; break
            } else {
                $pedido.initial_prompt = $a
            }
        }
        if ($null -ne $barrou) {
            $cobre = $false
            [Console]::Error.WriteLine("hangar: o argumento '$barrou' nao cabe no pedido do Hangar; abrindo o claude sem o Hangar")
        }
        if ($cobre) {
            # A regra unica dos atalhos (scripts/hangar-server-lookup): CP_RUST_SERVER_BIN vence e nao
            # cai para outro; sem ela, o bin\ da versao em uso pelo caminho real do current; senao o
            # mais novo entre o build desta copia e o baixado (empate fica com a copia).
            $servidor = $null
            $melhor = $null
            $escolhido = "$env:CP_RUST_SERVER_BIN".Trim()
            if ($escolhido) {
                if ($escolhido -eq '~' -or $escolhido.StartsWith('~\') -or $escolhido.StartsWith('~/')) { $escolhido = $HOME + $escolhido.Substring(1) }
                if (Test-Path -LiteralPath $escolhido -PathType Leaf) { $servidor = $escolhido }
            } else {
                # A troca no Windows deixa so o `current.old` por um instante: espera o link novo ate 1 s.
                $atual = Join-Path $HOME '.hangar\current'
                for ($espera = 0; $espera -lt 20 -and -not (Test-Path -LiteralPath $atual) -and (Test-Path -LiteralPath "$atual.old"); $espera++) { Start-Sleep -Milliseconds 50 }
                $doPacote = Join-Path $atual 'bin\hangar-server.exe'
                if (Test-Path -LiteralPath $doPacote -PathType Leaf) {
                    $item = Get-Item -LiteralPath $atual
                    $real = if ($item.LinkType -and $item.Target) { @($item.Target)[0] } else { $item.FullName }
                    $servidor = Join-Path $real 'bin\hangar-server.exe'
                } else {
                    foreach ($c in @((Join-Path $PSScriptRoot '..\..\crates\target\release\hangar-server.exe'),
                                     (Join-Path $HOME '.hangar\bin\hangar-server.exe'))) {
                        if (Test-Path -LiteralPath $c -PathType Leaf) {
                            $item = Get-Item -LiteralPath $c
                            if (-not $melhor -or $item.LastWriteTimeUtc -gt $melhor.LastWriteTimeUtc) { $melhor = $item }
                        }
                    }
                    if ($melhor) { $servidor = $melhor.FullName }
                }
            }
            if (-not $servidor) {
                [Console]::Error.WriteLine('hangar: hangar-server nao encontrado; abrindo o claude sem o Hangar')
            } else {
                # Pela entrada padrao, nao como argumento: o PowerShell 5.1 come as aspas de dentro
                # do JSON ao chamar um executavel. UTF-8 pra pasta com acento chegar inteira.
                $OutputEncoding = New-Object System.Text.UTF8Encoding $false
                $saida = ($pedido | ConvertTo-Json -Compress) | & $servidor open
                $codigo = $LASTEXITCODE
                if ($codigo -eq 0) {
                    $nome = (($saida -join "`n") | ConvertFrom-Json).name
                    if (-not $nome) { Write-Error "hangar: resposta do hangar-server sem o nome da sessao: $saida"; return }
                    # Nome cru: o psmux nao honra `=` em todo comando e o nome ja e exato la.
                    tmux attach-session -t $nome
                    return
                }
                # 2 = hangar-server antigo, sem `open`; 4 = sem token no .env. Nada foi enviado: segue cru.
                if ($codigo -eq 2 -or $codigo -eq 4) {
                    [Console]::Error.WriteLine("hangar: o hangar-server nao abriu a sessao (saida $codigo); abrindo o claude sem o Hangar")
                } else {
                    # Saida 3 (resolvido mas nao aberto aqui) traz o spec no stdout.
                    if ($saida) { [Console]::Error.WriteLine(($saida -join "`n")) }
                    $global:LASTEXITCODE = $codigo
                    return
                }
            }
        }

        # O Hangar nao abriu: sem o psmux o claude ficaria invisivel pro app. Subcomando roda cru.
        if ($args.Count -eq 0 -or $subcomandos -cnotcontains [string]$args[0]) {
            # Sessao com o nome da pasta, unica. FormD + descarte da marca = acento vira ASCII,
            # mesma regra do backend (app/names.py).
            $base = Split-Path -Leaf (Get-Location).ProviderPath
            $base = -join ($base.Normalize([Text.NormalizationForm]::FormD).ToCharArray() | Where-Object {
                [Globalization.CharUnicodeInfo]::GetUnicodeCategory($_) -ne 'NonSpacingMark'
            })
            $base = ($base -replace '[^A-Za-z0-9_-]', '-').Trim('-')
            if (-not $base) { $base = 'session' }
            $nome = $base; $n = 2
            while ($true) {
                # `=`: match exato; sem ele "proj-2" viva responderia por "proj".
                tmux has-session -t "=$nome" 2>&1 | Out-Null
                if ($LASTEXITCODE -ne 0) { break }
                $nome = "$base-$n"; $n++
            }
            # So com a variavel: no psmux o pane herda o ambiente de quem chama, e
            # CLAUDE_CONFIG_DIR apontando pro padrao faria o Claude ler o .claude.json errado.
            if ($env:CLAUDE_CONFIG_DIR) { $cfg = @('-e', "CLAUDE_CONFIG_DIR=$($env:CLAUDE_CONFIG_DIR)") }
            else { $cfg = @() }
            # CP_SESSION_NAME: identidade da sessao pro hangar-send de dentro do pane.
            $cmd = @('tmux', 'new-session', '-s', $nome, '-c', (Get-Location).ProviderPath,
                     '-e', 'COLORTERM=truecolor', '-e', 'CLAUDE_CODE_TMUX_TRUECOLOR=1',
                     '-e', "CP_SESSION_NAME=$nome") +
                   $cfg + $pre + @('claude', '--session-id', $id) + $plug + $args
            $r = @($cmd[1..($cmd.Count - 1)])
            & $cmd[0] @r
            return
        }
    }

    # Claude cru com o id: ja dentro do multiplexador, modo -p, entrada redirecionada ou subcomando.
    $env:COLORTERM = 'truecolor'; $env:CLAUDE_CODE_TMUX_TRUECOLOR = '1'
    if ($pre.Count -eq 0) { & $claudeExe --session-id $id @plug @args }
    else { $r = @($pre[1..($pre.Count - 1)]) + @('claude', '--session-id', $id) + $plug + $args
           & $pre[0] @r }
}
