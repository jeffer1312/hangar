# Funcoes compartilhadas pelo instalador e pela vigia, sem iniciar processos ao carregar.
function Get-HangarProcessTable {
    $job = Start-Job {
        Get-CimInstance Win32_Process -ErrorAction Stop |
            Select-Object ProcessId, ParentProcessId, Name, CommandLine, CreationDate
    }
    try {
        if (-not (Wait-Job $job -Timeout 5)) { throw 'WMI nao respondeu; nenhuma instancia sera encerrada ou iniciada' }
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
            if ($row.Name -notmatch '^(python(?:\d+(?:\.\d+)*)?|uv)\.exe$' -or
                $row.CommandLine -notmatch '(?:^|\s)-m\s+app\.main(?:\s|$)') { continue }
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

function Test-HangarHttp([int]$port) {
    $response = $null
    try {
        $request = [Net.HttpWebRequest]::Create("http://127.0.0.1:$port/")
        $request.Proxy = $null
        $request.Timeout = 3000
        $request.ReadWriteTimeout = 3000
        $request.AllowAutoRedirect = $false
        $response = $request.GetResponse()
        return ([int]$response.StatusCode -lt 500)
    } catch [Net.WebException] {
        $response = $_.Exception.Response
        # Ate um 404 prova resposta HTTP quando o dist ainda nao esta instalado.
        return ($null -ne $response -and [int]$response.StatusCode -lt 500)
    } finally { if ($response) { $response.Close() } }
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
    if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue) { return $true }
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
        # O hangar-server e filho do backend e segura a porta publica: entra na parada depois do
        # pai (ele sai sozinho quando o cano do stdin fecha; se ainda estiver vivo, cai aqui).
        $candidates = @($candidates) + @($rows | Where-Object {
            $_.Name -eq 'hangar-server.exe' -and [int]$_.ParentProcessId -in @($candidates.ProcessId)
        })
        $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue)
        foreach ($listener in $listeners) {
            if ($listener.OwningProcess -notin @($candidates.ProcessId)) {
                if (Test-HangarServedFront $rows $name @($listener)) { continue }
                $quem = Format-HangarPortOwner $rows $listener
                throw "Porta $port ocupada por outro processo ($quem); $name nao sera reiniciada. Se nao for seu, encerre: Stop-Process -Id $($listener.OwningProcess) -Force"
            }
        }
        if ($task.State -eq 'Running' -and -not $candidates.Count) {
            throw "Tarefa $name em execucao sem processo identificado; nao e seguro reiniciar"
        }
        # NAO se pausa o reinicio automatico aqui, por dois motivos medidos (12/09/2026).
        # 1) Nao da: zerar o RestartCount por Set-ScheduledTask produz um <RestartOnFailure> com
        #    <Interval> e sem <Count>, e o Agendador recusa o XML INTEIRO - HRESULT 0x80041319,
        #    "Um elemento ou atributo necessario esta faltando no XML da tarefa. (43,8):Count:".
        #    Como as duas tarefas nascem com -RestartCount 3, isto falhava em 100% das chamadas:
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
            $held = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue)
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
        # O front fica no inicio direto: quem o recupera e a vigia.
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

# Reinicio das tarefas pelo atualizador, sem o instalador. Mesmo mutex do install.ps1 e da vigia:
# os tres controlam a parada das mesmas tarefas e nao podem se sobrepor.
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
            catch { Write-Warning "hangar-frontend nao reiniciou, a vigia recupera: $_" }
        }
    } finally {
        if ($locked) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}

function Repair-HangarTask([string]$name, [int]$port, [string]$directory) {
    $mutex = New-Object Threading.Mutex($false, 'Local\HangarInstall')
    $locked = $false
    try {
    try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
    if (-not $locked) { return }
    if (Test-HangarHttp $port) { return }
    Start-Sleep -Seconds 2
    if (Test-HangarHttp $port) { return }
    $task = Get-ScheduledTask -TaskName $name -ErrorAction Stop
    if (-not $task.Settings.Enabled) { return }
    $info = Get-ScheduledTaskInfo -TaskName $name -ErrorAction Stop
    if ($info.LastRunTime -gt (Get-Date).AddMinutes(-10)) { return }
    $rows = Get-HangarProcessTable
    # Instalacao e atualizacao controlam a propria parada, inclusive quando a API nao responde.
    if ($rows | Where-Object {
        ($_.Name -match '^python.*\.exe$' -and $_.CommandLine -match '(?:^|\s)-m\s+app\.atualizar(?:\s|$)') -or
        ($_.Name -eq 'hangar-server.exe' -and $_.CommandLine -match '(?:^|\s)(?:update|checkout-update|migrate-package|dev-mode)(?:\s|$)') -or
        ($_.Name -match '^(powershell|pwsh)\.exe$' -and $_.CommandLine -match '(?:^|[\s\\/])install\.ps1(?:["\s]|$)')
    }) { return }
    $candidates = @(Get-HangarTaskProcesses $rows $name $directory)
    if ($candidates | Where-Object { $_.CreationDate -gt (Get-Date).AddMinutes(-10) }) { return }
    Write-Output "$(Get-Date -Format s) vigia: $name sem resposta HTTP; recuperando a instancia identificada"
    Restart-HangarTask $name $port $directory
    } finally {
        if ($locked) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}
