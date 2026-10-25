# Testa as decisoes do modo -App do install.ps1 sem instalar nada: extrai as funcoes pelo AST e
# roda cada uma com dubles. Uso (Windows PowerShell 5.1):
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-install-app.ps1
$ErrorActionPreference = 'Stop'
$installer = Join-Path (Split-Path -Parent $PSScriptRoot) 'install.ps1'
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }

$script:falhas = 0
function Assert($condition, $message) {
    if ($condition) { [Console]::WriteLine("ok   $message") } else { [Console]::WriteLine("FAIL $message"); $script:falhas++ }
}
# Dubles: Write-Host vira lista (as marcas sao conferidas por linha) e nada pode esperar teclado.
$script:saida = New-Object 'System.Collections.Generic.List[string]'
function Write-Host { param([Parameter(Position = 0)]$Object, $ForegroundColor, [switch]$NoNewline) $script:saida.Add("$Object") }
function Read-Host { throw 'Read-Host chamado: o -App nao pode esperar teclado' }
function Limpa { $script:saida.Clear() }

foreach ($name in @('Nota', 'Ok', 'Falta', 'Erro', 'Pergunte-Mesmo', 'Pergunte', 'Eleva-E-Roda', 'Mark-Step', 'Add-AppPending')) {
    $def = $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true)
    if (-not $def) { throw "Funcao ausente: $name" }
    . ([scriptblock]::Create($def.Extent.Text))
}

# Parametros novos
$params = @{}
foreach ($p in $ast.ParamBlock.Parameters) { $params[$p.Name.VariablePath.UserPath] = $p }
Assert ($params.ContainsKey('App') -and $params['App'].StaticType -eq [switch]) 'install.ps1 aceita -App (switch)'
Assert ($params.ContainsKey('SemNativo') -and $params['SemNativo'].StaticType -eq [switch]) 'install.ps1 aceita -SemNativo (switch)'
$vs = @($params['Tailscale'].Attributes | Where-Object { $_.TypeName.Name -eq 'ValidateSet' })
Assert ($vs.Count -eq 1 -and (($vs[0].PositionalArguments | ForEach-Object { $_.Value }) -join ',') -eq 'sim,nao') '-Tailscale so aceita sim ou nao'

# Respostas pela tabela
$App = $true; $Sim = $false; $Avancado = $false; $script:Interativo = $false
Assert ((Pergunte-Mesmo 'Ligar o Modo Desenvolvedor?') -eq $true) '-App: Pergunte-Mesmo responde sim sem ler teclado'
Assert ((Pergunte 'Instalar (recomendado)?') -eq $true) '-App: Pergunte responde sim'
$App = $false
Assert ((Pergunte-Mesmo 'Ligar?') -eq $false) 'sem -App e sem terminal: Pergunte-Mesmo continua nao'
Assert ((Pergunte 'Instalar?') -eq $false) 'sem -App e sem terminal: Pergunte continua nao'

# UAC sem terminal
function EhAdmin { return $false }
$script:startCalls = 0
function Start-Process { param($FilePath, $Verb, [switch]$Wait, [switch]$PassThru, $ArgumentList) $script:startCalls++; return [pscustomobject]@{ ExitCode = 0 } }
$PowerShellExe = 'powershell.exe'
$App = $true; $script:Interativo = $false
Assert ((Eleva-E-Roda 'liberar a porta' 'exit 0') -eq $true) '-App: Eleva-E-Roda pede UAC mesmo sem terminal'
Assert ($script:startCalls -eq 1) '-App: o UAC foi de fato pedido'
$App = $false
Assert ((Eleva-E-Roda 'liberar a porta' 'exit 0') -eq $false) 'sem -App e sem terminal: Eleva-E-Roda recusa'
Assert ($script:startCalls -eq 1) 'sem -App: nenhum UAC pedido'

# Marcas de etapa
$script:currentStep = ''
$App = $false; Limpa; Mark-Step 'preparar' 'fazendo'
Assert ($script:saida.Count -eq 0) 'sem -App: nenhuma marca de etapa'
$App = $true; Limpa; Mark-Step 'preparar' 'fazendo'; Mark-Step 'preparar' 'ok'
Assert (($script:saida -join '|') -eq '##HANGAR-PASSO## preparar fazendo|##HANGAR-PASSO## preparar ok') '-App: marcas de etapa na sintaxe do contrato'
Assert ($script:currentStep -eq 'preparar') 'fazendo guarda a etapa que vai no PASSO falhou'

