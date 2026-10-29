# Bootstrap do primeiro servidor; o executor Rust assume as trocas seguintes.
param([switch]$NeverFail, [switch]$Fetch)
$ErrorActionPreference = 'Stop'
function Install-ServerBinaries {
    if ($env:HANGAR_UPDATE_OWNS_RUST -eq '1') {
        Write-Host 'binários Rust: quem baixa é o Atualizar, pelo commit escolhido'
        return 0
    }
    $root = Split-Path -Parent $PSScriptRoot
    $platform = 'windows-x86_64'
    if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') { return 2 }
    $branch = (& git -C $root rev-parse --abbrev-ref HEAD 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $branch -or $branch -in @('main','master','HEAD')) { $tag = 'server-latest' }
    else { $tag = 'server-' + ($branch -replace '[^A-Za-z0-9._-]', '-') }
    $base = 'https://github.com/jeffer1312/hangar/releases/download'
    $url = if ($env:HANGAR_SERVER_RELEASE_URL) { $tag = ''; $env:HANGAR_SERVER_RELEASE_URL.TrimEnd('/') } else { "$base/$tag" }
    $dest = if ($env:HANGAR_SERVER_BIN_DIR) { $env:HANGAR_SERVER_BIN_DIR } else { Join-Path $HOME '.hangar\bin' }
    $branchTag = ''; $missing = 'ausente'; $manifest = $null
    try {
        try { $manifest = Invoke-RestMethod -Uri "$url/server-latest.json" -TimeoutSec 30 }
        catch {
            if ($_.Exception.Response.StatusCode.value__ -ne 404 -or -not $tag -or $tag -eq 'server-latest') { throw }
            $branchTag = $tag
        }
        if (-not $branchTag -and $manifest -isnot [pscustomobject]) { throw 'manifesto não é um objeto' }
        if ($manifest -and $tag -and $tag -ne 'server-latest') {
            if (-not $manifest.files -or -not $manifest.files."$platform/hangar-server" -or -not $manifest.files."$platform/hangar-cano") {
                $branchTag = $tag; $missing = "sem build para $platform"; $manifest = $null
            }
        }
        if ($branchTag) { $url = "$base/server-latest"; $manifest = Invoke-RestMethod -Uri "$url/server-latest.json" -TimeoutSec 30 }
        if ($manifest -isnot [pscustomobject] -or -not $manifest.PSObject.Properties['files']) { throw 'manifesto não é um objeto com files' }
        New-Item -ItemType Directory -Force -Path $dest | Out-Null
    } catch { Write-Warning "binários Rust não baixados: não consegui ler o manifesto da release ($($_.Exception.Message))"; return 1 }
    $failed = $false; $kept = @(); $installed = @()
    foreach ($name in @('hangar-server','hangar-cano')) {
        $target = Join-Path $dest "$name.exe"
        Get-ChildItem -LiteralPath $dest -ErrorAction SilentlyContinue | Where-Object { -not $_.PSIsContainer -and $_.Name.StartsWith("$name.exe.old", [StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }
        if ($branchTag -and (Test-Path -LiteralPath $target)) { $kept += $name; continue }
        $entry = $manifest.files."$platform/$name"
        if ($entry.name -isnot [string] -or $entry.sha256 -isnot [string]) { Write-Warning "${name}: a release não traz build para $platform"; $failed = $true; continue }
        $sha = $entry.sha256.ToLowerInvariant()
        $current = if (Test-Path -LiteralPath $target -PathType Leaf) { try { (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() } catch { '' } } else { '' }
        if ($current -ne $sha) {
            $tmp = Join-Path $dest ".$name.$([guid]::NewGuid().ToString('N')).tmp"
            try {
                Invoke-WebRequest -UseBasicParsing -Uri "$url/$($entry.name)" -OutFile $tmp -TimeoutSec 200
                if ((Get-FileHash -LiteralPath $tmp -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sha) { throw 'o sha256 não confere com a release; o binário anterior ficou' }
                $old = $null
                if (Test-Path -LiteralPath $target) {
                    foreach ($number in 0..99) {
                        $candidate = if ($number -eq 0) { "$target.old" } else { "$target.old-$number" }
                        if (-not (Test-Path -LiteralPath $candidate)) { $old = $candidate; break }
                    }
                    if (-not $old) { throw "sem nome livre para $name.exe.old" }
                    Move-Item -LiteralPath $target -Destination $old
                }
                try { Move-Item -LiteralPath $tmp -Destination $target }
                catch { if ($old) { Move-Item -LiteralPath $old -Destination $target }; throw }
            } catch { Write-Warning "${name}: $($_.Exception.Message)"; $failed = $true; continue }
            finally { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
        }
        if ($branchTag) { $installed += $name }
    }
    if ($kept.Count) { Write-Warning "release $branchTag $missing; mantive os binários instalados ($($kept -join ', '))"; $failed = $true }
    if ($installed.Count) { Write-Warning "release $branchTag $missing; instalei os da main ($($installed -join ', '))"; $failed = $true }
    if ($failed) { return 1 }; return 0
}
$result = Install-ServerBinaries
$code = if ($result -is [array]) { $result[-1] } else { $result }
if ($NeverFail -or ($Fetch -and $code -ne 2)) { exit 0 }
exit [int]$code
