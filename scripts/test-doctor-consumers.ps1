param([string]$InstallScript = (Join-Path $PSScriptRoot '..\install.ps1'))
$ErrorActionPreference = 'Stop'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('hangar-doctor-contract-' + [guid]::NewGuid().ToString('N'))
$previousLog = $env:HANGAR_DOCTOR_FIXTURE_LOG
try {
    $raiz = Join-Path $temporary 'checkout com espaços'
    $raizAtalhos = $raiz
    New-Item -ItemType Directory -Force (Join-Path $raiz 'scripts') | Out-Null
    $env:HANGAR_DOCTOR_FIXTURE_LOG = Join-Path $temporary 'calls.jsonl'
    $fake = @'
[IO.File]::AppendAllText($env:HANGAR_DOCTOR_FIXTURE_LOG, (ConvertTo-Json -Compress -InputObject @($args)) + "`n", [Text.UTF8Encoding]::new($false))
exit 0
'@
    [IO.File]::WriteAllText((Join-Path $raiz 'scripts\hangar-server-command.ps1'), $fake, [Text.UTF8Encoding]::new($true))
    $tokens = $null; $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $InstallScript).ProviderPath, [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw 'O instalador não passou pelo parser PowerShell.' }
    $calls = @($ast.FindAll({ param($node)
        $node -is [Management.Automation.Language.CommandAst] -and
        $node.CommandElements.Count -ge 2 -and
        $node.CommandElements[0].Extent.Text -like '*hangar-server-command.ps1*' -and
        $node.CommandElements[1].Extent.Text -eq 'doctor'
    }, $true))
    if ($calls.Count -ne 2) { throw 'As duas chamadas diretas do diagnóstico não foram encontradas.' }
    foreach ($call in $calls) { & ([scriptblock]::Create($call.Extent.Text)) }
    $assignment = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$conteudoDoctor'
    }, $true)
    if (-not $assignment) { throw 'O lançador do diagnóstico não foi encontrado.' }
    . ([scriptblock]::Create($assignment.Extent.Text))
    $launcher = Join-Path $temporary 'doctor.cmd'
    $encoding = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and
        $node.Left.Extent.Text -eq '$script:launcherCodePage'
    }, $true)
    $writer = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Escrever-Lancador'
    }, $true)
    if (-not $encoding -or -not $writer) { throw 'A escrita real do lançador não foi encontrada.' }
    . ([scriptblock]::Create($encoding.Extent.Text))
    . ([scriptblock]::Create($writer.Extent.Text))
    Escrever-Lancador $launcher $conteudoDoctor 'cmd' | Out-Null
    & $launcher --qr
    if ($LASTEXITCODE -ne 0) { throw 'O lançador do diagnóstico falhou.' }
    $rows = @([IO.File]::ReadAllLines($env:HANGAR_DOCTOR_FIXTURE_LOG) | ForEach-Object { ,(ConvertFrom-Json $_) })
    if ($rows.Count -ne 3) { throw 'Quantidade inesperada de chamadas do diagnóstico.' }
    for ($i = 0; $i -lt $rows.Count; $i++) {
        $expected = @('doctor', '--backend', (Join-Path $raiz 'backend'))
        if ($i -gt 0) { $expected += '--qr' }
        if (($rows[$i] -join "`0") -cne ($expected -join "`0")) { throw "Diagnóstico $i perdeu a pasta explícita ou os argumentos." }
    }
    Write-Host 'GREEN: três consumidores do diagnóstico usam a instalação explícita, inclusive caminho com espaços.'
} finally {
    $env:HANGAR_DOCTOR_FIXTURE_LOG = $previousLog
    Remove-Item -Recurse -Force $temporary -ErrorAction SilentlyContinue
}
