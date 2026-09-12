$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/windows-tasks.ps1"
function Assert($value, $message) { if (-not $value) { throw $message } }
$testLocalData = $env:LOCALAPPDATA
if (-not $env:LOCALAPPDATA) { $env:LOCALAPPDATA = [IO.Path]::GetTempPath() }
$old = (Get-Date).AddMinutes(-20)
function Row($id, $parent, $name, $command) {
    [pscustomobject]@{ ProcessId = $id; ParentProcessId = $parent; Name = $name; CommandLine = $command; CreationDate = $old }
}
$rows = @(
    (Row 10 1 'python.exe' 'C:\repo\backend\.venv\Scripts\python.exe -m app.main'),
    (Row 11 10 'python.exe' 'C:\uv\python.exe -m app.main'),
    (Row 20 11 'python.exe' 'C:\repo\backend\.venv\Scripts\python.exe -m app.atualizar'),
    (Row 21 20 'python.exe' 'C:\uv\python.exe -m app.atualizar'),
    (Row 30 11 'psmux.exe' 'psmux server'),
    (Row 31 30 'python.exe' 'C:\repo\backend\tool.py'),
    (Row 40 1 'python.exe' 'C:\other\backend\.venv\Scripts\python.exe -m app.main'),
    (Row 41 1 'python.exe' 'C:\repo\backend-other\.venv\Scripts\python.exe -m app.main')
)
Assert (((Get-HangarTaskProcesses $rows 'hangar-backend' 'C:\repo\backend').ProcessId -join ',') -eq '10,11') 'Selecao atingiu atualizador, sessao ou outro checkout'

$script:events = @()
function Reset-State {
    $script:events = @()
    $script:task = [pscustomobject]@{ State = 'Running'; Settings = [pscustomobject]@{ Enabled = $true; RestartCount = 3 } }
    $script:live = @{}
    foreach ($id in @(10, 11)) {
        $process = [pscustomobject]@{ Id = $id; StartTime = $old }
        $process | Add-Member ScriptMethod WaitForExit { param($timeout) return $true }
        $script:live[$id] = $process
    }
    $script:owner = 11
    $script:stuckLauncher = $false
    $script:stopFails = $false
    $script:tableFails = $false
    $script:processRows = $rows
    $script:lastRun = $old
    $script:httpReplies = [Collections.Generic.Queue[bool]]::new()
    $script:httpReplies.Enqueue($false)
    $script:httpReplies.Enqueue($false)
}
function Get-HangarProcessTable {
    if ($script:tableFails) { throw 'WMI indisponivel' }
    return $script:processRows
}
function Get-ScheduledTask { param($TaskName, $ErrorAction) return $script:task }
function Get-ScheduledTaskInfo { param($TaskName, $ErrorAction) return @{ LastRunTime = $script:lastRun } }
function Set-ScheduledTask { param($TaskName, $Settings, $ErrorAction) $script:events += "settings:$($Settings.RestartCount)" }
function Get-NetTCPConnection { param($State, $LocalPort, $ErrorAction)
    if ($script:owner) { return @{ OwningProcess = $script:owner } }
}
function Get-Process { param($Id, $ErrorAction) return $script:live[[int]$Id] }
function Stop-Process { param($InputObject, [switch]$Force, $ErrorAction)
    if ($script:stopFails) { throw 'Acesso negado' }
    $script:events += "stop:$($InputObject.Id)"
    $script:live.Remove([int]$InputObject.Id)
    if ($script:owner -eq $InputObject.Id) { $script:owner = $null }
    if (-not $script:live.Count -and -not $script:stuckLauncher) { $script:task.State = 'Ready' }
}
function Start-ScheduledTask { param($TaskName, $ErrorAction)
    Assert (-not $script:live.Count -and -not $script:owner) 'Nova instancia iniciada antes de encerrar a antiga'
    $script:events += 'start'
    $script:task.State = 'Running'
}
function Start-Sleep { param($Seconds, $Milliseconds) }
function Test-HangarHttp { param($port) return $script:httpReplies.Dequeue() }

