param([string]$InstallScript = (Join-Path $PSScriptRoot '..\install.ps1'))
$ErrorActionPreference = 'Stop'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('hangar-shims-current-' + [guid]::NewGuid().ToString('N'))
try {
    # O instalador roda de dentro da pasta da versão; a poda a apaga, então os atalhos vão pelo `current`.
    $fakeHome = Join-Path $temporary 'perfil'
    $raiz = Join-Path $fakeHome '.hangar\versions\0.1.0.1'
    New-Item -ItemType Directory -Force $raiz, (Join-Path $fakeHome '.hangar\current') | Out-Null
    $bash = 'C:\Program Files\Git\bin\bash.exe'
    $binMsys = '/c/perfil/.local/bin'
    $py = 'C:\py\python.exe'; $a = ''; $nome = 'hangar-codex'; $pyMsysTui = '/c/py/python.exe'

    $tokens = $null; $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $InstallScript).ProviderPath, [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw 'O instalador não passou pelo parser PowerShell.' }
    function Get-Assignment([string]$name) {
        $node = $ast.Find({ param($n)
            $n -is [Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq $name
        }, $true)
        if (-not $node) { throw "Atribuição $name não encontrada no instalador." }
        [scriptblock]::Create($node.Extent.Text.Replace('$HOME', '$fakeHome'))
    }
    . (Get-Assignment '$raizAtalhos')
    . (Get-Assignment '$shimRootMsys')

    $version = '0.1.0.1'
    $failures = @()
    foreach ($name in '$conteudo', '$conteudoPreview', '$conteudoFolha', '$conteudoMcp', '$corpo', '$corpoSh', '$corpoCp', '$corpoPreview') {
        . (Get-Assignment $name)
        $text = Get-Variable -Name $name.TrimStart('$') -ValueOnly
        if ($text -like "*$version*" -or $text -notmatch '[\\/]\.hangar[\\/]current[\\/]scripts[\\/]') { $failures += "$name -> $text" }
    }
    if ($failures.Count) { throw ("Atalho fora do current:`n" + ($failures -join "`n")) }
    'ok: todos os atalhos apontam para o current'
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
