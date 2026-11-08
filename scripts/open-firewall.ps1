# Libera no Firewall do Windows as portas que o hangar-server escuta em 0.0.0.0: 8765 (app) e 8766
# (convite pela rede local), nos perfis Private e Public. A regra e por porta porque o executavel muda
# de pasta a cada versao: o "Allow" do aviso vale so para o caminho daquela versao. So regra de
# liberacao cala o aviso; a de bloqueio nao.
# Arquivo so em ASCII: o PowerShell 5.1 le sem BOM como cp1252.
#
# Sem admin pede o UAC uma vez. Nunca falha: passo que falha derruba o Atualizar inteiro. UAC
# recusado, sem resposta ou pulado fica escrito na marca e na saida.
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File scripts/open-firewall.ps1 [-Ports 8765,8766] [-Check] [-NoPrompt]
# -Check so confere: sai com o numero de portas ainda sem regra.
# -NoPrompt nunca pede UAC: o da atualizacao vem de processo escondido, so pisca na barra de tarefas,
# e o Start-Process fica preso esperando a resposta ate o passo estourar o prazo.
param([int[]]$Ports = @(8765, 8766), [switch]$Check, [switch]$NoPrompt)
$ErrorActionPreference = 'Continue'
$marca = Join-Path $HOME '.hangar\firewall-ports'

function Missing {
    @($Ports | Where-Object {
        $ok = Get-NetFirewallRule -DisplayName "hangar $_" -ErrorAction SilentlyContinue | Where-Object {
            # Profile: Any = 0, Private = 2, Public = 4.
            $_.Enabled -eq 'True' -and $_.Action -eq 'Allow' -and ([int]$_.Profile -eq 0 -or ([int]$_.Profile -band 6) -eq 6)
        }
        -not $ok
    })
}

$faltam = Missing
if ($Check) { exit $faltam.Count }

if ($faltam.Count -gt 0) {
    # A regra antiga so sai depois que a nova existe: falha no meio nao deixa a porta sem regra.
    $cmd = "`$ErrorActionPreference = 'Stop'; " + (($faltam | ForEach-Object {
        "`$old = @(Get-NetFirewallRule -DisplayName 'hangar $_' -ErrorAction SilentlyContinue); " +
        "New-NetFirewallRule -DisplayName 'hangar $_' -Direction Inbound -Action Allow -Protocol TCP -LocalPort $_ -Profile Private,Public | Out-Null; " +
        "`$old | Remove-NetFirewallRule"
    }) -join '; ')
    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
    if ($admin) {
        try { & ([scriptblock]::Create($cmd)) } catch { Write-Host "firewall: $($_.Exception.Message)" }
    } elseif ($NoPrompt) {
        Write-Host "firewall: sem admin e sem UAC (-NoPrompt); porta(s) $($faltam -join ', ') ficam para o instalador"
    } else {
        Write-Host "firewall: pedindo o UAC para liberar a(s) porta(s) $($faltam -join ', ')"
        try {
            $p = Start-Process (Get-Process -Id $PID).Path -Verb RunAs -PassThru -WindowStyle Hidden `
                -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', $cmd
            if (-not $p.WaitForExit(120000)) { Write-Host 'firewall: o UAC ficou 2 min sem resposta' }
            elseif ($p.ExitCode -ne 0) { Write-Host "firewall: o comando elevado saiu com $($p.ExitCode)" }
        } catch { Write-Host "firewall: UAC recusado ou indisponivel: $($_.Exception.Message)" }
    }
    $faltam = Missing
}

$texto = if ($faltam.Count -eq 0) { "liberadas: $($Ports -join ', ')" } else {
    "sem regra: $($faltam -join ', ') - num PowerShell como admin: powershell -ExecutionPolicy Bypass -File scripts\open-firewall.ps1"
}
Write-Host "firewall: $texto"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $marca) | Out-Null
[IO.File]::WriteAllText($marca, $texto)
exit 0
