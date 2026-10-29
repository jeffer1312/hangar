# Prova isolada do bootstrap, sem rede nem alterações na instalação do usuário.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$bootstrap = Join-Path $PSScriptRoot 'install-server-binaries.ps1'
$bytes = [IO.File]::ReadAllBytes($bootstrap)
if ($bytes.Length -lt 3 -or $bytes[0] -ne 239 -or $bytes[1] -ne 187 -or $bytes[2] -ne 191) { throw 'O bootstrap exige BOM para o PowerShell 5.1.' }
$fixture = Get-Content -Raw -Encoding UTF8 (Join-Path $repo 'backend/tests/fixtures/contract/update-git/auth-release-windows.json') | ConvertFrom-Json
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
function global:git { $global:LASTEXITCODE = 0; if ($global:ProofCase.name.StartsWith('branch-')) { 'beta' } else { 'main' } }
function global:Invoke-RestMethod {
    param([string]$Uri,[int]$TimeoutSec)
    if ($TimeoutSec -ne 30) { throw 'Prazo incorreto do manifesto.' }
    [IO.File]::AppendAllText((Join-Path $global:ProofRoot 'calls'), "$Uri`n")
    if ($Uri.Contains('/server-beta/') -and $global:ProofCase.name -ne 'branch-missing-auth') {
        $exception = New-Object Exception 'missing'
        $exception | Add-Member -NotePropertyName Response -NotePropertyValue ([pscustomobject]@{StatusCode=[pscustomobject]@{value__=404}})
        throw $exception
    }
    $files = [ordered]@{}
    foreach ($name in @('hangar-auth','hangar-cano','hangar-server')) {
        $sha = [Security.Cryptography.SHA256]::Create()
        try { $hash = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes("new-$name"))).Replace('-','').ToLowerInvariant() } finally { $sha.Dispose() }
        $files["windows-x86_64/$name"] = [pscustomobject]@{name=$name;sha256=$hash}
    }
    if ($global:ProofCase.name -eq 'missing-auth' -or ($global:ProofCase.name.StartsWith('branch-') -and $Uri.Contains('/server-beta/'))) { $files.Remove('windows-x86_64/hangar-auth') }
    if ($global:ProofCase.name -eq 'invalid-auth-sha') { $files['windows-x86_64/hangar-auth'].sha256 = 'invalid' }
    return [pscustomobject]@{files=[pscustomobject]$files}
}
function global:Invoke-WebRequest {
    param([switch]$UseBasicParsing,[string]$Uri,[string]$OutFile,[int]$TimeoutSec)
    if ($TimeoutSec -ne 200) { throw 'Prazo incorreto do binário.' }
    [IO.File]::AppendAllText((Join-Path $global:ProofRoot 'calls'), "$Uri`n")
    $name = ($Uri -split '/')[-1]
    $kind = $name.Replace('hangar-', '')
    if ($global:ProofCase.name.Replace('branch-', '') -eq "$kind-download-failure") { throw 'fixture-download-failed' }
    $data = if ($global:ProofCase.name -eq "$kind-hash-failure") { 'wrong' } else { "new-$name" }
    [IO.File]::WriteAllBytes($OutFile,[Text.Encoding]::UTF8.GetBytes($data))
}
$env:PROOF_CASE = $global:ProofCase.name
$env:NODE_OPTIONS = '--require="' + (Join-Path $Root 'failure.cjs').Replace('\', '/') + '"'
& $Bootstrap
exit $LASTEXITCODE
'@
try {
    $probeFile = Join-Path $scratch 'probe.ps1'
    [IO.File]::WriteAllText($probeFile, $probe, [Text.UTF8Encoding]::new($true))
    foreach ($case in $fixture.cases) {
        if ($case.name.StartsWith('hold-') -or ($case.name.EndsWith('publish-failure') -and -not $case.name.StartsWith('branch-'))) { continue }
        $root = Join-Path $scratch $case.name
        New-Item -ItemType Directory -Path $root | Out-Null
        $dest = Join-Path $root 'dest'
        New-Item -ItemType Directory -Path $dest | Out-Null
        foreach ($entry in $case.initial.PSObject.Properties) {
            $target = Join-Path $dest $entry.Name
            New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
            [IO.File]::WriteAllText($target, $entry.Value, (New-Object Text.UTF8Encoding($false)))
        }
        $failure = 'const fs=require("fs"),path=require("path"),original=fs.renameSync;let failed=false;fs.renameSync=function(src,target){const n=process.env.PROOF_CASE;if(!failed&&process.argv[1]?.endsWith("server-binaries-publish.cjs")&&((n==="branch-server-publish-failure"&&path.basename(target)==="hangar-server.exe")||(n==="branch-metadata-publish-failure"&&path.basename(target)==="hangar-server.auth.json"))){failed=true;throw new Error("fixture-publication-failed");}return original(src,target);};'
        [IO.File]::WriteAllText((Join-Path $root 'failure.cjs'),$failure,(New-Object Text.UTF8Encoding($false)))
        $caseFile = Join-Path $root 'case.json'
        [IO.File]::WriteAllText($caseFile, ($case | ConvertTo-Json -Depth 20), [Text.UTF8Encoding]::new($true))
        $output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $probeFile -Root $root -CasePath $caseFile -Bootstrap $bootstrap 2>&1
        $expectedCode = if ($case.result.Count) { 1 } else { 0 }
        if ($LASTEXITCODE -ne $expectedCode) { throw "$($case.name): código $LASTEXITCODE, esperado $($case.code): $output" }
        $urls = @([IO.File]::ReadAllLines((Join-Path $root 'calls'), [Text.Encoding]::UTF8))
        if (($urls | ConvertTo-Json -Compress) -cne (@($case.urls | ForEach-Object { $_.Replace('https://release.invalid','https://github.com/jeffer1312/hangar/releases/download') }) | ConvertTo-Json -Compress)) { throw "$($case.name): URLs divergentes." }
        $files = @(Get-ChildItem -LiteralPath (Join-Path $root 'dest') -File -Recurse)
        $expectedCount = @($case.after_publish.PSObject.Properties).Count
        if ($files.Count -ne $expectedCount) { throw "$($case.name): número de arquivos divergente." }
        foreach ($entry in $case.after_publish.PSObject.Properties) {
            if ([IO.File]::ReadAllText((Join-Path (Join-Path $root 'dest') $entry.Name)) -cne $entry.Value) { throw "$($case.name): conteúdo divergente em $($entry.Name)." }
        }
        $expectedMessages = @($case.result | Where-Object { $_.Contains('instalei os da main') })
        $actualMessages = @($output | ForEach-Object { [string]$_ } | Where-Object { $_.Contains('instalei os da main') } | ForEach-Object { $_ -replace '^WARNING: ', '' -replace '^AVISO: ', '' })
        if (($expectedMessages | ConvertTo-Json -Compress) -cne ($actualMessages | ConvertTo-Json -Compress)) { throw "$($case.name): aviso de publicação divergente: $actualMessages" }
        Write-Output "ok $($case.name)"
    }
} finally { Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue }
Write-Output 'Bootstrap PowerShell 5.1: fallback, conteúdo, prazos e BOM conferidos.'
