# Funcoes compartilhadas pelo instalador e pelo atualizador, sem iniciar processos ao carregar.
function Get-HangarProcessTable {
    $job = Start-Job {
        Get-CimInstance Win32_Process -ErrorAction Stop |
            Select-Object ProcessId, ParentProcessId, Name, CommandLine, CreationDate
    }
    try {
        # O Job abre um PowerShell novo, que em maquina lenta gasta segundos preparando modulos antes da consulta.
        if (-not (Wait-Job $job -Timeout 30)) { throw 'WMI nao respondeu em 30 s; nenhuma instancia sera encerrada ou iniciada' }
        $rows = @(Receive-Job $job -ErrorAction Stop)
        if (-not $rows.Count) { throw 'Tabela de processos vazia; nao e seguro recuperar a tarefa' }
        return $rows
    } finally { Remove-Job $job -Force -ErrorAction SilentlyContinue }
}

function Get-HangarTaskProcesses($rows, [string]$name, [string]$directory) {
    if ($name -notin @('hangar-backend', 'hangar-frontend') -or [string]::IsNullOrWhiteSpace($directory)) {
        throw 'Tarefa ou diretorio do Hangar invalido; selecao de processos cancelada'
    }
    $byId = @{}
    foreach ($row in $rows) { $byId[[int]$row.ProcessId] = $row }
    $pathPattern = [regex]::Escape($directory.TrimEnd('\', '/')) + '(?:[\\/]|["\s]|$)'
    $launcherPattern = [regex]::Escape((Join-Path $env:LOCALAPPDATA "hangar\$name.vbs"))
    # No pacote o Python roda do caminho real da versao, que nao e o diretorio pedido: quem o
    # identifica e o lancador `hangar-server launch backend` acima dele.
    $packagePattern = '(?:^|\s)launch\s+backend(?:\s|$)'
    foreach ($row in $rows) {
        if ($name -eq 'hangar-backend') {
            $python = $row.Name -match '^(python(?:\d+(?:\.\d+)*)?|uv)\.exe$' -and
                      $row.CommandLine -match '(?:^|\s)-m\s+app\.main(?:\s|$)'
            # Versao que sobe sem o Python: o hangar-server filho direto do lancador `launch backend`.
            $launcher = $byId[[int]$row.ParentProcessId]
            $direct = $row.Name -eq 'hangar-server.exe' -and $row.CommandLine -notmatch $packagePattern -and
                      $launcher -and $launcher.Name -match '^hangar-(?:server|launcher)\.exe$' -and
                      $launcher.CommandLine -match $packagePattern -and $launcher.CreationDate -le $row.CreationDate
            if (-not ($python -or $direct)) { continue }
        } elseif ($row.Name -notmatch '^(node|npm|vite)\.exe$') { continue }
        $current = $row
        $seen = New-Object 'System.Collections.Generic.HashSet[int]'
        while ($current -and $seen.Add([int]$current.ProcessId)) {
            if ($current.CommandLine -match $pathPattern -or
                ($current.Name -eq 'wscript.exe' -and $current.CommandLine -match $launcherPattern) -or
                ($name -eq 'hangar-backend' -and $current.Name -match '^hangar-(?:server|launcher)\.exe$' -and
                 $current.CommandLine -match $packagePattern)) {
                $row
                break
            }
            $parent = $byId[[int]$current.ParentProcessId]
            if ($parent -and $parent.CreationDate -gt $current.CreationDate) { break }
            $current = $parent
        }
    }
}

# Decisao 38: o lancador do pacote fica de pe e religa o hangar-server que cai; entre a queda e a
# religada ele e o unico processo da tarefa. Entra na parada o lancador pai de um candidato ou o
# filho do wscript desta tarefa; o de outra instalacao, nao.
function Get-HangarBackendLaunchers($rows, $candidates, [string]$name) {
    if ($name -ne 'hangar-backend') { return }
    $byId = @{}
    foreach ($row in $rows) { $byId[[int]$row.ProcessId] = $row }
    $vbs = [regex]::Escape((Join-Path $env:LOCALAPPDATA "hangar\$name.vbs"))
    $taken = @($candidates | ForEach-Object { [int]$_.ProcessId })
    $parents = @($candidates | ForEach-Object { [int]$_.ParentProcessId })
    foreach ($row in $rows) {
        if ($row.Name -notmatch '^hangar-(?:server|launcher)\.exe$' -or
            $row.CommandLine -notmatch '(?:^|\s)launch\s+backend(?:\s|$)' -or [int]$row.ProcessId -in $taken) { continue }
        $parent = $byId[[int]$row.ParentProcessId]
        $fromTask = $parent -and $parent.Name -eq 'wscript.exe' -and $parent.CommandLine -match $vbs -and
                    $parent.CreationDate -le $row.CreationDate
        if ([int]$row.ProcessId -in $parents -or $fromTask) { $row }
    }
}

# Prova do passo de maquina: a tarefa do backend religa sempre e sobe pelo `launch backend`. Devolve
# o motivo da reprovacao, ou nada. Sem a tarefa nao ha o que conferir. O .vbs e UTF-16LE sem BOM (os
# bytes do GetBytes): lido como UTF-8, o texto nunca casaria.
function Get-HangarBackendTaskProblem([string]$vbs) {
    $task = Get-ScheduledTask -TaskName 'hangar-backend' -ErrorAction SilentlyContinue
    if (-not $task) { return }
    if ($task.Settings.RestartCount -ne 999) { return "a tarefa hangar-backend religa $($task.Settings.RestartCount) vezes, nao 999" }
    if (-not (Test-Path -LiteralPath $vbs)) { return "$vbs nao existe" }
    if ([IO.File]::ReadAllText($vbs, [Text.Encoding]::Unicode) -notmatch '(?:^|\s)launch\s+backend\b') {
        return "$vbs nao abre o lancador pelo launch backend"
    }
}

function Get-HangarPortOwner($rows, $listener) {
    return $rows | Where-Object { [int]$_.ProcessId -eq [int]$listener.OwningProcess } | Select-Object -First 1
}

function Format-HangarPortOwner($rows, $listener) {
    $owner = Get-HangarPortOwner $rows $listener
    if ($owner) { return "$($owner.Name) PID $($owner.ProcessId): $($owner.CommandLine)" }
    return "PID $($listener.OwningProcess)"
}

# Em maquina migrada o hangar-server abre a porta do front e entrega a mesma tela: nao ha o que subir.
function Test-HangarServedFront($rows, [string]$name, $listeners) {
    if ($name -ne 'hangar-frontend' -or -not (Test-Path -LiteralPath "$env:USERPROFILE\.hangar\current")) { return $false }
    foreach ($listener in $listeners) {
        $owner = Get-HangarPortOwner $rows $listener
        if (-not $owner -or $owner.Name -ne 'hangar-server.exe') { return $false }
    }
    return $true
}

# A lista de portas do Windows ja respondeu vazia com o servidor atendendo; a saude do proprio
# hangar-server desempata (conexao crua aceitaria qualquer processo na porta).
function Test-HangarHealth([int]$port) {
    try {
        $request = [Net.WebRequest]::Create("http://127.0.0.1:$port/__hangar_server/health")
        $request.Proxy = $null
        $request.Timeout = 500
        $response = $request.GetResponse()
        try { return ([int]$response.StatusCode -eq 200) } finally { $response.Close() }
    } catch { return $false }
}
function Test-HangarPortOpen([int]$port) {
    # PID 0 e a leitura errada da lista: so a saude prova a subida.
    if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue | Where-Object { [int]$_.OwningProcess -ne 0 }) { return $true }
    return (Test-HangarHealth $port)
}

# Start-ScheduledTask volta sem erro mesmo quando nenhuma instancia abre, e o LastRunTime ja passou por
# inicio que nao deixou processo de pe: so a porta prova a subida. Repetir e seguro, o IgnoreNew descarta.
# Tarefa rodando sem porta e subida lenta: quem julga e a espera de quem pediu o reinicio.
# Cada tentativa sai numa linha `hangar-tarefa:`, que o atualizador leva ao diario: repetir esconde a falha.
function Start-HangarTask([string]$name, [int]$port) {
    $before = Get-ScheduledTaskInfo -TaskName $name -ErrorAction Stop
    $watch = [Diagnostics.Stopwatch]::StartNew()
    foreach ($attempt in 1..3) {
        Start-ScheduledTask -TaskName $name -ErrorAction Stop
        $stopped = 0
        foreach ($check in 1..40) {
            $open = Test-HangarPortOpen $port
            $state = (Get-ScheduledTask -TaskName $name -ErrorAction Stop).State
            if ($open) { break }
            if ($state -eq 'Running') { $stopped = 0 } elseif (++$stopped -ge 5) { break }
            Start-Sleep -Milliseconds 200
        }
        $after = Get-ScheduledTaskInfo -TaskName $name -ErrorAction Stop
        Write-Output ("hangar-tarefa: $name tentativa=$attempt porta=$(if ($open) { 'sim' } else { 'nao' }) estado=$state " +
            ("antes={0:s}/0x{1:X8} depois={2:s}/0x{3:X8} ms={4}" -f $before.LastRunTime, $before.LastTaskResult, $after.LastRunTime, $after.LastTaskResult, $watch.ElapsedMilliseconds))
        if ($open -or $state -eq 'Running') { return }
    }
    throw "Tarefa $name nao iniciou depois de 3 tentativas"
}

# Confere a identidade e para o processo. O que saiu sozinho entre a leitura e a parada (o
# hangar-server sai com o Python dele) conta como parado: o erro dele nao pode cancelar o inicio.
# Identidade divergente aborta sempre.
function Stop-HangarCandidate($candidate, $process) {
    try { $started = $process.StartTime }
    catch {
        $failure = $_
        if (Test-HangarExited $process) { return }
        throw $failure
    }
    # O PowerShell engole o erro da leitura e devolve nulo do processo que esta saindo (o hangar-server
    # morre logo depois do Python dele): espera a saida antes de chamar de identidade ilegivel.
    if ($null -eq $started) {
        foreach ($wait in 0..30) {
            if ((Test-HangarExited $process) -or -not (Get-Process -Id $candidate.ProcessId -ErrorAction SilentlyContinue)) { return }
            if ($wait -lt 30) { Start-Sleep -Milliseconds 100 }
        }
        throw "Identidade do processo $($candidate.ProcessId) ilegivel; parada cancelada"
    }
    # WMI e GetProcessTimes devolvem o nascimento com precisoes diferentes.
    if (-not $candidate.CreationDate -or
        [Math]::Abs(($started.ToUniversalTime() - $candidate.CreationDate.ToUniversalTime()).Ticks) -ge [TimeSpan]::TicksPerMillisecond) {
        throw "Identidade do processo $($candidate.ProcessId) mudou; parada cancelada"
    }
    try { Stop-Process -InputObject $process -Force -ErrorAction Stop }
    catch {
        $failure = $_
        if (-not (Test-HangarExited $process)) { throw $failure }
    }
}

# HasExited ilegivel (processo sem direito de consulta) nao prova que saiu.
function Test-HangarExited($process) { try { return [bool]$process.HasExited } catch { return $false } }

# -NoStart: so para (a migracao para o pacote troca o lancador entre parar e subir).
function Restart-HangarTask([string]$name, [int]$port, [string]$directory, [switch]$NoStart) {
    $mutex = New-Object Threading.Mutex($false, "Local\HangarRestart-$name")
    $locked = $false
    try {
        try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
        if (-not $locked) { throw "Outra recuperacao de $name esta em andamento" }
        $task = Get-ScheduledTask -TaskName $name -ErrorAction Stop
        if (-not $task.Settings.Enabled) { throw "A tarefa $name esta desabilitada" }
        $rows = Get-HangarProcessTable
        $candidates = @(Get-HangarTaskProcesses $rows $name $directory)
        # Na versao pelo Python o hangar-server e filho dele e segura a porta publica: entra na parada
        # depois do pai (ele sai sozinho quando o cano do stdin fecha; se ainda estiver vivo, cai aqui).
        # So filho do Python: filho do hangar-server direto e o executor da atualizacao, que chamou isto.
        $pythonIds = @($candidates | Where-Object { $_.Name -match '^(python(?:\d+(?:\.\d+)*)?|uv)\.exe$' } |
            ForEach-Object { [int]$_.ProcessId })
        $candidates = @($candidates) + @($rows | Where-Object {
            $_.Name -eq 'hangar-server.exe' -and [int]$_.ParentProcessId -in $pythonIds
        })
        # Por ultimo: parado o servidor, o lancador sai sozinho; parado o lancador, o Job leva o resto.
        $candidates = @($candidates) + @(Get-HangarBackendLaunchers $rows $candidates $name)
        $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue)
        foreach ($listener in $listeners) {
            # O Windows ja deu o PID 0 como dono da porta do servidor vivo: num endereco que o hangar-server
            # abre e com a saude dele respondendo em 127.0.0.1, nao e outro programa (quem abrisse ali
            # responderia a saude no lugar dele).
            if ([int]$listener.OwningProcess -eq 0 -and "$($listener.LocalAddress)" -in @('0.0.0.0', '::', '127.0.0.1') -and (Test-HangarHealth $port)) { continue }
            if ($listener.OwningProcess -notin @($candidates.ProcessId)) {
                if (Test-HangarServedFront $rows $name @($listener)) { continue }
                $quem = Format-HangarPortOwner $rows $listener
                $dica = if ([int]$listener.OwningProcess -eq 0) { 'O Windows nao informa o dono; confira com netstat -ano' }
                        else { "Se nao for seu, encerre: Stop-Process -Id $($listener.OwningProcess) -Force" }
                throw "Porta $port ocupada por outro processo ($quem); $name nao sera reiniciada. $dica"
            }
        }
        if ($task.State -eq 'Running' -and -not $candidates.Count) {
            throw "Tarefa $name em execucao sem processo identificado; nao e seguro reiniciar"
        }
        # NAO se pausa o reinicio automatico aqui, por dois motivos medidos (12/09/2026).
        # 1) Nao da: zerar o RestartCount por Set-ScheduledTask produz um <RestartOnFailure> com
        #    <Interval> e sem <Count>, e o Agendador recusa o XML INTEIRO - HRESULT 0x80041319,
        #    "Um elemento ou atributo necessario esta faltando no XML da tarefa. (43,8):Count:".
        #    Como as duas tarefas nascem com -RestartCount 999, isto falhava em 100% das chamadas:
        #    o Restart-HangarTask morria ANTES de parar qualquer coisa, o passo 7/8 do instalador
        #    virava pendencia ("nenhuma tarefa chegou a ser iniciada") e a vigia nunca recuperou
        #    travamento nenhum. Set-ScheduledTask tambem nao consegue REMOVER o bloco (ele funde
        #    com o XML existente e o Interval sobrevive sozinho); so re-registrar o XML tira, e
        #    isso e o que o instalador faz na janela LONGA dele (Suspender-Recuperacao).
        # 2) Nao precisa: a parada/inicio abaixo leva segundos e, se o reinicio por falha do
        #    Agendador chegar depois, o MultipleInstances=IgnoreNew descarta a instancia extra.
        #    Ele nao substitui o Start-HangarTask: processo morto sem falha de lancamento nao o aciona.
        foreach ($candidate in $candidates) {
            $process = Get-Process -Id $candidate.ProcessId -ErrorAction SilentlyContinue
            if (-not $process) { continue }
            # Nao encerra a arvore: psmux e o atualizador precisam sobreviver ao backend.
            Stop-HangarCandidate $candidate $process
            if (-not $process.WaitForExit(5000)) { throw "Processo $($candidate.ProcessId) nao encerrou" }
        }
        foreach ($attempt in 1..50) {
            if ((Get-ScheduledTask -TaskName $name -ErrorAction Stop).State -ne 'Running') { break }
            Start-Sleep -Milliseconds 100
        }
        if ((Get-ScheduledTask -TaskName $name -ErrorAction Stop).State -eq 'Running') {
            throw "Lancador de $name ainda esta rodando; nova instancia nao sera iniciada"
        }
        $held = @()
        foreach ($attempt in 1..10) {
            # PID 0 depois da parada e a mesma leitura errada de antes dela: quem segura a porta de fato
            # faz o bind exclusivo do servidor novo falhar, e a espera da versao nova acusa a falha.
            $held = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue |
                Where-Object { [int]$_.OwningProcess -ne 0 })
            if (-not $held.Count) { break }
            Start-Sleep -Milliseconds 500
        }
        if ($held.Count) {
            # WMI fora do ar nao pode trocar o motivo: fica a tabela lida antes da parada.
            try { $rows = Get-HangarProcessTable } catch { }
            if (Test-HangarServedFront $rows $name $held) { return }
            $quem = Format-HangarPortOwner $rows $held[0]
            throw "Porta $port ainda ocupada por $quem; nova instancia nao sera iniciada"
        }
        # O front fica no inicio direto, sem a espera da porta.
        if (-not $NoStart) {
            if ($name -eq 'hangar-backend') { Start-HangarTask $name $port } else { Start-ScheduledTask -TaskName $name -ErrorAction Stop }
        }
    } finally {
        if ($locked) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}

# Para o front SEM subir de novo, para o `npm ci` do atualizador: ele apaga o node_modules antes de
# reinstalar, e o Windows nao deixa apagar binario nativo mapeado em processo vivo. Quem sobe a
# tarefa depois e o Restart-HangarTasks.
function Stop-HangarFrontend([string]$raiz) {
    if (-not (Get-ScheduledTask -TaskName 'hangar-frontend' -ErrorAction SilentlyContinue)) { return }
    $mutex = New-Object Threading.Mutex($false, 'Local\HangarRestart-hangar-frontend')
    $locked = $false
    try {
        try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
        if (-not $locked) { throw 'Outra recuperacao de hangar-frontend esta em andamento' }
        $rows = Get-HangarProcessTable
        foreach ($candidate in @(Get-HangarTaskProcesses $rows 'hangar-frontend' (Join-Path $raiz 'frontend'))) {
            $process = Get-Process -Id $candidate.ProcessId -ErrorAction SilentlyContinue
            if (-not $process) { continue }
            Stop-HangarCandidate $candidate $process
            if (-not $process.WaitForExit(5000)) { throw "Processo $($candidate.ProcessId) nao encerrou" }
        }
    } finally {
        if ($locked) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}

# Reinicio das tarefas pelo atualizador, sem o instalador. Mesmo mutex do install.ps1: os dois
# controlam a parada das mesmas tarefas e nao podem se sobrepor.
function Restart-HangarTasks([string]$raiz, [int]$portaBack, [int]$portaFront) {
    $mutex = New-Object Threading.Mutex($false, 'Local\HangarInstall')
    $locked = $false
    try {
        try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
        if (-not $locked) { throw 'Instalacao ou recuperacao em andamento; tente de novo em instantes' }
        Restart-HangarTask 'hangar-backend' $portaBack (Join-Path $raiz 'backend')
        if ($portaFront -gt 0 -and (Get-ScheduledTask -TaskName 'hangar-frontend' -ErrorAction SilentlyContinue)) {
            # Com o backend novo de pe, erro aqui faria o atualizador desfazer uma versao boa.
            try { Restart-HangarTask 'hangar-frontend' $portaFront (Join-Path $raiz 'frontend') }
            catch { Write-Warning "hangar-frontend nao reiniciou: $_" }
        }
    } finally {
        if ($locked) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}
