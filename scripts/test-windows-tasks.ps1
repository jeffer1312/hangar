$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/windows-tasks.ps1"
# A leitura real, antes da troca abaixo: o teste dela substitui so a espera pelo Job.
$realProcessTable = ${function:Get-HangarProcessTable}
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
# Pacote: o Python roda do caminho real da versao anterior; quem o identifica e o lancador acima dele.
$pacote = @(
    (Row 60 1 'hangar-launcher.exe' '"C:\Users\u\.hangar\bin\hangar-launcher.exe" launch backend --log "C:\l.log"'),
    (Row 61 60 'python.exe' '"C:\Users\u\.hangar\versions\0.1.0.5\python\python.exe" -m app.main --log "C:\l.log"'),
    (Row 62 1 'hangar-server.exe' '"C:\Users\u\.hangar\versions\0.1.0.5\bin\hangar-server.exe" update --run --listen 0.0.0.0:8765'),
    (Row 63 62 'python.exe' '"C:\Users\u\.hangar\versions\0.1.0.5\python\python.exe" -m app.main')
)
Assert (((Get-HangarTaskProcesses $pacote 'hangar-backend' 'C:\Users\u\.hangar\versions\0.1.0.6\backend').ProcessId -join ',') -eq '61') 'Backend do lancador nao identificado (ou o atualizador entrou junto)'
Assert (-not (Get-HangarTaskProcesses $pacote 'hangar-frontend' 'C:\repo\frontend')) 'Lancador do backend tomado pelo front'
# Decisao 9: versao sem o Python, o hangar-server filho do lancador e o backend; o de outro pai, nao.
$direto = @(
    (Row 70 1 'hangar-launcher.exe' '"C:\Users\u\.hangar\bin\hangar-launcher.exe" launch backend --log "C:\l.log"'),
    (Row 71 70 'hangar-server.exe' '"C:\Users\u\.hangar\versions\0.1.0.7\bin\hangar-server.exe"'),
    (Row 72 1 'hangar-server.exe' '"C:\Users\u\.hangar\versions\0.1.0.7\bin\hangar-server.exe" update --run'),
    (Row 73 71 'hangar-server.exe' '"C:\Users\u\.hangar\versions\0.1.0.7\bin\hangar-server.exe" open claude')
)
Assert (((Get-HangarTaskProcesses $direto 'hangar-backend' 'C:\Users\u\.hangar\versions\0.1.0.8\backend').ProcessId -join ',') -eq '71') 'hangar-server do lancador nao identificado (ou outro hangar-server entrou junto)'
# Decisao 38: o lancador religa o servidor que cai e entra na parada; o de outra instalacao, nao.
Assert (((Get-HangarBackendLaunchers $direto @($direto[1]) 'hangar-backend').ProcessId -join ',') -eq '70') 'Lancador pai do hangar-server fora da parada'
$vbs = Join-Path $env:LOCALAPPDATA 'hangar\hangar-backend.vbs'
$espera = @(
    (Row 80 1 'wscript.exe' "wscript.exe `"$vbs`""),
    (Row 81 80 'hangar-launcher.exe' '"C:\Users\u\.hangar\bin\hangar-launcher.exe" launch backend --log "C:\l.log"'),
    (Row 82 1 'hangar-launcher.exe' '"C:\outro\hangar-launcher.exe" launch backend --log "C:\outro.log"')
)
Assert (((Get-HangarBackendLaunchers $espera @() 'hangar-backend').ProcessId -join ',') -eq '81') 'Lancador na espera da religada nao identificado (ou o de outra instalacao entrou junto)'
Assert (-not (Get-HangarBackendLaunchers $espera @() 'hangar-frontend')) 'Lancador do backend tomado pelo front'

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
    $script:apart = @()
    $script:lateOwner = $null
    $script:frontOwner = $null
    $script:slowRelease = 0
    $script:idleOwner = $false
    $script:idleStays = $false
    $script:healthGoneAfterStop = $false
    $script:idleAddress = '0.0.0.0'
    $script:ignoredStarts = 0
    $script:frozenLastRun = $false
    $script:listening = $false
    $script:noPort = $false
    $script:healthy = $false
    $script:phantomStarts = 0
    $script:migrated = $true
    $script:tableReads = 0
    $script:stopFails = $false
    $script:exitsBeforeStop = $null
    $script:tableFails = $false
    $script:processRows = $rows
    $script:lastRun = $old
}
# Processo vivo que nao e da tarefa (o executor da atualizacao sai do Job): nao a segura em Running.
function Get-TaskLive { @($script:live.Keys | Where-Object { [int]$_ -notin $script:apart }).Count }
function Get-HangarProcessTable {
    if ($script:tableFails) { throw 'WMI indisponivel' }
    $script:tableReads++
    return $script:processRows
}
function Test-Path { param($LiteralPath)
    if ($LiteralPath -like '*\.hangar\current') { return $script:migrated }
    return Microsoft.PowerShell.Management\Test-Path -LiteralPath $LiteralPath
}
function Get-ScheduledTask { param($TaskName, $ErrorAction) return $script:task }
function Get-ScheduledTaskInfo { param($TaskName, $ErrorAction) return @{ LastRunTime = $script:lastRun } }
function Set-ScheduledTask { param($TaskName, $Settings, $ErrorAction) $script:events += "settings:$($Settings.RestartCount)" }
function Get-NetTCPConnection { param($State, $LocalPort, $ErrorAction)
    if ($LocalPort -eq 8765 -and $script:listening) { return @{ OwningProcess = 500 } }
    if ($LocalPort -eq 8765 -and $script:idleStays) { return @{ OwningProcess = 0; LocalAddress = $script:idleAddress } }
    if ($LocalPort -eq 8765 -and $script:idleOwner -and -not $script:events.Count) { return @{ OwningProcess = 0; LocalAddress = $script:idleAddress } }
    if ($LocalPort -eq 5173 -and $script:frontOwner) { return @{ OwningProcess = $script:frontOwner } }
    if ($script:events.Count -and $script:lateOwner) { return @{ OwningProcess = $script:lateOwner } }
    if ($script:events.Count -and $script:slowRelease -gt 0) { $script:slowRelease--; return @{ OwningProcess = 777 } }
    if ($script:owner) { return @{ OwningProcess = $script:owner } }
}
function Test-HangarHealth { param($port) if ($script:healthGoneAfterStop -and $script:events.Count) { return $false }; return $script:healthy }
function Get-Process { param($Id, $ErrorAction) return $script:live[[int]$Id] }
function Stop-Process { param($InputObject, [switch]$Force, $ErrorAction)
    if ($script:stopFails) { throw 'Acesso negado' }
    if ($script:exitsBeforeStop -eq $InputObject.Id) {
        $script:live.Remove([int]$InputObject.Id)
        if ($script:owner -eq $InputObject.Id) { $script:owner = $null }
        if (-not (Get-TaskLive) -and -not $script:stuckLauncher) { $script:task.State = 'Ready' }
        $InputObject | Add-Member -NotePropertyName HasExited -NotePropertyValue $true -Force
        throw [InvalidOperationException]'Process has exited, so the requested information is not available.'
    }
    $script:events += "stop:$($InputObject.Id)"
    $script:live.Remove([int]$InputObject.Id)
    if ($script:owner -eq $InputObject.Id) { $script:owner = $null }
    if (-not (Get-TaskLive) -and -not $script:stuckLauncher) { $script:task.State = 'Ready' }
}
function Start-ScheduledTask { param($TaskName, $ErrorAction)
    Assert (-not (Get-TaskLive) -and -not $script:owner) 'Nova instancia iniciada antes de encerrar a antiga'
    $script:events += 'start'
    if ($script:ignoredStarts -gt 0) { $script:ignoredStarts--; return }
    if (-not $script:frozenLastRun) { $script:lastRun = $script:lastRun.AddSeconds(1) }
    if ($script:phantomStarts -gt 0) { $script:phantomStarts--; $script:task.State = 'Ready'; return }
    $script:task.State = 'Running'
    $script:listening = -not $script:noPort
}
function Start-Sleep { param($Seconds, $Milliseconds) }

try {
    Reset-State
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    # Sem 'settings:*' na sequencia: o Restart-HangarTask nao mexe mais no RestartCount (o
    # Agendador recusa Count=0 e o IgnoreNew ja descarta o reinicio por falha atrasado - ver o
    # comentario no windows-tasks.ps1 e o scripts/test-windows-tasks-reais.ps1).
    Assert (($script:events -join ',') -eq 'stop:10,stop:11,start') 'Parada/reinicio fora de ordem'
    Assert ('settings:0' -notin $script:events) 'Voltou a pausar a recuperacao por Set-ScheduledTask (o Agendador recusa)'
    # Decisao 38: entre a queda e a religada so o lancador esta de pe; o reinicio o para e sobe outro.
    Reset-State
    $script:processRows = $espera
    $waiting = [pscustomobject]@{ Id = 81; StartTime = $old }
    $waiting | Add-Member ScriptMethod WaitForExit { param($timeout) return $true }
    $script:live = @{ 81 = $waiting }
    $script:owner = $null
    Restart-HangarTask 'hangar-backend' 8765 'C:\Users\u\.hangar\versions\0.1.0.8\backend' | Out-Null
    Assert (($script:events -join ',') -eq 'stop:81,start') "Lancador na espera da religada nao foi parado: $($script:events -join ',')"
    # O Agendador pode aceitar o pedido sem abrir instancia: nao vale como reinicio feito.
    Reset-State
    $script:ignoredStarts = 1
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    Assert (($script:events -join ',') -eq 'stop:10,stop:11,start,start') "Inicio descartado nao foi repetido: $($script:events -join ',')"
    Reset-State
    $script:ignoredStarts = 99
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' } catch { $failed = "$_" }
    Assert ($failed -match 'nao iniciou') "Tarefa que nunca iniciou passou por reiniciada: $failed"
    Assert (@($script:events | Where-Object { $_ -eq 'start' }).Count -eq 3) 'Tentativas de inicio fora do limite'
    Reset-State
    $script:frozenLastRun = $true
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    Assert ('start' -in $script:events) 'LastRunTime atrasado desfez um reinicio que subiu'
    # Medido em atualizacao real: saiu com 0, sem processo nem porta. LastRunTime nao prova a subida.
    Reset-State
    $script:phantomStarts = 1
    $trail = @(Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend')
    Assert (@($script:events | Where-Object { $_ -eq 'start' }).Count -eq 2) "Inicio sem processo nao foi repetido: $($script:events -join ',')"
    Assert ($trail.Count -eq 2 -and $trail[0] -match 'tentativa=1 porta=nao' -and $trail[1] -match 'tentativa=2 porta=sim') "Trilha das tentativas fora do formato: $($trail -join ' | ')"
    Reset-State
    $script:phantomStarts = 99
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' } catch { $failed = "$_" }
    Assert ($failed -match 'nao iniciou') "Inicio que nunca deixou processo passou por reiniciado: $failed"
    # O hangar-server sai sozinho com o Python dele: sumir antes da parada nao pode cancelar o inicio.
    Reset-State
    $script:exitsBeforeStop = 11
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null
    Assert ('start' -in $script:events) "Processo que saiu antes da parada cancelou o inicio: $($script:events -join ',')"
    # Saiu antes da conferencia de identidade: o StartTime ilegivel tambem conta como parado.
    Reset-State
    $gone = [pscustomobject]@{ Id = 11; HasExited = $true }
    $gone | Add-Member ScriptProperty StartTime {
        $script:live.Remove(11); $script:owner = $null
        if (-not $script:live.Count) { $script:task.State = 'Ready' }
        throw [InvalidOperationException]'Process has exited, so the requested information is not available.'
    }
    $gone | Add-Member ScriptMethod WaitForExit { param($timeout) return $true }
    $script:live[11] = $gone
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null
    Assert ('start' -in $script:events) "Processo que saiu antes da conferencia cancelou o inicio: $($script:events -join ',')"
    # O PowerShell devolve StartTime nulo, sem erro, de processo que nao consegue abrir: saiu = parado.
    Reset-State
    $null11 = [pscustomobject]@{ Id = 11 }
    $null11 | Add-Member ScriptProperty StartTime {
        $script:live.Remove(11); $script:owner = $null
        if (-not $script:live.Count) { $script:task.State = 'Ready' }
        return $null
    }
    $null11 | Add-Member ScriptMethod WaitForExit { param($timeout) return $true }
    $script:live[11] = $null11
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null
    Assert ('start' -in $script:events) "StartTime nulo de processo que saiu cancelou o inicio: $($script:events -join ',')"
    # Nulo com o processo ainda vivo nao prova identidade: aborta.
    Reset-State
    $script:live[11] | Add-Member ScriptProperty StartTime { return $null } -Force
    $failed = $false
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null } catch { $failed = "$_" }
    Assert ($failed -match 'ilegivel' -and 'start' -notin $script:events) "StartTime nulo com o processo vivo nao abortou: $failed"
    # Lista de portas vazia com o servidor de pe: a saude do hangar-server prova a subida.
    Reset-State
    $script:phantomStarts = 99
    $script:healthy = $true
    $trail = @(Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend')
    Assert (@($script:events | Where-Object { $_ -eq 'start' }).Count -eq 1 -and $trail[0] -match 'porta=sim') "Servidor de pe com a lista de portas vazia virou falha: $($trail -join ' | ')"
    Reset-State
    $script:noPort = $true
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null
    Assert (@($script:events | Where-Object { $_ -eq 'start' }).Count -eq 1) 'Subida lenta virou inicio repetido ou falha'
    Reset-State
    $script:slowRelease = 2
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    Assert ('start' -in $script:events) 'Porta que soltou logo depois barrou o reinicio'
    Assert ($script:tableReads -eq 1) 'Espera da porta voltou a depender do WMI depois da parada'
    # A lista de portas do Windows ja deu o PID 0 (System Idle) como dono da porta do servidor vivo.
    Reset-State
    $script:owner = $null
    $script:idleOwner = $true
    $script:healthy = $true
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null } catch { $failed = "$_" }
    Assert (-not $failed -and 'start' -in $script:events) "PID 0 na lista de portas barrou o reinicio: $failed"
    # PID 0 sem a saude do hangar-server na porta pode ser outro programa: recusa antes de parar.
    Reset-State
    $script:owner = $null
    $script:idleOwner = $true
    $script:healthy = $false
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null } catch { $failed = "$_" }
    Assert ($failed -match 'ocupada por outro processo' -and -not $script:events.Count) "PID 0 sem o hangar-server parou o servidor: $failed $($script:events -join ',')"
    Assert ($failed -notmatch 'Stop-Process -Id 0') "Mandou encerrar o System Idle: $failed"
    # PID 0 num endereco que o hangar-server nao abre e outro programa, mesmo com a saude respondendo.
    Reset-State
    $script:owner = $null
    $script:idleOwner = $true
    $script:idleAddress = '::1'
    $script:healthy = $true
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null } catch { $failed = "$_" }
    Assert ($failed -match 'ocupada por outro processo' -and -not $script:events.Count) "PID 0 em [::1] parou o servidor: $failed $($script:events -join ',')"
    # A lista que deu PID 0 com o servidor vivo continua dando depois da parada: sobe mesmo assim.
    Reset-State
    $script:owner = $null
    $script:idleStays = $true
    $script:healthy = $true
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null } catch { $failed = "$_" }
    Assert (-not $failed -and 'start' -in $script:events) "PID 0 que ficou na lista depois da parada deixou o servidor parado: $failed"
    # PID 0 que sobra na lista nao prova a subida: sem instancia e sem saude, as tentativas acabam em erro.
    Reset-State
    $script:owner = $null
    $script:idleStays = $true
    $script:healthy = $true
    $script:healthGoneAfterStop = $true
    $script:phantomStarts = 99
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null } catch { $failed = "$_" }
    Assert ($failed -match 'nao iniciou') "PID 0 que sobrou na lista passou por subida sem servidor: $failed"
    # A saude de um servidor que nao entra na parada (aberto a mao, tarefa sem processo) nao vale pelo PID 0.
    Reset-State
    $script:owner = $null
    $script:idleOwner = $true
    $script:healthy = $true
    $script:processRows = $espera
    $script:live = @{}
    $script:task.State = 'Ready'
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null } catch { $failed = "$_" }
    Assert ($failed -match 'ocupada por outro processo' -and -not $script:events.Count) "Saude de servidor fora da parada liberou o PID 0: $failed $($script:events -join ',')"
    # Com CP_LISTEN em 127.0.0.1 a saude vai ao proprio endereco: PID 0 ali com a saude respondendo e o servidor.
    Reset-State
    $script:owner = $null
    $script:idleOwner = $true
    $script:idleAddress = '127.0.0.1'
    $script:healthy = $true
    $failed = $null
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' | Out-Null } catch { $failed = "$_" }
    Assert (-not $failed -and 'start' -in $script:events) "PID 0 em 127.0.0.1 com a saude respondendo barrou o reinicio: $failed"
    # Maquina migrada: o hangar-server novo abre a porta do front entre as duas conferencias.
    $frontRows = @(
        (Row 70 1 'node.exe' 'C:\repo\frontend\node_modules\vite\bin\vite.js preview'),
        (Row 71 1 'hangar-server.exe' 'C:\Users\u\.hangar\current\bin\hangar-server.exe'),
        (Row 72 1 'node.exe' 'C:\outro\server.js')
    )
    foreach ($case in @(
        @{ late = 71; migrated = $true; served = $true; holder = '' },
        @{ late = 72; migrated = $true; served = $false; holder = 'node\.exe PID 72' },
        @{ late = 71; migrated = $false; served = $false; holder = 'hangar-server\.exe PID 71' }
    )) {
        Reset-State
        $script:migrated = $case.migrated
        $script:processRows = $frontRows
        $script:live = @{}
        $vite = [pscustomobject]@{ Id = 70; StartTime = $old }
        $vite | Add-Member ScriptMethod WaitForExit { param($timeout) return $true }
        $script:live[70] = $vite
        $script:owner = 70
        $script:lateOwner = $case.late
        $failed = $null
        try { Restart-HangarTask 'hangar-frontend' 5173 'C:\repo\frontend' } catch { $failed = "$_" }
        Assert ('start' -notin $script:events) "Front subiu com a porta tomada: $($case.late)"
        if ($case.served) { Assert (-not $failed) "Porta do front no hangar-server virou falha: $failed" }
        else { Assert ($failed -match "ainda ocupada por $($case.holder)") "Porta presa nao disse quem segura: $failed" }
    }
    Reset-State
    $script:processRows = $frontRows
    $script:live = @{}
    $script:task.State = 'Ready'
    $script:owner = 71
    Restart-HangarTask 'hangar-frontend' 5173 'C:\repo\frontend'
    Assert ('start' -notin $script:events) 'Front subiu com a tela ja servida pelo hangar-server'
    Reset-State
    $script:processRows = $frontRows
    $script:live = @{}
    $script:task.State = 'Ready'
    $script:owner = $null
    $script:ignoredStarts = 99
    Restart-HangarTask 'hangar-frontend' 5173 'C:\repo\frontend'
    Assert (@($script:events | Where-Object { $_ -eq 'start' }).Count -eq 1) 'Front que nao abriu derrubaria uma versao boa'
    Reset-State
    $script:processRows = @($rows) + @($frontRows[2])
    $script:frontOwner = 72
    Restart-HangarTasks 'C:\repo' 8765 5173 3>$null
    Assert ('start' -in $script:events) 'Falha do front derrubou o reinicio do backend'
    Reset-State
    $script:processRows = $frontRows
    $script:owner = 71
    $failed = $false
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' } catch { $failed = $true }
    Assert $failed 'hangar-server alheio aceito na porta do backend'
    Reset-State
    $script:live[10].StartTime = $old.AddTicks(5)
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    Assert ('start' -in $script:events) 'Precisao de WMI recusou o mesmo processo'
    Reset-State
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' -NoStart
    Assert (($script:events -join ',') -eq 'stop:10,stop:11') "-NoStart subiu a tarefa ou nao parou: $($script:events -join ',')"
    Reset-State
    $script:processRows = @($rows) + @(Row 12 11 'hangar-server.exe' 'C:\Users\u\.hangar\bin\hangar-server.exe')
    $filho = [pscustomobject]@{ Id = 12; StartTime = $old }
    $filho | Add-Member ScriptMethod WaitForExit { param($timeout) return $true }
    $script:live[12] = $filho
    $script:owner = 12
    Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend'
    Assert (($script:events -join ',') -eq 'stop:10,stop:11,stop:12,start') 'hangar-server do backend travou ou desordenou o reinicio'
    # Versao direta: o executor da atualizacao e filho do hangar-server e e quem pediu o reinicio.
    Reset-State
    $script:processRows = @($direto[0], $direto[1],
        (Row 74 71 'hangar-server.exe' '"C:\Users\u\.hangar\versions\0.1.0.7\bin\hangar-server.exe" update --run --listen 0.0.0.0:8765'))
    $script:live = @{}
    foreach ($id in @(70, 71, 74)) {
        $process = [pscustomobject]@{ Id = $id; StartTime = $old }
        $process | Add-Member ScriptMethod WaitForExit { param($timeout) return $true }
        $script:live[$id] = $process
    }
    $script:apart = @(74)
    $script:owner = 71
    Restart-HangarTask 'hangar-backend' 8765 'C:\Users\u\.hangar\versions\0.1.0.8\backend' | Out-Null
    Assert (($script:events -join ',') -eq 'stop:71,stop:70,start') "Reinicio da versao direta parou o executor da atualizacao: $($script:events -join ',')"
    Reset-State
    $script:processRows = @($rows) + @(Row 13 40 'hangar-server.exe' 'C:\other\hangar-server.exe')
    $script:owner = 13
    $failed = $false
    try { Restart-HangarTask 'hangar-backend' 8765 'C:\repo\backend' } catch { $failed = $true }
    Assert ($failed -and 'start' -notin $script:events) 'hangar-server de outro checkout tratado como do backend'
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
        $failed = $null
        try { Restart-HangarTasks 'C:\repo' 8765 0 } catch { $failed = "$_" }
        Assert ($failed -match 'em andamento' -and -not $script:events.Count) "Reinicio do atualizador interferiu com instalacao em outro processo: $failed"
    } finally { Remove-Job $lockJob -Force }
    # Leitura que estoura o prazo tenta de novo uma vez: nada foi parado antes dela.
    $tabela = @((Row 1 0 'System' ''))
    function Read-HangarProcessJob([int]$seconds) {
        $script:jobWaits += $seconds
        if ($script:jobWaits.Count -le $script:jobTimeouts) { return $null }
        return ,@($tabela)
    }
    $script:jobWaits = @(); $script:jobTimeouts = 1
    $lidas = & $realProcessTable
    Assert (($script:jobWaits -join ',') -eq '30,30' -and @($lidas).Count -eq 1) "Leitura que estourou o prazo nao foi repetida: esperas '$($script:jobWaits -join ',')'"
    $script:jobWaits = @(); $script:jobTimeouts = 2
    $failed = $null
    try { & $realProcessTable | Out-Null } catch { $failed = "$_" }
    Assert (($script:jobWaits -join ',') -eq '30,30' -and $failed -match 'nenhuma instancia sera encerrada') "Duas leituras estouradas: esperas '$($script:jobWaits -join ',')', erro: $failed"

    $tokens = $null; $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile("$PSScriptRoot/../install.ps1", [ref]$tokens, [ref]$errors)
    Assert (-not $errors.Count) 'Instalador nao parseia'
    $assignment = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$linhaVbs'
    }, $true)
    $cmdLinha = 'cmd /c "C:\repo com espaco\python.exe" -m app.main'
    . ([scriptblock]::Create($assignment.Extent.Text))
    Assert ($linhaVbs -match '^WScript\.Quit .*\.Run\(.*0, True\)$') 'Lancador nao acompanha o filho ou nao propaga seu codigo de saida'
    # O backend abre o proprio log: o `>` do cmd deixava a tarefa presa a um neto com a copia herdada.
    $assignment = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$cmdLinha'
    }, $true)
    $exe = 'C:\repo com espaco\python.exe'; $log = 'C:\Users\x\hangar-backend.log'
    $t = @{ Nome = 'hangar-backend'; Args = '-m app.main' }
    . ([scriptblock]::Create($assignment.Extent.Text))
    Assert ($cmdLinha -eq '"C:\repo com espaco\python.exe" -m app.main --log "C:\Users\x\hangar-backend.log"') "Backend voltou a abrir o log pelo cmd: $cmdLinha"
    $exe = 'C:\node\npm.cmd'; $log = 'C:\Users\x\hangar-frontend.log'
    $t = @{ Nome = 'hangar-frontend'; Args = 'run preview' }
    . ([scriptblock]::Create($assignment.Extent.Text))
    Assert ($cmdLinha -eq 'cmd /c ""C:\node\npm.cmd" run preview > "C:\Users\x\hangar-frontend.log" 2>&1"') "Frontend mudou de lancador: $cmdLinha"
    function New-ScheduledTaskSettingsSet {
        param([switch]$AllowStartIfOnBatteries, [switch]$DontStopIfGoingOnBatteries,
            $ExecutionTimeLimit, $MultipleInstances, $RestartCount, $RestartInterval, $Priority)
        return $PSBoundParameters
    }
    $assignment = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$cfg'
    }, $true)
    . ([scriptblock]::Create($assignment.Extent.Text))
    Assert ($cfg.MultipleInstances -eq 'IgnoreNew' -and $cfg.RestartCount -eq 999 -and $cfg.RestartInterval.TotalMinutes -eq 1) 'Agendador perdeu protecao de instancia ou recuperacao'
    Assert ($cfg.Priority -eq 4) 'Tarefa voltou a prioridade padrao do Agendador (7, abaixo do normal)'
    # Tarefa do backend: na maquina migrada, o lancador e o padrao que acha o Python do pacote.
    $assignment = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$tarefas'
    }, $true)
    $raiz = 'C:\repo'; $portaBack = 8765
    $lancadorPacote = 'C:\Users\u\.hangar\bin\hangar-launcher.exe'
    . ([scriptblock]::Create($assignment.Extent.Text))
    $b = $tarefas[0]
    Assert ($b.Exe -eq $lancadorPacote -and $b.Args -eq 'launch backend') "Tarefa migrada fora do lancador: $($b.Exe) $($b.Args)"
    $python = '"C:\Users\u\.hangar\versions\0.1.0.5\python\python.exe" -m app.main'
    Assert ($python -match $b.Padrao -and 'python.exe' -match $b.ExeProc) 'Pare-Servico nao acha o Python do pacote'
    Assert ('"C:\Users\u\.hangar\bin\hangar-launcher.exe" launch backend --log x' -match $b.Padrao -and 'hangar-launcher.exe' -match $b.ExeProc) 'Pare-Servico nao acha o lancador'
    Assert (-not ('"D:\outra\hangar-launcher.exe" launch backend --log x' -match $b.Padrao)) 'Pare-Servico pegou o lancador de outra instalacao'
    Assert (-not ('C:\repo\backend\.venv\Scripts\python.exe -m app.atualizar' -match $b.Padrao)) 'Padrao do pacote pegou o checkout'
    $lancadorPacote = $null
    . ([scriptblock]::Create($assignment.Extent.Text))
    $b = $tarefas[0]
    Assert ($b.Padrao -eq [regex]::Escape('C:\repo\backend') -and $b.ExeProc -eq 'uv|python') 'Maquina nao migrada mudou de padrao'
    # Decisao 7: checkout sem o app\main.py, a tarefa sobe o hangar-server pelo `launch backend`.
    $servidorCheckout = 'C:\repo\crates\target\release\hangar-server.exe'
    . ([scriptblock]::Create($assignment.Extent.Text))
    $b = $tarefas[0]
    Assert ($b.Exe -eq $servidorCheckout -and $b.Args -eq 'launch backend' -and $b.ExeProc -eq 'hangar-server') "Checkout sem Python fora do hangar-server: $($b.Exe) $($b.Args)"
    Assert ('"C:\repo\crates\target\release\hangar-server.exe" launch backend --log x' -match $b.Padrao) 'Pare-Servico nao acha o hangar-server do checkout'
    Assert (-not ('"D:\outra\crates\target\release\hangar-server.exe" launch backend --log x' -match $b.Padrao)) 'Pare-Servico pegou o hangar-server de outra worktree'
    $servidorCheckout = $null
    $releaseFunction = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Liberar-Instalacao'
    }, $true)
    . ([scriptblock]::Create($releaseFunction.Extent.Text))
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
    # Prova do passo de maquina: o .vbs com os mesmos bytes que o Escrever-Lancador grava (sem BOM).
    $pasta = Join-Path ([IO.Path]::GetTempPath()) "hangar-vbs-$PID"
    New-Item -ItemType Directory -Force $pasta | Out-Null
    try {
        $semBom = New-Object System.Text.UnicodeEncoding $false, $true
        $vbsDireto = Join-Path $pasta 'direto.vbs'
        [IO.File]::WriteAllBytes($vbsDireto, $semBom.GetBytes('WScript.Quit CreateObject("WScript.Shell").Run("""C:\Users\u\.hangar\bin\hangar-launcher.exe"" launch backend --log ""C:\l.log""", 0, True)'))
        $comBom = Join-Path $pasta 'bom.vbs'
        [IO.File]::WriteAllBytes($comBom, $semBom.GetPreamble() + $semBom.GetBytes('Run("""C:\h\hangar-launcher.exe"" launch backend", 0, True)'))
        $python = Join-Path $pasta 'python.vbs'
        [IO.File]::WriteAllBytes($python, $semBom.GetBytes('Run("cmd /c ""C:\repo\.venv\Scripts\python.exe"" -m app.main", 0, True)'))
        Reset-State
        $script:task.Settings.RestartCount = 999
        Assert (-not (Get-HangarBackendTaskProblem $vbsDireto)) '.vbs UTF-16 sem BOM com o launch backend reprovado'
        Assert (-not (Get-HangarBackendTaskProblem $comBom)) '.vbs UTF-16 com BOM reprovado'
        Assert ((Get-HangarBackendTaskProblem $python) -match 'launch backend') '.vbs sem o launch backend aprovado'
        Assert ((Get-HangarBackendTaskProblem (Join-Path $pasta 'falta.vbs')) -match 'nao existe') '.vbs ausente aprovado'
        $script:task.Settings.RestartCount = 3
        Assert ((Get-HangarBackendTaskProblem $vbsDireto) -match 'religa 3 vezes') 'Tarefa sem os reinicios automaticos aprovada'
        $script:task = $null
        Assert (-not (Get-HangarBackendTaskProblem $python)) 'Maquina sem a tarefa reprovada'
    } finally { Remove-Item -Recurse -Force $pasta -ErrorAction SilentlyContinue }
    Write-Output 'OK: selecao de processos, recuperacao, falhas, espera, atualizacao e lancador'
} finally { $env:LOCALAPPDATA = $testLocalData }
