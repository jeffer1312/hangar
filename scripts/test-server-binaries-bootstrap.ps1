# Prova isolada do bootstrap, sem rede nem alterações na instalação do usuário.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$bootstrap = Join-Path $PSScriptRoot 'install-server-binaries.ps1'
$bytes = [IO.File]::ReadAllBytes($bootstrap)
if ($bytes.Length -lt 3 -or $bytes[0] -ne 239 -or $bytes[1] -ne 187 -or $bytes[2] -ne 191) { throw 'O bootstrap exige BOM para o PowerShell 5.1.' }
$fixture = Get-Content -Raw -Encoding UTF8 (Join-Path $repo 'backend/tests/fixtures/contract/update-git/bootstrap-windows.json') | ConvertFrom-Json
$scratch = Join-Path ([IO.Path]::GetTempPath()) ('hangar-bootstrap-proof-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null
$probe = @'
param([string]$Root,[string]$CasePath,[string]$Bootstrap)
$ErrorActionPreference = 'Stop'
$global:ProofRoot = $Root
$global:ProofCase = Get-Content -Raw -Encoding UTF8 $CasePath | ConvertFrom-Json
$env:HANGAR_SERVER_BIN_DIR = Join-Path $Root 'dest'
$env:HANGAR_SERVER_RELEASE_URL = ''
$env:HANGAR_UPDATE_OWNS_RUST = ''
function global:git { $global:LASTEXITCODE = 0; 'beta' }
function global:Invoke-RestMethod {
    param([string]$Uri,[int]$TimeoutSec)
    if ($TimeoutSec -ne 30) { throw 'Prazo incorreto do manifesto.' }
    [IO.File]::AppendAllText((Join-Path $global:ProofRoot 'calls'), "$Uri`n")
    $files = [ordered]@{}
    foreach ($name in @('hangar-auth','hangar-cano','hangar-server')) {
        $sha = [Security.Cryptography.SHA256]::Create()
        try { $hash = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes("new-$name"))).Replace('-','').ToLowerInvariant() } finally { $sha.Dispose() }
        $files["windows-x86_64/$name"] = [pscustomobject]@{name=$name;sha256=$hash}
    }
    if ($Uri.Contains('/server-beta/')) { $files['windows-x86_64/hangar-server'] = $global:ProofCase.entry }
    return [pscustomobject]@{files=[pscustomobject]$files}
}
function global:Invoke-WebRequest {
    param([switch]$UseBasicParsing,[string]$Uri,[string]$OutFile,[int]$TimeoutSec)
    if ($TimeoutSec -ne 200) { throw 'Prazo incorreto do binário.' }
    [IO.File]::AppendAllText((Join-Path $global:ProofRoot 'calls'), "$Uri`n")
    [IO.File]::WriteAllBytes($OutFile,[Text.Encoding]::UTF8.GetBytes('new-' + ($Uri -split '/')[-1]))
}
& $Bootstrap
exit $LASTEXITCODE
'@
try {
    $probeFile = Join-Path $scratch 'probe.ps1'
    [IO.File]::WriteAllText($probeFile, $probe, [Text.UTF8Encoding]::new($true))
    foreach ($case in $fixture.cases) {
        $root = Join-Path $scratch $case.name
        New-Item -ItemType Directory -Path $root | Out-Null
        $caseFile = Join-Path $root 'case.json'
        [IO.File]::WriteAllText($caseFile, ($case | ConvertTo-Json -Depth 20), [Text.UTF8Encoding]::new($true))
        $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $probeFile -Root $root -CasePath $caseFile -Bootstrap $bootstrap 2>&1
        if ($LASTEXITCODE -ne $case.code) { throw "$($case.name): código $LASTEXITCODE, esperado $($case.code): $output" }
        $urls = @([IO.File]::ReadAllLines((Join-Path $root 'calls'), [Text.Encoding]::UTF8))
        if (($urls | ConvertTo-Json -Compress) -cne ($case.urls | ConvertTo-Json -Compress)) { throw "$($case.name): URLs divergentes. Recebidas: $($urls | ConvertTo-Json -Compress); esperadas: $($case.urls | ConvertTo-Json -Compress)." }
        $files = @(Get-ChildItem -LiteralPath (Join-Path $root 'dest') -File -Recurse)
        $expectedCount = @($case.files.PSObject.Properties).Count
        if ($files.Count -ne $expectedCount) { throw "$($case.name): número de arquivos divergente." }
        foreach ($entry in $case.files.PSObject.Properties) {
            if ([IO.File]::ReadAllText((Join-Path (Join-Path $root 'dest') $entry.Name)) -cne $entry.Value) { throw "$($case.name): conteúdo divergente em $($entry.Name)." }
        }
        Write-Output "ok $($case.name)"
    }
} finally { Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue }
Write-Output 'Bootstrap PowerShell 5.1: fallback, conteúdo, prazos e BOM conferidos.'