# Guardas no texto
$texto = [IO.File]::ReadAllText($installer)
Assert ($texto.Contains('if ($vivo -and -not $Update -and -not $App) {')) '-App nao abre app nem navegador no fim'
Assert ($texto.Contains('if ($tokenFim -and $App) {')) '-App nao imprime o token no resumo'
Assert ($texto.Contains('Remove-Item Env:HANGAR_TOKEN')) 'HANGAR_TOKEN sai do ambiente'
Assert ($texto.Contains('Write-Host "##HANGAR-FIM## $($script:finalState)"')) 'a FIM sai no finally'
$bytes = [IO.File]::ReadAllBytes($installer)
Assert ($bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) 'install.ps1 continua UTF-8 com BOM'

# Senha do celular: o que o python-dotenv reinterpreta e recusado no -App e na pergunta do terminal
$def = $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Test-ForbiddenToken' }, $true)
if (-not $def) { throw 'Funcao ausente: Test-ForbiddenToken' }
. ([scriptblock]::Create($def.Extent.Text))
foreach ($ruim in @('senha#forte', 'senha$forte', "senha'forte", 'senha"forte', 'senha\forte', ' senhaforte', "senhaforte`t", ("senha " + [char]0xE7 + [char]0xE3 + "o"))) {
    Assert (Test-ForbiddenToken $ruim) "senha recusada: [$ruim]"
}
Assert (-not (Test-ForbiddenToken 'senha boa 123')) 'espaco no meio continua valendo'
Assert ($texto.Contains('if (Test-ForbiddenToken $token) { Erro $tokenForbiddenText; continue }')) 'a pergunta do terminal recusa os mesmos caracteres'

$tokenIf = $ast.Find({ param($n) $n -is [Management.Automation.Language.IfStatementAst] -and $n.Clauses[0].Item1.Extent.Text -eq '$temToken' }, $true)
$tokenForbiddenText = 'FRASE-PROIBIDA'
$script:gravado = $null
$script:pare = $null
function Set-EnvKey { param($Chave, $Valor) $script:gravado = $Valor }
function Pare($mensagem, $dicas) { $script:pare = $mensagem; throw 'PARE' }
foreach ($n in @('Token-Aleatorio')) {
    $d = $ast.Find({ param($x) $x -is [Management.Automation.Language.FunctionDefinitionAst] -and $x.Name -eq $n }, $true)
    . ([scriptblock]::Create($d.Extent.Text))
}
function Run-TokenStep($valor) {
    $script:gravado = $null; $script:pare = $null; Limpa
    $temToken = $false; $App = $true; $tokenDoApp = $valor
    try { . ([scriptblock]::Create($tokenIf.Extent.Text)) } catch { if ("$_" -ne 'PARE') { throw } }
}
Run-TokenStep 'senha#forte'
Assert ($script:pare -and $script:pare.Contains('FRASE-PROIBIDA') -and -not $script:gravado) '-App: senha com # para a instalacao sem gravar'
Assert (-not $script:pare.Contains('senha#forte') -and -not ($script:saida -join '|').Contains('senha#forte')) '-App: a senha recusada nao aparece na saida'
Run-TokenStep 'curta'
Assert ($script:pare -and -not $script:gravado) '-App: senha curta para a instalacao'
Run-TokenStep 'senha boa 123'
Assert (-not $script:pare -and $script:gravado -eq 'senha boa 123') '-App: senha valida gravada como veio'
Run-TokenStep ''
Assert (-not $script:pare -and $script:gravado -match '^[0-9a-f]{48}$') '-App: senha vazia gera a aleatoria'

