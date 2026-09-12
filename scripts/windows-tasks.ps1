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
    foreach ($row in $rows) {
        if ($name -eq 'hangar-backend') {
            if ($row.Name -notmatch '^(python(?:\d+(?:\.\d+)*)?|uv)\.exe$' -or
                $row.CommandLine -notmatch '(?:^|\s)-m\s+app\.main(?:\s|$)') { continue }
        } elseif ($row.Name -notmatch '^(node|npm|vite)\.exe$') { continue }
        $current = $row
        $seen = New-Object 'System.Collections.Generic.HashSet[int]'
        while ($current -and $seen.Add([int]$current.ProcessId)) {
            if ($current.CommandLine -match $pathPattern -or
                ($current.Name -eq 'wscript.exe' -and $current.CommandLine -match $launcherPattern)) {
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

function Restart-HangarTask([string]$name, [int]$port, [string]$directory) {
    $mutex = New-Object Threading.Mutex($false, "Local\HangarRestart-$name")
    $locked = $false
    $settings = $null
    try {
        try { $locked = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $locked = $true }
        if (-not $locked) { throw "Outra recuperacao de $name esta em andamento" }
        $task = Get-ScheduledTask -TaskName $name -ErrorAction Stop
        if (-not $task.Settings.Enabled) { throw "A tarefa $name esta desabilitada" }
        $rows = Get-HangarProcessTable
        $candidates = @(Get-HangarTaskProcesses $rows $name $directory)
        $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue)
        foreach ($listener in $listeners) {
            if ($listener.OwningProcess -notin @($candidates.ProcessId)) {
                throw "Porta $port ocupada por outro processo; $name nao sera reiniciada"
            }
        }
        if ($task.State -eq 'Running' -and -not $candidates.Count) {
            throw "Tarefa $name em execucao sem processo identificado; nao e seguro reiniciar"
        }
        $settings = $task.Settings
        $restartCount = $settings.RestartCount
        $settings.RestartCount = 0
        Set-ScheduledTask -TaskName $name -Settings $settings -ErrorAction Stop | Out-Null
        foreach ($candidate in $candidates) {
            $process = Get-Process -Id $candidate.ProcessId -ErrorAction SilentlyContinue
            if (-not $process) { continue }
            # WMI e GetProcessTimes devolvem o nascimento com precisoes diferentes.
            if (-not $candidate.CreationDate -or
                [Math]::Abs(($process.StartTime.ToUniversalTime() - $candidate.CreationDate.ToUniversalTime()).Ticks) -ge [TimeSpan]::TicksPerMillisecond) {
                throw "Identidade do processo $($candidate.ProcessId) mudou; parada cancelada"
            }
            # Nao encerra a arvore: psmux e o atualizador precisam sobreviver ao backend.
            Stop-Process -InputObject $process -Force -ErrorAction Stop
            if (-not $process.WaitForExit(5000)) { throw "Processo $($candidate.ProcessId) nao encerrou" }
        }
        foreach ($attempt in 1..50) {
            if ((Get-ScheduledTask -TaskName $name -ErrorAction Stop).State -ne 'Running') { break }
            Start-Sleep -Milliseconds 100
        }
        if ((Get-ScheduledTask -TaskName $name -ErrorAction Stop).State -eq 'Running') {
            throw "Lancador de $name ainda esta rodando; nova instancia nao sera iniciada"
        }
        if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue) {
            throw "Porta $port ainda ocupada; nova instancia nao sera iniciada"
        }
        Start-ScheduledTask -TaskName $name -ErrorAction Stop
    } finally {
        try {
            if ($settings) {
                $settings.RestartCount = $restartCount
                Set-ScheduledTask -TaskName $name -Settings $settings -ErrorAction Stop | Out-Null
            }
        } finally {
            if ($locked) { $mutex.ReleaseMutex() }
            $mutex.Dispose()
        }
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
