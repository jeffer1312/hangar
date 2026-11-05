$ErrorActionPreference = 'Stop'
# Extrai apenas a função: o instalador completo nunca é executado nesta prova.
$tokens = $null; $errors = $null
$tree = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot '../install.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'O instalador não passou pelo parser' }
$function = $tree.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Pare-Servico' }, $true)
Invoke-Expression $function.Extent.Text
$old = (Get-Date).AddMinutes(-10)
function Row($id, $parent, $name, $command) { [pscustomobject]@{ ProcessId=$id; ParentProcessId=$parent; Name=$name; CommandLine=$command; CreationDate=$old } }
function Tabela-De-Processos { $script:rows }
function Falta($message) { throw $message }
function Stop-Process { param([int[]]$Id, [switch]$Force, $ErrorAction) foreach ($number in $Id) { $script:stopped += $number; $script:live.Remove($number) } }
function Get-Process { param([int]$Id, $ErrorAction) if ($script:live.ContainsKey($Id)) { [pscustomobject]@{ Id=$Id } } }
foreach ($command in @('C:\repo\hangar-server.exe checkout-update --run --port 8765', '"C:\repo with spaces\hangar-server.exe" checkout-update --run --port 8765', 'C:\repo\python.exe -m app.atualizar')) {
    $script:rows = @(
        (Row 10 1 'python.exe' 'C:\repo\python.exe -m app.main'),
        (Row 11 10 'python.exe' 'C:\repo\python.exe -m app.main'),
        (Row 20 11 'hangar-server.exe' $command),
        (Row $PID 20 'powershell.exe' 'powershell -File install.ps1 -Update')
    )
    $script:live = @{ 10=$true; 11=$true; 20=$true }; $script:live[$PID]=$true; $script:stopped=@()
    $count = Pare-Servico -Nome 'hangar-backend' -Porta 0 -Padrao 'app\.main' -Exe 'python.exe'
    if ($count -ne 2 -or (($script:stopped | Sort-Object) -join ',') -ne '10,11') { throw "Parada incorreta: $command / $script:stopped" }
    if (-not $script:live.ContainsKey(20) -or -not $script:live.ContainsKey($PID)) { throw 'O atualizador ou o instalador foi encerrado' }
}
Write-Output 'Proteção do atualizador: 3 tabelas conferidas, sem processos reais'