# Roda do mouse: no -App o psmux so e reiniciado com -ConsertarRoda
$askIf = $ast.Find({ param($n) $n -is [Management.Automation.Language.IfStatementAst] -and $n.Clauses[0].Item1.Extent.Text -eq "`$script:psmuxConsole -eq 'perguntar'" }, $true)
$runIf = $ast.Find({ param($n) $n -is [Management.Automation.Language.IfStatementAst] -and $n.Clauses[0].Item1.Extent.Text -eq "`$script:psmuxConsole -eq 'pular'" }, $true)
$script:conptyCalls = 0
function FakePowerShell { if ("$args" -match 'install-psmux-conpty\.ps1') { $script:conptyCalls++ }; $global:LASTEXITCODE = 1 }
# O ramo 'fazer' so encerra o psmux se o console instalou; o duble garante que nada real e derrubado.
function Psmux-Encerrar-Antigos { throw 'Psmux-Encerrar-Antigos chamado no teste' }
$PowerShellExe = 'FakePowerShell'
$raiz = 'C:\hangar-falso'
$script:psmuxSessoes = 2
$App = $true
foreach ($caso in @(@{ roda = $false; esperado = 0 }, @{ roda = $true; esperado = 1 })) {
    $ConsertarRoda = $caso.roda; $script:conptyCalls = 0; $script:faltaRodaPsmux = $false; Limpa
    $script:pendencias = @(); $script:pendingCodes = @{}
    $script:psmuxConsole = 'perguntar'
    . ([scriptblock]::Create($askIf.Extent.Text))
    . ([scriptblock]::Create($runIf.Extent.Text))
    Assert ($script:conptyCalls -eq $caso.esperado) "-App -ConsertarRoda:$($caso.roda): install-psmux-conpty.ps1 chamado $($caso.esperado) vez(es)"
}

# Preambulo do -App: a codepage dos .cmd e lida antes da troca para UTF-8, e o Python sai em UTF-8
$iCp = $texto.IndexOf('$script:launcherCodePage = [Console]::OutputEncoding.CodePage')
$iUtf = $texto.IndexOf('try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false } catch { }')
Assert ($iCp -ge 0 -and $iCp -lt $iUtf) 'a codepage dos .cmd e guardada antes da troca para UTF-8'
$appIf = $ast.Find({ param($n) $n -is [Management.Automation.Language.IfStatementAst] -and $n.Clauses[0].Item1.Extent.Text -eq '$App' -and $n.Extent.Text.Contains('HANGAR-PROTOCOLO') }, $true)
Assert ($appIf -and $appIf.Extent.Text.Contains("`$env:PYTHONIOENCODING = 'utf-8'")) '-App: o Python escreve em UTF-8'
Assert ($appIf -and $appIf.Extent.Text.Contains("`$script:launcherCodePage = [int](Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Nls\CodePage').OEMCP")) '-App: os .cmd vao na OEM do sistema, nao na do console herdado'
$d = $ast.Find({ param($x) $x -is [Management.Automation.Language.FunctionDefinitionAst] -and $x.Name -eq 'Escrever-Lancador' }, $true)
. ([scriptblock]::Create($d.Extent.Text))
$cmdFile = Join-Path $env:TEMP ("hangar-test-" + [guid]::NewGuid().ToString('N') + '.cmd')
$encAnt = [Console]::OutputEncoding
try {
    $script:launcherCodePage = 850
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
    [void](Escrever-Lancador $cmdFile ("@echo C:\Users\Jo" + [char]0x00E3 + "o") -Tipo cmd)
    $b = [IO.File]::ReadAllBytes($cmdFile)
    Assert ($b -contains 0xC6 -and -not ($b -contains 0xC3)) '-App: o .cmd sai na codepage guardada (850), nao em UTF-8'
} finally {
    [Console]::OutputEncoding = $encAnt
    Remove-Item $cmdFile -ErrorAction SilentlyContinue
}

