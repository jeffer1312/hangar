# Testa o modo pacote do install.ps1 (a pasta tem manifest.json e nao tem .git) sem instalar nada:
# extrai pelo AST os trechos do pacote e roda cada um com dubles, numa versao falsa em %TEMP%.
# Uso (Windows PowerShell 5.1):
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-install-pacote.ps1
$ErrorActionPreference = 'Stop'
$scripts = $PSScriptRoot
$installer = Join-Path (Split-Path -Parent $scripts) 'install.ps1'
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }

$script:falhas = 0
function Assert($condition, $message) {
    if ($condition) { [Console]::WriteLine("ok   $message") } else { [Console]::WriteLine("FAIL $message"); $script:falhas++ }
}
# Dubles: cada mensagem vira uma linha "<tipo> <texto>".
$script:saida = New-Object 'System.Collections.Generic.List[string]'
function Ok($m) { $script:saida.Add("ok $m") }
function Nota($m) { $script:saida.Add("nota $m") }
function Falha($rotulo, $motivo) { $script:saida.Add("falha ${rotulo}: $motivo") }
function Limpa { $script:saida.Clear() }
function Saida { return ($script:saida -join "`n") }

# Ler-Texto vem do windows-wrappers.ps1, como no instalador.
$wAst = [Management.Automation.Language.Parser]::ParseFile((Join-Path $scripts 'windows-wrappers.ps1'), [ref]$tokens, [ref]$parseErrors)
. ([scriptblock]::Create($wAst.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Ler-Texto' }, $true).Extent.Text))

# O trecho do instalador cujo texto comeca com $inicio e contem $marca. $HOME e somente leitura no
# PowerShell: o trecho le $casa no lugar dele.
function Trecho([string]$inicio, [string]$marca) {
    $no = $ast.Find({ param($n) $n.Extent.Text.StartsWith($inicio) -and $n.Extent.Text.Contains($marca) }, $true)
    if (-not $no) { throw "trecho ausente: $inicio ... $marca" }
    return [scriptblock]::Create($no.Extent.Text.Replace('$HOME', '$casa'))
}

$raizTeste = Join-Path $env:TEMP ('hangar-install-pacote-' + [guid]::NewGuid().ToString('N'))
$casa = Join-Path $raizTeste 'casa'
$versao = Join-Path $raizTeste 'versao'
New-Item -ItemType Directory -Force -Path $casa, $versao | Out-Null
try {
    # --- Deteccao do pacote: manifest.json sem .git ---
    $deteccao = Trecho '$pacote = ' 'manifest.json'
    $raiz = $versao
    . $deteccao; Assert (-not $pacote) 'deteccao: sem manifest.json nao e pacote'
    '{}' | Set-Content -Encoding ASCII (Join-Path $versao 'manifest.json')
    . $deteccao; Assert ($pacote) 'deteccao: manifest.json sem .git e pacote'
    New-Item -ItemType Directory -Force -Path (Join-Path $versao '.git') | Out-Null
    . $deteccao; Assert (-not $pacote) 'deteccao: checkout com manifest.json (modo desenvolvedor) nao e pacote'
    Remove-Item -Recurse -Force (Join-Path $versao '.git')

    # --- .env: no pacote sempre em ~\.hangar\config ---
    $env_ = Trecho '$envFile = ' 'config'
    $pacote = $true; . $env_
    Assert ($envFile -eq "$casa\.hangar\config\.env") 'pacote: .env em ~\.hangar\config'
    $pacote = $false; . $env_
    Assert ($envFile -eq "$versao\backend\.env") 'checkout nao migrado: .env do backend'

    # --- Tela web: a da dist_key em ~\.hangar\ui, ou pendencia ---
    $tela = Trecho 'if ($pacote) {' '$precisa = $false'
    $chave = 'a' * 64
    [IO.File]::WriteAllText((Join-Path $versao 'manifest.json'), "{`n  `"channel`": `"latest`",`n  `"dist_key`": `"$chave`",`n  `"version`": `"0.1.0.42`"`n}`n")
    $pacote = $true; $precisa = $true
    New-Item -ItemType Directory -Force -Path (Join-Path $casa ".hangar\ui\$chave") | Out-Null
    '' | Set-Content (Join-Path $casa ".hangar\ui\$chave\index.html")
    Limpa; . $tela
    Assert ((Saida) -match 'a tela vem pronta com o pacote') 'tela: pronta quando a pasta da chave existe'
    Assert (-not $precisa) 'tela: nada a compilar no pacote'
    Remove-Item -Recurse -Force (Join-Path $casa '.hangar\ui')
    Limpa; . $tela
    Assert ((Saida) -match "falha tela web: nao esta em ~\\\.hangar\\ui\\$chave") 'tela: ausente vira pendencia com a chave'
    '{ "web_ui": false }' | Set-Content -Encoding ASCII (Join-Path $casa '.hangar\web-ui.json')
    Limpa; . $tela
    Assert ((Saida) -match 'tela web desligada' -and (Saida) -notmatch 'falha') 'tela: desligada nao e pendencia'
    Remove-Item (Join-Path $casa '.hangar\web-ui.json')
    '{}' | Set-Content -Encoding ASCII (Join-Path $versao 'manifest.json')
    Limpa; . $tela
    Assert ((Saida) -match 'falha tela web: .*sem dist_key') 'tela: manifesto sem dist_key vira pendencia'

    # --- Passos marcados pelo hangar-server do pacote, com a saida visivel ---
    New-Item -ItemType Directory -Force -Path (Join-Path $versao 'bin') | Out-Null
    $log = Join-Path $raizTeste 'server.log'
    Add-Type -OutputType ConsoleApplication -OutputAssembly (Join-Path $versao 'bin\hangar-server.exe') -TypeDefinition @'
using System; using System.IO;
public static class FakeMark {
    public static int Main(string[] args) {
        File.AppendAllText(Environment.GetEnvironmentVariable("FAKE_LOG"), string.Join(" ", args) + "\r\n");
        Console.WriteLine("aviso: passo malformado ignorado: torto.md (docs/atualizacoes)");
        string rc = Environment.GetEnvironmentVariable("FAKE_RC");
        return string.IsNullOrEmpty(rc) ? 0 : int.Parse(rc);
    }
}
'@
    $env:FAKE_LOG = $log
    $passos = Trecho 'if ($pacote -and -not $Update' 'install-package --mark-steps'
    $raiz = $versao; $pacote = $true; $Update = $false; $pacoteNovo = $true
    $marcador = Join-Path $casa '.hangar\fresh-package'
    [IO.File]::WriteAllText($marcador, '')
    Limpa; . $passos
    Assert ((Get-Content $log) -eq 'install-package --mark-steps') 'passos: --mark-steps pelo hangar-server do pacote'
    Assert ((Saida) -match 'nota aviso: passo malformado ignorado: torto.md') 'passos: passo malformado aparece'
    Assert ((Saida) -match 'ok passos de atualizacao marcados') 'passos: ok'
    Assert (-not (Test-Path -LiteralPath $marcador)) 'passos: marcador de instalacao nova apagado'
    $env:FAKE_RC = '1'
    [IO.File]::WriteAllText($marcador, '')
    Limpa; . $passos
    Assert ((Saida) -match 'falha passos de atualizacao: .*--mark-steps') 'passos: falha vira pendencia com o comando'
    Assert (Test-Path -LiteralPath $marcador) 'passos: falha deixa o marcador para a proxima'
    Remove-Item -LiteralPath $marcador
    Remove-Item Env:FAKE_RC
    Remove-Item $log
    $Update = $true
    Limpa; . $passos
    Assert (-not (Test-Path $log)) 'passos: -Update nao marca'
    $Update = $false; $pacoteNovo = $false
    Limpa; . $passos
    Assert (-not (Test-Path $log)) 'passos: instalacao que ja existia nao marca'
    Assert ((Saida) -match 'nota .*nenhum passo de atualizacao marcado.*Atualizar') 'passos: diz que os pendentes ficam para o Atualizar'
    # O bootstrap manda a marca pelo ambiente; o instalador a le e tira dela.
    $texto0 = [IO.File]::ReadAllText($installer)
    Assert ($texto0.Contains("`$pacoteNovo = (`$env:HANGAR_FRESH_PACKAGE -eq '1')") -and $texto0.Contains('Remove-Item Env:HANGAR_FRESH_PACKAGE')) 'passos: marca de instalacao nova vem do HANGAR_FRESH_PACKAGE'

    # --- O que o pacote pula, e onde ---
    $texto = [IO.File]::ReadAllText($installer) -replace "`r`n", "`n"
    Assert ($texto.Contains("if (`$pacote) {`n    Ok 'dependencias: vieram no pacote'`n} else {`n`$rcSync = Nativo uv sync --quiet")) 'pacote: sem uv sync'
    Assert ($texto.Contains("if (-not `$pacote) {`nInstale 'uv'")) 'pacote: sem instalar uv'
    Assert ($texto.Contains("    if (`$pacote) {`n        `$rcRust = 'pacote'`n    } elseif")) 'pacote: sem rust_release'
    Assert ($texto.Contains("if (-not `$Update -and -not `$pacote) {`n    Titulo 'Passos de atualizacao'")) 'pacote: marcar_todos do uv fica de fora'
    Assert ($texto.Contains("} elseif (`$pacote) {`n    Ok 'o pacote se atualiza pelo Atualizar do app, sem git pull'")) 'pacote: sem hook de git pull'
    Assert ($texto.Contains("if (`$pacote) { `$pyFumaca = @(`$pyBackend) }")) 'pacote: fumaca pelo Python do pacote'
    Assert (-not ($texto -match '\$pyFumaca = if')) 'pacote: fumaca nunca pelo valor de um if (desembrulha o array de um item)'
    Assert ($texto.Contains("if (`$pacote) { `$pyExe = Join-Path `$HOME '.hangar\current\python\python.exe' }")) 'pacote: lancadores pelo Python do pacote, via current'
    $fumaca = $texto.IndexOf("Ok 'o multiplexador mata sessao'")
    $marca = $texto.IndexOf('install-package --mark-steps 2>&1')
    $fim = $texto.IndexOf('# -- Confere e entrega o acesso')
    Assert ($fumaca -gt 0 -and $marca -gt $fumaca -and $fim -gt $marca) 'pacote: passos marcados depois da fumaca'
    $saiChecar = $texto.IndexOf("if (`$SoChecar) {`n    `$pyVenvCheck")
    Assert ($saiChecar -gt 0 -and $saiChecar -lt $marca) 'pacote: -SoChecar sai antes de marcar passos'
    $bytes = [IO.File]::ReadAllBytes($installer)
    Assert ($bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) 'install.ps1 continua UTF-8 com BOM'
} finally {
    Remove-Item Env:FAKE_LOG, Env:FAKE_RC -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $raizTeste -ErrorAction SilentlyContinue
}
if ($script:falhas) { [Console]::WriteLine("$($script:falhas) falha(s)"); exit 1 }
[Console]::WriteLine('tudo ok')
