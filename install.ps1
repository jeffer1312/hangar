# claude-cockpit — instalacao no Windows, em um comando.
#
#   powershell -ExecutionPolicy Bypass -File install.ps1
#   powershell -ExecutionPolicy Bypass -File install.ps1 -Sim        # nao interativo
#   powershell -ExecutionPolicy Bypass -File install.ps1 -SoChecar   # so diz o que falta
#
# Espelha o install.sh do Linux. Diferencas que sao do sistema, nao de escolha:
#   - o multiplexador e o psmux (tmux nativo de Windows, ConPTY); nao existe tmux aqui;
#   - nao ha systemd: os servicos ficam como tarefa do usuario, ou voce roda na mao;
#   - os wrappers de `claude`/`codex` sao .sh e nao rodam aqui — sessao aberta no terminal
#     ainda NAO aparece no app. O app cria e dirige as proprias sessoes normalmente.
param([switch]$Sim, [switch]$SoChecar)

$ErrorActionPreference = 'Stop'
$raiz = Split-Path -Parent $MyInvocation.MyCommand.Path

function Titulo($m) { Write-Host "`n$m" -ForegroundColor Cyan }
function Ok($m)     { Write-Host "  ok  $m" -ForegroundColor Green }
function Falta($m)  { Write-Host "  --  $m" -ForegroundColor Yellow }
function Erro($m)   { Write-Host "  X   $m" -ForegroundColor Red }

function Atualiza-Path {
    # winget grava o PATH no registro, mas o PowerShell JA ABERTO segue com o antigo -> o
    # programa recem-instalado "nao existe". Reler os dois escopos evita mandar fechar o terminal
    # no meio da instalacao, que e onde a maioria dos roteiros de Windows perde o usuario.
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
                [Environment]::GetEnvironmentVariable('Path', 'User')
}

