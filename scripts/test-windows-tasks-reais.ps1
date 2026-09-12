# Exercita contra o AGENDADOR DE VERDADE o que o test-windows-tasks.ps1 nao consegue: ele roda no
# Linux com PowerShell 7 e simula Get-/Set-ScheduledTask, entao a validacao do XML da tarefa - a
# unica coisa que reprova a manobra de pausar a recuperacao - nunca era exercida. Foi assim que o
# instalador chegou a producao pausando a recuperacao por Set-ScheduledTask, manobra que o
# Agendador recusa em 100% das chamadas (HRESULT 0x80041319, "(43,8):Count:"): o passo 7/8 nunca
# iniciava a tarefa e a vigia nunca recuperava travamento nenhum.
#
# Usa uma tarefa DESCARTAVEL (hangar-teste-recuperacao), nunca as de verdade, e a remove no fim.
# Nao precisa de elevacao: registra no proprio usuario, igual ao instalador comum.
$ErrorActionPreference = 'Stop'
function Assert($value, $message) { if (-not $value) { throw $message } }

if (-not (Get-Command Register-ScheduledTask -ErrorAction SilentlyContinue)) {
    Write-Output 'PULADO: sem o modulo ScheduledTasks (rode no Windows; o resto vive no test-windows-tasks.ps1)'
    exit 0
}

$nome = 'hangar-teste-recuperacao'
$marcador = Join-Path $env:TEMP 'hangar-teste-recuperacao-marcador.txt'
# As funcoes testadas sao as do INSTALADOR, extraidas do arquivo - teste que redigita a logica nao
# prova nada sobre o que roda na instalacao.
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile("$PSScriptRoot/../install.ps1", [ref]$tokens, [ref]$errors)
Assert (-not $errors.Count) 'Instalador nao parseia'
foreach ($fn in @('Suspender-Recuperacao', 'Restaurar-Recuperacao')) {
    $def = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $fn
    }, $true)
    Assert $def "install.ps1 nao tem mais a funcao $fn"
    . ([scriptblock]::Create($def.Extent.Text))
}

