param([string]$InstallScript = (Join-Path $PSScriptRoot '..\install.ps1'),
      [string]$Checker = (Join-Path $PSScriptRoot 'windows-atalhos-sem-python.ps1'))
# O passo 2026-11-17-atalhos-windows-sem-python: com os lancadores que o install.ps1 da main gravava
# (pelo Python), a conferencia recusa e nao deixa a prova; com os que o install.ps1 atual grava, passa.
$ErrorActionPreference = 'Stop'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('hangar-atalhos-sem-python-' + [guid]::NewGuid().ToString('N'))
$prova = '.hangar\passos\2026-11-17-atalhos-windows-sem-python'
function Conferir([string]$perfil) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $Checker -Perfil $perfil 2>&1 | Out-Null
    $LASTEXITCODE
}
function Gravar([string]$bin, [hashtable]$arquivos) {
    New-Item -ItemType Directory -Force $bin | Out-Null
    foreach ($nome in $arquivos.Keys) { [IO.File]::WriteAllText((Join-Path $bin $nome), $arquivos[$nome]) }
}
try {
    $raizAtalhos = '%USERPROFILE%\.hangar\current'
    $bash = 'C:\Program Files\Git\bin\bash.exe'
    $binMsys = '/c/perfil/.local/bin'
    $shimRootMsys = '/c/perfil/.hangar/current'

    # O que o install.ps1 da main (2ad8e335e) gravava: hangar-conta pelo hangar-python.cmd e os do
    # Codex pelo python.exe (ou pelo py.exe -3), no .cmd e no atalho sh do Git Bash.
    foreach ($py in @('"C:\Users\ze\AppData\Local\Programs\Python\Python314\python.exe"', '"C:\Windows\py.exe" -3')) {
        $pyMsys = ($py -replace '\\', '/') -replace '"', '' -replace '^C:', '/c'
        $perfil = Join-Path $temporary ('main-' + [guid]::NewGuid().ToString('N'))
        Gravar (Join-Path $perfil '.local\bin') @{
            'hangar-conta.cmd'     = "@echo off`r`ncall `"%~dp0hangar-python.cmd`" `"$raizAtalhos\scripts\hangar-conta`" %*`r`n"
            'hangar-codex-tui.cmd' = "@echo off`r`nset `"PATH=%USERPROFILE%\.local\bin;%PATH%`"`r`n$py `"$raizAtalhos\scripts\hangar-codex-tui`" %*`r`n"
            'hangar-codex.cmd'     = "@echo off`r`nset `"PATH=%USERPROFILE%\.local\bin;%PATH%`"`r`n$py `"$raizAtalhos\scripts\hangar-codex`" %*`r`n"
            'hangar-codex-tui'     = "#!/bin/sh`n# Gerado pelo instalador do hangar (install.ps1 / install-hangar-send.sh).`nPATH='$binMsys':`$PATH; export PATH`nexec '$pyMsys' '$shimRootMsys/scripts/hangar-codex-tui' `"`$@`"`n"
            'hangar-codex'         = "#!/bin/sh`n# Gerado pelo instalador do hangar (install.ps1 / install-hangar-send.sh).`nPATH='$binMsys':`$PATH; export PATH`nexec '$pyMsys' '$shimRootMsys/scripts/hangar-codex' `"`$@`"`n"
        }
        if ((Conferir $perfil) -ne 1) { throw "os lancadores da main ($py) passaram na conferencia" }
        if (Test-Path -LiteralPath (Join-Path $perfil $prova)) { throw "a prova ficou com os lancadores da main ($py)" }
        # Um por vez: so o hangar-conta antigo ja basta para recusar.
        $so = Join-Path $temporary ('conta-' + [guid]::NewGuid().ToString('N'))
        Gravar (Join-Path $so '.local\bin') @{ 'hangar-conta.cmd' = [IO.File]::ReadAllText((Join-Path $perfil '.local\bin\hangar-conta.cmd')) }
        if ((Conferir $so) -ne 1) { throw 'o hangar-conta.cmd da main passou sozinho' }
    }

    # O que o install.ps1 atual grava, tirado do proprio instalador.
    $tokens = $null; $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile((Resolve-Path -LiteralPath $InstallScript).ProviderPath, [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw 'O instalador nao passou pelo parser PowerShell.' }
    function Get-Assignment([string]$name) {
        $node = $ast.Find({ param($n)
            $n -is [Management.Automation.Language.AssignmentStatementAst] -and $n.Left.Extent.Text -eq $name
        }, $true)
        if (-not $node) { throw "Atribuicao $name nao encontrada no instalador." }
        [scriptblock]::Create($node.Extent.Text)
    }
    $servidorTui = '%USERPROFILE%\.hangar\current\bin\hangar-server.exe'
    $servidorTuiMsys = '/c/perfil/.hangar/current/bin/hangar-server.exe'
    . (Get-Assignment '$conteudoConta')
    . (Get-Assignment '$lancadoresCodex')
    $novos = @{ 'hangar-conta.cmd' = $conteudoConta }
    foreach ($par in $lancadoresCodex) {
        $nome, $linhaCmd, $linhaSh = $par
        . (Get-Assignment '$corpo')
        . (Get-Assignment '$corpoSh')
        $novos["$nome.cmd"] = $corpo
        $novos[$nome] = $corpoSh
    }
    $perfil = Join-Path $temporary 'atual'
    Gravar (Join-Path $perfil '.local\bin') $novos
    if ((Conferir $perfil) -ne 0) { throw 'os lancadores do install.ps1 atual nao passaram na conferencia' }
    if (-not (Test-Path -LiteralPath (Join-Path $perfil $prova))) { throw 'a conferencia passou sem deixar a prova' }

    # Maquina sem nenhum desses lancadores: nada depende do Python, a prova fica.
    $vazio = Join-Path $temporary 'vazio'
    New-Item -ItemType Directory -Force $vazio | Out-Null
    if ((Conferir $vazio) -ne 0) { throw 'perfil sem lancadores recusado' }
    'ok: o passo recusa os lancadores da main e aceita os do install.ps1 atual'
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