# Outra instalacao segurando a trava: a FIM continua sendo a ultima linha
$mutex = New-Object Threading.Mutex($false, 'Local\HangarInstall')
$segurou = $false
try { $segurou = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $segurou = $true }
if ($segurou) {
    $out = Join-Path $env:TEMP ("hangar-test-" + [guid]::NewGuid().ToString('N') + '.txt')
    try {
        & cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File `"$installer`" -App -Tailscale nao > `"$out`" 2>&1 < NUL"
        $linhas = @(Get-Content $out | Where-Object { $_.Trim() })
        Assert ($linhas.Count -and $linhas[-1] -eq '##HANGAR-FIM## falhou') "-App com a trava ocupada: ultima linha FIM falhou (veio: $($linhas[-1]))"
        Assert (@($linhas | Where-Object { $_ -match 'Outra instalacao' }).Count -eq 1) '-App com a trava ocupada: o motivo aparece'
    } finally {
        $mutex.ReleaseMutex()
        Remove-Item $out -ErrorAction SilentlyContinue
    }
} else {
    [Console]::WriteLine('skip trava ocupada: ha uma instalacao de verdade rodando')
}
$mutex.Dispose()

# --- Itens, codigos, pendencias e link ---
foreach ($name in @('Mark-Item', 'Mark-ItemSince', 'Add-AppPending', 'Send-TailscaleLink', 'Policy-Locked', 'Instale')) {
    $def = $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true)
    if (-not $def) { throw "Funcao ausente: $name" }
    . ([scriptblock]::Create($def.Extent.Text))
}
$App = $true; Limpa
Mark-Item 'psmux' 'fila' 'psmux (multiplexador)'
Assert ($script:saida[0] -eq '##HANGAR-ITEM## psmux fila psmux (multiplexador)') 'ITEM na sintaxe do contrato'
$App = $false; Limpa; Mark-Item 'psmux' 'ok' 'psmux'
Assert ($script:saida.Count -eq 0) 'sem -App: nenhum ITEM'
$App = $true

Limpa
Send-TailscaleLink 'To authenticate, visit: https://login.tailscale.com/a/abc123'
Send-TailscaleLink 'Success.'
Assert (($script:saida -join '|') -eq '##HANGAR-LINK## tailscale-login https://login.tailscale.com/a/abc123') 'LINK do login sai so da linha com o link'

$script:pendencias = @(); $script:pendingCodes = @{}
Add-AppPending 'tailscale serve' 'tailscale-https'
Assert (($script:pendencias -contains 'tailscale serve') -and $script:pendingCodes['tailscale serve'] -eq 'tailscale-https') 'pendencia guarda o codigo'
Limpa; Mark-ItemSince 'servicos' 0 'inicio automatico'
Assert ($script:saida[0] -eq '##HANGAR-ITEM## servicos pendente inicio automatico') 'parte que somou pendencia fica pendente'
Limpa; Mark-ItemSince 'servicos' 1 'inicio automatico'
Assert ($script:saida[0] -eq '##HANGAR-ITEM## servicos ok inicio automatico') 'parte sem pendencia nova fica ok'

function Get-ExecutionPolicy { param($Scope) if ($Scope -eq 'MachinePolicy') { return 'AllSigned' } return 'Undefined' }
Assert (Policy-Locked) 'regra da TI (AllSigned na maquina) conta como travada'
function Get-ExecutionPolicy { param($Scope) return 'Undefined' }
Assert (-not (Policy-Locked)) 'sem regra da TI nao esta travada'

function Tem($cmd) { return $false }
function Nativo { return 1 }
function Atualiza-Path { }
function Test-Internet { return $false }
$SoChecar = $false; $Update = $false; $script:depsError = ''; $script:pendencias = @(); Limpa
[void](Instale 'psmux (multiplexador)' 'psmux' 'marlocarlo.psmux' 'sem ele nao existe sessao')
Assert ($script:depsError -eq 'sem-internet') 'Instale sem internet guarda sem-internet'
$itens = @($script:saida | Where-Object { $_ -like '##HANGAR-ITEM## psmux *' })
Assert (($itens -join '|') -eq '##HANGAR-ITEM## psmux fazendo psmux (multiplexador)|##HANGAR-ITEM## psmux falhou psmux (multiplexador)') 'Instale marca fazendo e falhou'
$SoChecar = $true; Limpa
[void](Instale 'uv' 'uv' 'astral-sh.uv' 'gerencia o venv do backend')
Assert ($script:saida -contains '##HANGAR-ITEM## uv fila uv') 'no -SoChecar o que falta fica na fila'
$SoChecar = $false

$pare = $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Pare' }, $true).Extent.Text
$iErro = $pare.IndexOf('##HANGAR-ERRO##'); $iMsg = $pare.IndexOf('Erro $mensagem')
Assert ($iErro -ge 0 -and $iErro -lt $iMsg) 'Pare imprime o codigo antes da mensagem'
$falha = $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Falha' }, $true).Extent.Text
Assert ($falha.Contains('Write-Host "##HANGAR-FALHA## ${rotulo}: $motivo"')) '##HANGAR-FALHA## com o texto de sempre (atualizar.py)'
Assert ($texto.Contains("Write-Host '##HANGAR-ERRO## sem-winget'")) 'sem winget sai com codigo'

# --- Portao do 1/8: extra com codigo segue ate o fim; dependencia essencial para ---
# Roda num processo filho: o portao e o fim chamam exit. Trechos reais do install.ps1, dubles no resto.
function Get-Def($name) { $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name }, $true).Extent.Text }
function Get-If($cond, $contains) {
    $ast.Find({ param($n) $n -is [Management.Automation.Language.IfStatementAst] -and $n.Clauses[0].Item1.Extent.Text -eq $cond -and $n.Extent.Text.Contains($contains) }, $true).Extent.Text
}
$gate1 = Get-If '$essenciais.Count -gt 0' 'faltam:'
$gateEnd = Get-If '$pendencias.Count -gt 0' 'NAO terminou'
$devModeIf = Get-If '$script:faltaDevMode' 'modo-desenvolvedor'
$rodaFailIf = Get-If '$script:faltaRodaPsmux' 'roda do mouse'
$iEss = $texto.IndexOf('$essenciais = @(')
$essLine = $texto.Substring($iEss, $texto.IndexOf("`n", $iEss) - $iEss).TrimEnd("`r")
Assert (-not (Get-Def 'Instale').Contains('Add-AppPending') -and -not (Get-Def 'Instale-ClaudeCode').Contains('Add-AppPending')) 'dependencia essencial do 1/8 nao ganha codigo de pendencia'
$defs = (@('Nota', 'Ok', 'Falta', 'Erro', 'Titulo', 'Mark-Step', 'Mark-Item', 'Add-AppPending', 'Send-TailscaleLink', 'Instale', 'Loga-Tailscale', 'Pergunte-Mesmo', 'Policy-Locked') | ForEach-Object { Get-Def $_ }) -join "`n"
$preamble = @'
$ErrorActionPreference = 'Stop'
$App = $true; $Sim = $false; $SoChecar = $false; $Update = $false; $script:Interativo = $false
$script:pendencias = @(); $script:pendingCodes = @{}; $script:falhasMarcadas = @(); $script:depsError = ''
$script:currentStep = ''; $script:finalState = 'falhou'
function Pausa-Log { }
function Pausa-Fim { }
function FakePowerShell { $global:LASTEXITCODE = 1 }
$PowerShellExe = 'FakePowerShell'; $raiz = 'C:\hangar-falso'; $script:psmuxSessoes = 2
$pyBackend = Join-Path $raiz 'backend\.venv\Scripts\python.exe'
'@
function Run-Gate($caso, $corpo, $depois) {
    $child = Join-Path $env:TEMP ("hangar-test-" + [guid]::NewGuid().ToString('N') + '.ps1')
    $out = "$child.txt"
    $script1 = $preamble + "`n" + $defs + "`n" + $caso + "`ntry {`nMark-Step 'preparar' 'fazendo'`n" + $corpo + "`n" + $essLine + "`n" + $gate1 + "`n" +
        $devModeIf + "`n" + $rodaFailIf + "`nWrite-Host 'PASSOU-PORTAO'`n" + $depois + "`n" + $gateEnd + "`n`$script:finalState = 'ok'`n} finally {`n" +
        "if (`$script:finalState -eq 'falhou' -and `$script:currentStep) { Write-Host `"##HANGAR-PASSO## `$(`$script:currentStep) falhou`" }`n" +
        "Write-Host `"##HANGAR-FIM## `$(`$script:finalState)`"`n}`n"
    [IO.File]::WriteAllText($child, $script1, (New-Object Text.UTF8Encoding $true))
    try {
        & cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File `"$child`" > `"$out`" 2>&1 < NUL"
        return @(Get-Content $out | Where-Object { $_.Trim() })
    } finally { Remove-Item $child, $out -ErrorAction SilentlyContinue }
}
$askIf = $ast.Find({ param($n) $n -is [Management.Automation.Language.IfStatementAst] -and $n.Clauses[0].Item1.Extent.Text -eq "`$script:psmuxConsole -eq 'perguntar'" }, $true).Extent.Text
$runIf = $ast.Find({ param($n) $n -is [Management.Automation.Language.IfStatementAst] -and $n.Clauses[0].Item1.Extent.Text -eq "`$script:psmuxConsole -eq 'pular'" }, $true).Extent.Text
$rodaCorpo = "`$script:psmuxConsole = 'perguntar'`n" + $askIf + "`n" + $runIf

# (1) roda do mouse
$l = Run-Gate '$ConsertarRoda = $false' $rodaCorpo
Assert ($l -contains '##HANGAR-PENDENCIA## roda-do-mouse roda do mouse (psmux)') '-App sem -ConsertarRoda: pendencia roda-do-mouse'
Assert (($l -contains 'PASSOU-PORTAO') -and $l[-1] -eq '##HANGAR-FIM## pendente') "-App sem -ConsertarRoda: nao para no 1/8 e termina pendente (ultima: $($l[-1]))"
$l = Run-Gate '$ConsertarRoda = $true' $rodaCorpo
Assert (-not @($l | Where-Object { $_ -like '##HANGAR-PENDENCIA## roda-do-mouse *' }).Count) '-App -ConsertarRoda: sem pendencia roda-do-mouse'

# (2) login da Tailscale que nao conclui no 1/8 (o relogio pula 10 min a cada leitura)
$tsCaso = @'
$script:relogio = [datetime]'2026-01-01'
function Get-Date { $script:relogio = $script:relogio.AddMinutes(10); return $script:relogio }
function Start-Job { param($ScriptBlock) return [pscustomobject]@{ State = 'Running' } }
function Receive-Job { param($Job) 'To authenticate, visit: https://login.tailscale.com/a/teste1' }
function Stop-Job { param($Job, $ErrorAction) }
function Remove-Job { param($Job, [switch]$Force, $ErrorAction) }
function Start-Sleep { param($Milliseconds) }
'@
$l = Run-Gate $tsCaso 'Loga-Tailscale'
Assert ($l -contains '##HANGAR-LINK## tailscale-login https://login.tailscale.com/a/teste1') 'login da Tailscale: LINK sai'
Assert ($l -contains '##HANGAR-PENDENCIA## tailscale-login login do Tailscale') 'login da Tailscale sem concluir: pendencia tailscale-login'
Assert (($l -contains 'PASSOU-PORTAO') -and $l[-1] -eq '##HANGAR-FIM## pendente') "login da Tailscale sem concluir: nao para no 1/8 (ultima: $($l[-1]))"

# (3) dependencia essencial faltando continua parando no 1/8
$depCaso = @'
function Tem($cmd) { return $false }
function Nativo { return 1 }
function Atualiza-Path { }
function Test-Internet { return $false }
'@
$l = Run-Gate $depCaso "[void](Instale 'psmux (multiplexador)' 'psmux' 'marlocarlo.psmux' 'sem ele nao existe sessao')"
Assert (-not ($l -contains 'PASSOU-PORTAO') -and ($l -contains '##HANGAR-ERRO## sem-internet')) 'dependencia essencial faltando: para no 1/8 com o codigo'
Assert ($l[-2] -eq '##HANGAR-PASSO## preparar falhou' -and $l[-1] -eq '##HANGAR-FIM## falhou') "dependencia essencial faltando: FIM falhou (ultima: $($l[-1]))"

# (4) Tailscale que nao instalou no 1/8 e extra: o portao deixa passar e a etapa fecha pendente
$ts18 = Get-If "`$script:querTailscale -and -not (Tem 'tailscale')" 'Loga-Tailscale'
$l = Run-Gate ($depCaso + "`n`$script:querTailscale = `$true`nfunction Loga-Tailscale { }`nfunction Nota { }") $ts18
Assert ($l -contains '##HANGAR-PASSO## tailscale pendente') 'Tailscale que nao instalou: etapa tailscale pendente'
Assert (($l -contains 'PASSOU-PORTAO') -and ($l -contains '##HANGAR-PENDENCIA## outro Tailscale')) 'Tailscale que nao instalou: nao para no 1/8 e vira pendencia outro'
Assert ($l[-1] -eq '##HANGAR-FIM## pendente') "Tailscale que nao instalou: FIM pendente (ultima: $($l[-1]))"

# --- Rodada de correcao 1: a tela diz o que aconteceu ---
# Login da Tailscale sem concluir no 1/8 e o 5d sem nome de no: a etapa fecha pendente.
$ts5d = Get-If "`$Tailscale -eq 'nao'" 'Publica-Tailscale'
$tsClose = Get-If '$null -ne $antesTs' "Mark-Step 'tailscale'"
$tsCaso5d = $tsCaso + @'

$Tailscale = 'sim'; $script:querTailscale = $true
function Tem($c) { return $true }
function Publica-Tailscale { Nota 'tailscale sem nome de no (nao logado?)' }
'@
$l = Run-Gate $tsCaso5d 'Loga-Tailscale' ("`$antesTs = `$null`n" + $ts5d + "`n" + $tsClose)
$passosTs = @($l | Where-Object { $_ -like '##HANGAR-PASSO## tailscale *' })
Assert ($passosTs.Count -and $passosTs[-1] -eq '##HANGAR-PASSO## tailscale pendente') "login da Tailscale sem concluir no 1/8: etapa tailscale pendente (veio: $($passosTs -join '|'))"

# 7b: o bash ausente deixa o item hangar-send pendente, mesmo sem somar pendencia.
$iSend = $texto.IndexOf('$sendOk = $true')
# O checkout do Windows (core.autocrlf) deixa o install.ps1 em CRLF.
$nlSend = if ($texto.Contains("`r`n")) { "`r`n" } else { "`n" }
$fimSend = "if (`$App) { Add-AppPending 'hangar-send' 'outro' }$nlSend}"
$send7b = $texto.Substring($iSend, $texto.IndexOf($fimSend) + $fimSend.Length - $iSend)
. ([scriptblock]::Create((Get-Def 'Titulo')))
function Tem($cmd) { return $false }
$App = $true; Limpa
$script:pendencias = @(); $script:pendingCodes = @{}
. ([scriptblock]::Create($send7b))
Assert ($script:saida -contains '##HANGAR-ITEM## hangar-send pendente sessoes conversam entre si') '7b sem bash: ITEM hangar-send pendente'
Assert ($script:pendencias -contains 'hangar-send' -and $script:pendingCodes['hangar-send'] -eq 'outro') '7b sem bash: -App termina pendente (pendencia outro)'

# Firewall nao liberado: mesma regra, so no -App.
Assert ($texto -match "(?s)Mark-Item 'firewall' 'pendente'[^\n]*\n\s*if \(\`$App\) \{ Add-AppPending 'firewall' 'outro' \}") 'firewall recusado: -App termina pendente (pendencia outro)'

# Tailscale ja instalada e logada: itens tailscale e tailscale-conta (como no Linux).
Assert ($texto -match "(?s)# Instalada e logada.*?Mark-Item 'tailscale' 'ok' 'Tailscale'\s*Mark-Item 'tailscale-conta' 'ok' 'conta Tailscale'\s*\`$proxy443 = Get-Proxy443") '5d: itens tailscale e tailscale-conta quando ja logada'

# Claude Code que nao instalou: sem rede o codigo e sem-internet.
. ([scriptblock]::Create((Get-Def 'Instale-ClaudeCode')))
function Nativo { return 1 }
$SoChecar = $false; $Update = $false
function Test-Internet { return $false }
$script:depsError = ''; $script:pendencias = @(); Limpa
[void](Instale-ClaudeCode 'agente escolhido')
Assert ($script:depsError -eq 'sem-internet') 'Claude Code sem rede: sem-internet'
function Test-Internet { return $true }
$script:depsError = ''; $script:pendencias = @(); Limpa
[void](Instale-ClaudeCode 'agente escolhido')
Assert ($script:depsError -eq 'agente-nao-instalou') 'Claude Code com rede: agente-nao-instalou'

# Politica travada pela TI (AllSigned por GPO): item vermelho sempre com a pendencia politica-travada.
$polIf = Get-If '$App' 'politica-scripts'
$soChecarIf = Get-If '$SoChecar' 'Nada faltando'
$polCaso = @'
function Get-ExecutionPolicy { param($Scope) if ($Scope -eq 'MachinePolicy') { return 'AllSigned' } return 'Undefined' }
'@
$l = Run-Gate $polCaso $polIf
Assert ($l -contains '##HANGAR-ITEM## politica-scripts falhou permissao de scripts (travada pela TI)') 'AllSigned por GPO: item politica-scripts falhou'
Assert (($l -contains '##HANGAR-PENDENCIA## politica-travada permissao de scripts') -and $l[-1] -eq '##HANGAR-FIM## pendente') "AllSigned por GPO: pendencia politica-travada e FIM pendente (ultima: $($l[-1]))"
$l = Run-Gate ($polCaso + "`n`$SoChecar = `$true") ($polIf + "`n" + $soChecarIf)
Assert (($l -contains '##HANGAR-PASSO## preparar pendente') -and $l[-1] -eq '##HANGAR-FIM## pendente') "-SoChecar com AllSigned por GPO: preparar e FIM pendente (ultima: $($l[-1]))"

# --- fim dos casos ---
if ($script:falhas) { [Console]::WriteLine("$($script:falhas) falha(s)"); exit 1 }
[Console]::WriteLine('tudo ok')