# Mesma forma das tarefas reais (install.ps1:1835-1840): logon interativo, sem parar na bateria,
# sem limite de tempo, IgnoreNew e recuperacao 3x de minuto em minuto.
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited
$cfg = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
            -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew `
            -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
$gatilho = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$acao = New-ScheduledTaskAction -Execute 'cmd.exe' `
            -Argument "/c ping -n 120 127.0.0.1 > `"$marcador`"" -WorkingDirectory $env:TEMP

function Filhos-Do-Teste {
    # Casa pelo MARCADOR na linha de comando (o `cmd` da acao carrega o caminho do redirecionamento;
    # o `ping` neto, nao - o redirect e consumido pelo cmd). Assim nenhum ping alheio entra na conta.
    $meus = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -like "*$marcador*" })
    $netos = @()
    if ($meus.Count) {
        $ids = @($meus | ForEach-Object { [int]$_.ProcessId })
        $netos = @(Get-CimInstance Win32_Process -Filter "Name = 'PING.EXE'" -ErrorAction SilentlyContinue |
            Where-Object { [int]$_.ParentProcessId -in $ids })
    }
    return @($meus + $netos)
}

try {
    Register-ScheduledTask -TaskName $nome -Action $acao -Trigger $gatilho -Settings $cfg `
        -Principal $principal -Force | Out-Null

    # 1) Pausa com a tarefa RODANDO: a janela do instalador comeca com o backend anterior de pe, e
    #    re-registrar o XML nao pode derrubar quem esta no ar (senao a pausa seria pior que o bug).
    Start-ScheduledTask -TaskName $nome
    foreach ($i in 1..100) {
        if ((Get-ScheduledTask -TaskName $nome).State -eq 'Running' -and (Filhos-Do-Teste).Count) { break }
        Start-Sleep -Milliseconds 100
    }
    $antes = Get-ScheduledTask -TaskName $nome
    $pidsAntes = @(Filhos-Do-Teste | ForEach-Object ProcessId)
    Assert ($antes.State -eq 'Running') 'Tarefa de teste nao chegou a rodar'
    Assert ($pidsAntes.Count -ge 1) 'Processo filho da tarefa de teste nao apareceu'

    $bloco = Suspender-Recuperacao $nome
    # O OuterXml vem com o xmlns explicito ("<RestartOnFailure xmlns=...>"), e e reinserido num
    # documento com o MESMO namespace default - a redeclaracao e inofensiva, o item 2 abaixo prova.
    Assert ($bloco -match '<RestartOnFailure[ >]' -and $bloco -match '<Count>3</Count>') `
        "Suspender-Recuperacao nao devolveu o bloco removido: [$bloco]"
    $pausada = Get-ScheduledTask -TaskName $nome
    Assert ($pausada.Settings.RestartCount -eq 0) 'Recuperacao continuou ligada depois do pause'
    Assert (-not $pausada.Settings.RestartInterval) 'Intervalo de recuperacao sobreviveu ao pause'
    # O resto da definicao nao pode ter sido perdido no re-registro.
    Assert ($pausada.Settings.MultipleInstances -eq 'IgnoreNew') 'Pause perdeu a protecao contra duplicacao'
    Assert ($pausada.Settings.ExecutionTimeLimit -eq 'PT0S') 'Pause perdeu o limite de execucao'
    Assert ($pausada.Principal.LogonType -eq 'Interactive' -and $pausada.Principal.RunLevel -eq 'Limited') `
        'Pause perdeu o logon interativo ou o nivel de permissao'
    Assert ($pausada.Actions.Execute -eq 'cmd.exe' -and $pausada.Actions.WorkingDirectory -eq $env:TEMP) `
        'Pause perdeu a acao ou o diretorio de trabalho'
    Assert ($pausada.Triggers.Count -eq 1) 'Pause perdeu o gatilho'
    Assert ($pausada.State -eq 'Running') 'Pause derrubou a instancia em execucao'
    $pidsDepois = @(Filhos-Do-Teste | ForEach-Object ProcessId)
    Assert (@($pidsAntes | Where-Object { $_ -in $pidsDepois }).Count -ge 1) `
        'Pause matou o processo que estava no ar'

    # 2) O 7/8 re-registra a tarefa NO MEIO da janela de pausa (caminhos novos do .vbs e do venv).
    #    Repor o XML inteiro de antes desfaria essa instalacao; o restore tem de ser cirurgico.
    $acaoNova = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument '"C:\novo\hangar-backend.vbs"' `
                    -WorkingDirectory 'C:\novo'
    Register-ScheduledTask -TaskName $nome -Action $acaoNova -Trigger $gatilho `
        -Settings (New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew) `
        -Principal $principal -Force | Out-Null
    Restaurar-Recuperacao $nome $bloco
    $reposta = Get-ScheduledTask -TaskName $nome
    Assert ($reposta.Settings.RestartCount -eq 3 -and $reposta.Settings.RestartInterval -eq 'PT1M') `
        "Restore nao reps a recuperacao: RC=$($reposta.Settings.RestartCount) RI=$($reposta.Settings.RestartInterval)"
    Assert ($reposta.Actions.Execute -eq 'wscript.exe' -and $reposta.Actions.WorkingDirectory -eq 'C:\novo') `
        'Restore desfez o re-registro do passo 7/8 (voltou a definicao velha)'

    # 3) Tarefa que voltou com recuperacao PROPRIA sai sem toque (restore idempotente).
    Restaurar-Recuperacao $nome $bloco
    $duasVezes = Get-ScheduledTask -TaskName $nome
    Assert ($duasVezes.Settings.RestartCount -eq 3 -and $duasVezes.Settings.RestartInterval -eq 'PT1M') `
        'Restore repetido corrompeu a recuperacao'

    # 4) Nao e assert, e o registro do PORQUE: se algum dia o Set-ScheduledTask passar a aceitar
    #    Count=0, este caminho volta a ser opcao (e ai o XML deixa de ser necessario).
    $set = (Get-ScheduledTask -TaskName $nome).Settings
    $set.RestartCount = 0
    try {
        Set-ScheduledTask -TaskName $nome -Settings $set -ErrorAction Stop | Out-Null
        Write-Output '  NOTA: Set-ScheduledTask ACEITOU RestartCount=0 nesta maquina - reveja se o XML ainda e necessario'
        Restaurar-Recuperacao $nome $bloco
    } catch {
        Write-Output "  nota: Set-ScheduledTask com RestartCount=0 segue recusado pelo Agendador ($($_.Exception.Message.Split([char]10)[0]))"
    }

    Write-Output 'OK: pause/restore da recuperacao contra o Agendador real (instancia viva, definicao preservada, restore cirurgico)'
} finally {
    Stop-ScheduledTask -TaskName $nome -ErrorAction SilentlyContinue
    Filhos-Do-Teste | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Unregister-ScheduledTask -TaskName $nome -Confirm:$false -ErrorAction SilentlyContinue
    Remove-Item $marcador -Force -ErrorAction SilentlyContinue
}
