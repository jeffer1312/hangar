# Confere que os lancadores do ~/.local/bin que o install.ps1 antigo gravava pelo Python
# (hangar-conta e os do Codex) ja foram regravados sem ele, e so entao deixa a prova do passo
# 2026-11-17-atalhos-windows-sem-python. Lancador ausente nao e erro: nada nele depende do Python.
param([string]$Perfil = $HOME)
$ErrorActionPreference = 'Stop'
$passo = '2026-11-17-atalhos-windows-sem-python'
$bin = Join-Path $Perfil '.local\bin'
# O que a forma nova de cada um tem e a antiga nao tinha.
$esperado = [ordered]@{
    'hangar-conta.cmd'     = @('%HANGAR_SERVER_BIN%" conta %*')
    'hangar-codex-tui.cmd' = @('hangar-server', ' codex-tui %*')
    'hangar-codex.cmd'     = @('bash', 'scripts\hangar-codex" %*')
    'hangar-codex-tui'     = @('hangar-server', ' codex-tui ')
    'hangar-codex'         = @("exec sh '")
}
$antigos = @()
foreach ($nome in $esperado.Keys) {
    $arquivo = Join-Path $bin $nome
    if (-not (Test-Path -LiteralPath $arquivo -PathType Leaf)) { continue }
    $texto = [IO.File]::ReadAllText($arquivo)
    $faltando = @($esperado[$nome] | Where-Object { -not $texto.Contains($_) })
    if ($faltando.Count -or $texto -match '(?i)hangar-python|python[0-9.]*\.exe|[\\/]py\.exe') { $antigos += $nome }
}
if ($antigos.Count) {
    [Console]::Error.WriteLine('##HANGAR-FALHA## lancadores ainda pelo Python em ' + $bin + ': ' + ($antigos -join ', '))
    exit 1
}
$pasta = Join-Path $Perfil '.hangar\passos'
New-Item -ItemType Directory -Force $pasta | Out-Null
New-Item -ItemType File -Force (Join-Path $pasta $passo) | Out-Null
"ok: lancadores do ~/.local/bin sem Python"
