# Executa apenas as decisoes de elevacao, sem instalar nem registrar tarefas reais.
$ErrorActionPreference = 'Stop'
$installer = Join-Path (Split-Path -Parent $PSScriptRoot) 'install.ps1'
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }

function Assert($condition, $message) { if (-not $condition) { throw $message } }
foreach ($name in @('Get-InstallRunLevel', 'Set-ShortcutElevation')) {
    $definition = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $true)
    Assert ($null -ne $definition) "Funcao ausente: $name"
    . ([scriptblock]::Create($definition.Extent.Text))
}

function EhAdmin { return $script:testAdmin }
function Get-ScheduledTask { param($TaskName, $ErrorAction)
    Assert ($TaskName -eq 'hangar-backend') 'Consultar somente a tarefa do Hangar'
    if ($script:existingLevel) { return @{ Principal = @{ RunLevel = $script:existingLevel } } }
}
foreach ($existing in @($null, 'Limited', 'Highest')) {
    $script:existingLevel = $existing
    $script:testAdmin = $true
    Assert ((Get-InstallRunLevel) -eq 'Highest') 'Instalacao elevada precisa manter Highest'
    $script:testAdmin = $false
    if ($existing -eq 'Highest') {
        $blocked = $false
        try { Get-InstallRunLevel | Out-Null } catch { $blocked = $true }
        Assert $blocked 'Atualizacao comum nao pode rebaixar uma instalacao elevada'
    } else {
        Assert ((Get-InstallRunLevel) -eq 'Limited') 'Instalacao comum precisa manter Limited'
    }
}

function New-ScheduledTaskPrincipal { param($UserId, $LogonType, $RunLevel)
    Assert ($LogonType -eq 'Interactive') 'Tarefas precisam continuar na sessao interativa'
    Assert ($RunLevel -eq $script:installRunLevel) 'Principal perdeu o nivel de elevacao'
    return @{ UserId = $UserId; LogonType = $LogonType; RunLevel = $RunLevel }
}
function New-ScheduledTaskAction { param($Execute, $Argument) return @{} }
function Register-ScheduledTask { param($TaskName, $Action, $Trigger, $Settings, $Principal, $Xml, [switch]$Force, $ErrorAction)
    if ($Xml -or $PSBoundParameters.ContainsKey('Xml')) {
        # Registro por XML (Suspender-/Restaurar-Recuperacao): nivel e logon viajam DENTRO do XML
        # exportado, nao num -Principal - e cada comando aqui e executado isolado do resto da
        # funcao, entao nao ha documento pra inspecionar. Que o round-trip preserva LogonType e
        # RunLevel esta provado contra o Agendador real no scripts/test-windows-tasks-reais.ps1.
        # O que se cobra aqui e o -Force: sem ele o re-registro falha em tarefa existente, que e
        # o unico caso em que essas duas funcoes rodam.
        Assert $Force "Registro por XML sem -Force na tarefa $TaskName"
        return
    }
    Assert ($Principal.RunLevel -eq $script:installRunLevel) "Nivel errado na tarefa $TaskName"
    Assert ($Principal.LogonType -eq 'Interactive') "Sessao errada na tarefa $TaskName"
}
$principalAssignment = $ast.Find({ param($node)
    $node -is [Management.Automation.Language.AssignmentStatementAst] -and
    $node.Left.Extent.Text -eq '$taskPrincipal'
}, $true)
Assert ($null -ne $principalAssignment) 'Principal compartilhado ausente'
$registrations = $ast.FindAll({ param($node)
    $node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Register-ScheduledTask'
}, $true)
Assert ($registrations.Count -ge 4) 'Registros e caminhos de reparo precisam ser exercitados'
$t = @{ Nome = 'hangar-backend' }
foreach ($level in @('Limited', 'Highest')) {
    $script:installRunLevel = $level
    . ([scriptblock]::Create($principalAssignment.Extent.Text))
    foreach ($registration in $registrations) { & ([scriptblock]::Create($registration.Extent.Text)) }
}

$shortcut = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString() + '.lnk')
try {
    $bytes = New-Object byte[] 100
    $bytes[0] = 0x4c
    $bytes[0x15] = 0x81
    [IO.File]::WriteAllBytes($shortcut, $bytes)
    foreach ($elevated in @($true, $true, $false, $false)) {
        Set-ShortcutElevation $shortcut $elevated
        $actual = [IO.File]::ReadAllBytes($shortcut)
        $expected = if ($elevated) { 0xa1 } else { 0x81 }
        Assert ($actual[0x15] -eq $expected) 'Atalho perdeu flags ou nivel de elevacao'
        $actual[0x15] = $bytes[0x15]
        Assert ([Convert]::ToBase64String($actual) -eq [Convert]::ToBase64String($bytes)) 'Atalho alterado fora do flag de elevacao'
    }
    [IO.File]::WriteAllBytes($shortcut, (New-Object byte[] 4))
    $blocked = $false
    try { Set-ShortcutElevation $shortcut $true } catch { $blocked = $true }
    Assert $blocked 'Atalho invalido precisa falhar'
} finally { if (Test-Path $shortcut) { Remove-Item $shortcut } }
Write-Output 'OK: modos comum/admin, protecao da atualizacao, tarefas interativas e flags do atalho'