function Tem($cmd) { [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

$pendencias = @()

function Precisa($rotulo, $cmd, $id, $porque) {
    if (Tem $cmd) { Ok "$rotulo"; return $true }
    if ($SoChecar) { Falta "$rotulo — $porque (winget install --id $id)"; $script:pendencias += $rotulo; return $false }
    Titulo "instalando $rotulo ($id)"
    winget install --id $id --exact --silent `
        --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null
    Atualiza-Path
    if (Tem $cmd) { Ok "$rotulo instalado"; return $true }
    Erro "$rotulo nao instalou. Procure o id com:  winget search $cmd"
    $script:pendencias += $rotulo
    return $false
}

Atualiza-Path
if (-not (Tem 'winget')) {
    Erro 'winget nao encontrado. Instale o "App Installer" pela Microsoft Store.'
    exit 1
}

# ── 1. Dependencias ─────────────────────────────────────────────────────────
Titulo "1/5 Dependencias"
Precisa 'psmux (multiplexador)' 'psmux'  'marlocarlo.psmux'      'sem ele nao ha sessao'   | Out-Null
Precisa 'Claude Code'           'claude' 'Anthropic.ClaudeCode'  'e o que o app pilota'    | Out-Null
Precisa 'Python'                'py'     'Python.Python.3.13'    'o backend e Python'      | Out-Null
Precisa 'Node 20+'              'node'   'OpenJS.NodeJS.LTS'     'o frontend e Svelte'     | Out-Null
Precisa 'uv'                    'uv'     'astral-sh.uv'          'gerencia o venv do backend' | Out-Null

# O backend chama o multiplexador por `tmux`. O psmux publica esse alias; se um dia parar de
# publicar, isso vira um erro claro aqui em vez de "falha ao criar sessao" no app.
if ((Tem 'psmux') -and -not (Tem 'tmux')) {
    Erro 'psmux instalado mas sem o alias `tmux` — o backend chama por esse nome'
    $pendencias += 'alias tmux'
}

if ($SoChecar) {
    Titulo (($pendencias.Count -eq 0) ? "Nada faltando." : "Faltam: $($pendencias -join ', ')")
    exit ($pendencias.Count -eq 0 ? 0 : 1)
}
if ($pendencias.Count -gt 0) { Erro "faltam: $($pendencias -join ', ')"; exit 1 }

# ── 2. Backend ──────────────────────────────────────────────────────────────
Titulo "2/5 Backend (uv sync)"
Push-Location "$raiz\backend"
uv sync --quiet
Ok "dependencias instaladas (psutil entra aqui — no Windows nao ha /proc)"

# Token: mesmo criterio do install.sh. O backend recusa 'change-me' fora do loopback.
$envFile = "$raiz\backend\.env"
if ((Test-Path $envFile) -and (Select-String -Path $envFile -Pattern '^CP_AUTH_TOKEN=' -Quiet)) {
    Ok "backend/.env ja tem CP_AUTH_TOKEN (mantido)"
} else {
    $bytes = [byte[]]::new(24)
    [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
    $token = -join ($bytes | ForEach-Object { $_.ToString('x2') })
    Add-Content -Path $envFile -Value "CP_AUTH_TOKEN=$token"
    Ok "CP_AUTH_TOKEN gerado em backend\.env"
}
Pop-Location

# ── 3. Frontend ─────────────────────────────────────────────────────────────
Titulo "3/5 Frontend (npm ci + build)"
Push-Location "$raiz\frontend"
npm ci --silent
npm run build --silent
Pop-Location
Ok "frontend buildado em frontend\dist\"

# ── 4. Fumaca: o backend SOBE mesmo? ────────────────────────────────────────
# Ate agora tudo foi instalacao. Este passo e o que separa "instalou" de "funciona": ate pouco
# tempo atras o backend nem importava no Windows (um `import fcntl` no topo do projects.py) e o
# instalador teria dito sucesso do mesmo jeito.
Titulo "4/5 Checagem de fumaca"
Push-Location "$raiz\backend"
uv run python -c "from app import api, registry, procinfo, projects; import sys; sys.stdout.write('import ok')" | Out-Null
if ($LASTEXITCODE -ne 0) { Erro "o backend nao importa neste Windows"; Pop-Location; exit 1 }
Ok "o backend importa"
uv run python -c "from app import procinfo; assert not procinfo._TEM_PROC; import psutil; assert psutil.Process().cwd()" | Out-Null
if ($LASTEXITCODE -ne 0) { Erro "a leitura de processo via psutil falhou"; Pop-Location; exit 1 }
Ok "leitura de processo (psutil) funcionando"
Pop-Location

$sessao = "cp-fumaca-$PID"
tmux new-session -d -s $sessao -c $env:TEMP "cmd" 2>&1 | Out-Null
if ($LASTEXITCODE -eq 0) {
    tmux kill-session -t $sessao 2>&1 | Out-Null
    Ok "o multiplexador cria e mata sessao"
} else {
    Erro "o psmux nao criou uma sessao de teste — o app nao vai conseguir abrir sessao"
    exit 1
}

# ── 5. Como rodar ───────────────────────────────────────────────────────────
Titulo "5/5 Pronto"
@"
  - Subir o backend:
      cd backend ; `$env:CP_LAN_BIND_IP='auto' ; uv run python -m app.main
  - Subir o frontend (ou sirva frontend\dist\):
      cd frontend ; npm run dev
  - No celular: abra a URL do QR que o backend imprime e cole o token de backend\.env.
    Guia completo (Tailscale, instalar como PWA): docs\USAGE.md

  O que este Windows NAO tem, e e de propósito:
  - servico persistente: nao ha systemd. Rodando na mao, fechar o terminal derruba o backend.
  - `claude` aberto por voce no terminal NAO aparece no app: os wrappers que injetam o
    --session-id sao shell script. Sessao criada PELO app funciona normalmente.
  - deteccao de dev server por porta: funciona; no macOS e que ela degrada, nao aqui.
"@ | Write-Host
