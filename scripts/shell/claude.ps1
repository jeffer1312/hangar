# Wrapper do `claude` pro PowerShell - equivalente do scripts/shell/claude.posix.sh.
#
# Sem ele, um `claude` aberto por voce no terminal e INVISIVEL pro app: sem --session-id o
# backend nao sabe qual transcript .jsonl e daquela sessao, e fora do multiplexador nao ha pane
# pra ler estado nem receber input. Com ele, `claude` na pasta do projeto abre uma sessao no
# psmux, nomeada pela pasta, com id proprio - igual ao Linux.
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

    # Ja veio com id/retomada explicita? Nao inventa outro - repassa como esta. Injetar um
    # --session-id por cima de um --resume abriria uma conversa NOVA no lugar da pedida.
    foreach ($a in $args) {
        if ($a -match '^(--session-id|--resume)(=|$)' -or $a -eq '-c' -or $a -eq '--continue') {
            if ($pre.Count -eq 0) { & $claudeExe @args }
            else { $r = @($pre[1..($pre.Count - 1)]) + @('claude') + $args; & $pre[0] @r }
            return
        }
    }

    $id = [guid]::NewGuid().ToString()

    # So injeta o id (sem criar sessao) quando: ja estamos dentro do multiplexador, modo -p, ou
    # a entrada nao e um terminal (pipe/redirecionamento) - nesses casos criar sessao atrapalha.
    $modoPrint = $false
    foreach ($a in $args) { if ($a -eq '-p' -or $a -eq '--print') { $modoPrint = $true } }

    if ($env:TMUX -and $env:TMUX_PANE) {
        # TMUX herdado pode estar MORTO (terminal reaproveitado de um pane que ja fechou).
        # list-panes sai != 0 pra pane inexistente; stale -> limpa e cai no caminho de criar.
        tmux list-panes -t $env:TMUX_PANE 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { $env:TMUX = $null; $env:TMUX_PANE = $null }
    }

    if ($env:TMUX -or $modoPrint -or [Console]::IsInputRedirected) {
        $env:COLORTERM = 'truecolor'; $env:CLAUDE_CODE_TMUX_TRUECOLOR = '1'
        if ($pre.Count -eq 0) { & $claudeExe --session-id $id @args }
        else { $r = @($pre[1..($pre.Count - 1)]) + @('claude', '--session-id', $id) + $args
               & $pre[0] @r }
        return
    }

    # Fora do multiplexador e interativo: cria sessao com o nome da pasta, unico.
    $base = Split-Path -Leaf (Get-Location).Path
    # Acento vira o ASCII equivalente ANTES do filtro (mesma regra do backend, app/names.py):
    # sem isto "?rea de Trabalho" perderia a primeira letra. FormD separa a letra do acento e o
    # filtro seguinte descarta a marca, que e o que o iconv //TRANSLIT faz no Linux.
    $base = -join ($base.Normalize([Text.NormalizationForm]::FormD).ToCharArray() | Where-Object {
        [Globalization.CharUnicodeInfo]::GetUnicodeCategory($_) -ne 'NonSpacingMark'
    })
    $base = ($base -replace '[^A-Za-z0-9_-]', '-').Trim('-')
    if (-not $base) { $base = 'session' }

    $nome = $base; $i = 2
    while ($true) {
        # `=$nome`: match EXATO. Sem o '=', o alvo do tmux cai em match por PREFIXO e "proj-2"
        # viva responderia "existe" pra "proj" - o loop nunca acharia um nome livre.
        tmux has-session -t "=$nome" 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { break }
        $nome = "$base-$i"; $i++
    }

    # Conta Claude (claude-conta setou CLAUDE_CONFIG_DIR): psmux NAO herda o env de quem chama —
    # sem repassar aqui, a sessao nasceria na conta padrao, calada. Aqui pode ir por `-e` (ao
    # contrario da key do motor, que vai pelo `hangar-engine --exec` justamente pra nao aparecer em
    # /proc/<pid>/cmdline): isto e um caminho, nao um segredo.
    # SO quando o chamador tem a variavel. O padrao explicito ($HOME\.claude) que ficava aqui era
    # medido como ERRADO em 22/08/2026: pro Claude Code, CLAUDE_CONFIG_DIR setado — mesmo apontando
    # pro proprio ~/.claude — quer dizer "leia o .claude.json de DENTRO dessa pasta", que nasce
    # vazio; sem a variavel ele le o ~/.claude.json, o de verdade (onboarding, tema, historico).
    # Resultado do `-e` com o padrao: sessao na tela de boas-vindas ("Select login method") com a
    # credencial intacta, e o settings.json lido da pasta errada junto.
    # E omitir aqui NAO abre a porta pra conta vazar de outra sessao: no psmux o pane herda o
    # ambiente de QUEM CHAMA, e nada passa de uma sessao pra outra (medido no psmux 3.3.7 — a
    # mesma medicao esta em `_e_config_dir`, backend/app/tmux.py, que aplica esta regra do lado do
    # app). Chamador com a variavel (`claude-conta`) segue mandando o `-e`: e o unico jeito de a
    # conta chegar no pane, e ali o valor nunca e string vazia.
    if ($env:CLAUDE_CONFIG_DIR) { $cfg = @('-e', "CLAUDE_CONFIG_DIR=$($env:CLAUDE_CONFIG_DIR)") }
    else { $cfg = @() }

    # Sem `exec` e sem systemd-run, ao contrario do POSIX: nao ha shell intermediario dentro do
    # pane do psmux (ele roda o comando direto no ConPTY) e nao ha cgroup de servico pra escapar.
    # CP_SESSION_NAME: mesmo carimbo que o backend poe em new_session (app/tmux.py). Sem ele o
    # hangar-send de dentro da sessao cai no `display-message -p '#S'`, que devolve a sessao do CLIENTE
    # anexado e nao a de quem chama -> o --unpair de uma sessao desfazia o vinculo da OUTRA.
    $cmd = @('tmux', 'new-session', '-s', $nome, '-c', (Get-Location).Path,
             '-e', 'COLORTERM=truecolor', '-e', 'CLAUDE_CODE_TMUX_TRUECOLOR=1',
             '-e', "CP_SESSION_NAME=$nome") +
           $cfg + $pre + @('claude', '--session-id', $id) + $args
    $r = @($cmd[1..($cmd.Count - 1)])
    & $cmd[0] @r
}