try {
    Reset-State
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    # Sem 'settings:*' na sequencia: o Restart-HangarTask nao mexe mais no RestartCount (o
    # Agendador recusa Count=0 e o IgnoreNew ja descarta o reinicio por falha atrasado - ver o
    # comentario no windows-tasks.ps1 e o scripts/test-windows-tasks-reais.ps1).
    Assert (($script:events -join ',') -eq 'stop:10,stop:11,start') 'Parada/reinicio fora de ordem'
    Assert ('settings:0' -notin $script:events) 'Voltou a pausar a recuperacao por Set-ScheduledTask (o Agendador recusa)'
    Reset-State
    $script:live[10].StartTime = $old.AddTicks(5)
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    Assert ('start' -in $script:events) 'Precisao de WMI recusou o mesmo processo'
    foreach ($scenario in @('port', 'identity', 'wmi', 'stop', 'launcher', 'disabled')) {
        Reset-State
        switch ($scenario) {
            port { $script:owner = 99 }
            identity { $script:live[10].StartTime = Get-Date }
            wmi { $script:tableFails = $true }
            stop { $script:stopFails = $true }
            launcher { $script:stuckLauncher = $true }
            disabled { $script:task.Settings.Enabled = $false }
        }
        $failed = $false
        try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' } catch { $failed = $true }
        Assert $failed "Falha ignorada: $scenario"
        Assert ('start' -notin $script:events) "Duplicou processo: $scenario"
        Assert ($script:task.Settings.RestartCount -eq 3) "Perdeu recuperacao automatica: $scenario"
    }
    foreach ($scenario in @('healthy', 'transient', 'recent', 'updating', 'installing')) {
        Reset-State
        switch ($scenario) {
            healthy { $script:httpReplies.Clear(); $script:httpReplies.Enqueue($true) }
            transient { $script:httpReplies.Clear(); $script:httpReplies.Enqueue($false); $script:httpReplies.Enqueue($true) }
            recent { $script:lastRun = Get-Date }
            updating { }
            installing { $script:processRows = @($rows[0], $rows[1], (Row 50 1 'powershell.exe' 'powershell -File install.ps1 -Update')) }
        }
        Repair-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
        Assert (-not $script:events.Count) "Vigia interferiu: $scenario"
    }
    Reset-State
    $script:processRows = @($rows[0], $rows[1], $rows[4], $rows[5], $rows[6])
    Repair-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null
    Assert ('start' -in $script:events) 'Vigia nao recuperou travamento confirmado'

    $lockJob = Start-Job {
        $mutex = New-Object Threading.Mutex($false, 'Local\HangarInstall')
        try {
            [void]$mutex.WaitOne()
            Write-Output 'locked'
            Start-Sleep -Seconds 20
        } finally { $mutex.ReleaseMutex(); $mutex.Dispose() }
    }
    try {
        foreach ($attempt in 1..100) {
            if ('locked' -in @(Receive-Job $lockJob -Keep)) { break }
            [Threading.Thread]::Sleep(50)
        }
        Assert ('locked' -in @(Receive-Job $lockJob -Keep)) 'Processo de teste nao adquiriu a exclusao'
        Reset-State
        Repair-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
        Assert (-not $script:events.Count -and $script:httpReplies.Count -eq 2) 'Vigia interferiu com instalacao em outro processo'
    } finally { Remove-Job $lockJob -Force }

    $tokens = $null; $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile("$PSScriptRoot/../install.ps1", [ref]$tokens, [ref]$errors)
    Assert (-not $errors.Count) 'Instalador nao parseia'
    $assignment = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$linhaVbs'
    }, $true)
    $cmdLinha = 'cmd /c "C:\repo com espaco\python.exe" -m app.main'
    . ([scriptblock]::Create($assignment.Extent.Text))
    Assert ($linhaVbs -match '^WScript\.Quit .*\.Run\(.*0, True\)$') 'Lancador nao acompanha o filho ou nao propaga seu codigo de saida'
    function New-ScheduledTaskSettingsSet {
        param([switch]$AllowStartIfOnBatteries, [switch]$DontStopIfGoingOnBatteries,
            $ExecutionTimeLimit, $MultipleInstances, $RestartCount, $RestartInterval)
        return $PSBoundParameters
    }
    $assignment = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$cfg'
    }, $true)
    . ([scriptblock]::Create($assignment.Extent.Text))
    Assert ($cfg.MultipleInstances -eq 'IgnoreNew' -and $cfg.RestartCount -eq 3 -and $cfg.RestartInterval.TotalMinutes -eq 1) 'Agendador perdeu protecao de instancia ou recuperacao'
    $outerTry = $ast.EndBlock.Statements | Where-Object { $_ -is [Management.Automation.Language.TryStatementAst] } | Select-Object -First 1
    Reset-State
    # O pause/restore de verdade passa por Export-/Register-ScheduledTask e so o Agendador real
    # reprova a manobra errada - isso vive no scripts/test-windows-tasks-reais.ps1. Aqui a pergunta
    # e outra: uma instalacao que ESTOURA no meio ainda repoe a recuperacao de cada tarefa pausada?
    $script:reposto = @{}
    function Restaurar-Recuperacao([string]$nome, [string]$blocoXml) { $script:reposto[$nome] = $blocoXml }
    $bloco = '<RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>'
    $pausedRecovery = @{ 'hangar-backend' = $bloco }
    $installLocked = $false; $installMutex = $null
    try { throw 'falha simulada da instalacao' }
    catch { }
    finally { & ([scriptblock]::Create($outerTry.Finally.Extent.Text.TrimStart('{').TrimEnd('}'))) }
    Assert ($script:reposto['hangar-backend'] -eq $bloco) 'Falha da instalacao deixou recuperacao desativada'
    Write-Output 'OK: selecao de processos, recuperacao, falhas, espera, atualizacao, vigia e lancador'
} finally { $env:LOCALAPPDATA = $testLocalData }
