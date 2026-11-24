# Wrapper do `omp` pro PowerShell - equivalente do scripts/shell/omp.posix.sh, regra por regra.
#
# Mesmo papel do pi.ps1, com UMA diferenca: o omp nao tem `--session-id`. Quem escolhe a sessao e
# o CAMINHO do transcript (`--session <arquivo>`), montado igual ao servidor
# (crates/hangar-server/src/session_open/agents.rs), e CP_PI_SESSION leva o MESMO uuid do nome do
# arquivo.
#
#  - subcomando como PRIMEIRO argumento (lista de `omp --help`, secao COMMANDS) ou flag que
#    imprime e sai (-p/--print, --mode, --alias, --export, --help/-h, --version/-v) -> omp cru.
#  - flag que gerencia a propria sessao (--session, --resume/-r, --continue/-c, --no-session,
#    --session-dir) -> omp cru.
#  - perfil por --profile vira OMP_PROFILE (o omp honra; e de onde o servidor le o perfil).
#  - fora do psmux + terminal -> o lancador do Hangar abre a sessao e o terminal anexa nela.
#    Argumento que o pedido nao cobre ou sem hangar-server -> sessao no psmux com o nome da pasta.
#  - ja no psmux / entrada redirecionada -> omp cru com o --session e o CP_PI_SESSION.
#
# Escape explicito: `& (Get-Command omp -CommandType Application)`.
# Compativel com Windows PowerShell 5.1 (nada de operador ternario nem ??).

# Perfil gravado antes do hangar-open.ps1 existir carrega so os wrappers: o ajudante vem junto.
if (-not (Get-Command Open-HangarSession -CommandType Function -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot 'hangar-open.ps1')
}

# Perfil pedido na linha (`--profile x` / `--profile=x`) ou ja no ambiente. Vazio = sem perfil.
function Get-HangarOmpProfile([object[]]$lista) {
    for ($i = 0; $i -lt $lista.Count; $i++) {
        $a = [string]$lista[$i]
        if ($a -ceq '--profile') {
            if ($i + 1 -lt $lista.Count) { return [string]$lista[$i + 1] }
            return ''
        }
        if ($a.StartsWith('--profile=', [StringComparison]::Ordinal)) { return $a.Substring(10) }
    }
    return "$env:OMP_PROFILE"
}

# <raiz>\<slug do cwd>\<ts>_<id>.jsonl. O slug troca so o separador (e o ':' do drive) por '-',
# como o pi_cwd_slug do servidor; com perfil a raiz e a dele (o omp ignora PI_CODING_AGENT_DIR ali).
function Get-HangarOmpTarget([string]$id) {
    if ($env:PI_CODING_AGENT_DIR) { $raiz = [IO.Path]::Combine($env:PI_CODING_AGENT_DIR, 'sessions') }
    else { $raiz = [IO.Path]::Combine($HOME, '.omp', 'agent', 'sessions') }
    if ($env:OMP_PROFILE) { $raiz = [IO.Path]::Combine($HOME, '.omp', 'profiles', $env:OMP_PROFILE, 'agent', 'sessions') }
    $slug = '--' + (((Get-Location).ProviderPath -replace '^[/\\]', '') -replace '[/\\:]', '-') + '--'
    $quando = [DateTime]::UtcNow.ToString("yyyy-MM-dd'T'HH-mm-ss'-000Z'", [Globalization.CultureInfo]::InvariantCulture)
    return [IO.Path]::Combine($raiz, $slug, "$($quando)_$id.jsonl")
}

function omp {
    # -CommandType Application: o binario, nunca esta funcao (senao recursao infinita).
    $exe = (Get-Command omp -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
    if (-not $exe) { Write-Error 'omp nao encontrado no PATH'; return }
    $lista = @($args)
    # Entrada pelo pipeline (`Get-Content f | omp ...`) vai ao CLI cru, como o `[ -t 0 ]` do POSIX.
    $entrada = $null
    if ($MyInvocation.ExpectingInput) { $entrada = @($input) }

    # Subcomando so vale como PRIMEIRO argumento (`omp models` lista; `omp "models are slow"` e prompt).
    $sub = @('acp', 'agents', 'auth-broker', 'auth-gateway', 'bench', 'browser-relay', 'cleanse', 'commit',
             'completions', 'compress', 'config', 'dry-balance', 'gallery', 'gc', 'git', 'grep', 'grievances',
             'if-bench', 'images', 'install', 'join', 'models', 'plugin', 'ps', 'read', 'render', 'say', 'search',
             'setup', 'share', 'shell', 'ssh', 'stats', 'tiny-models', 'token', 'ttsr', 'update', 'usage', 'worktree')
    if ($lista.Count -gt 0 -and $sub -ccontains [string]$lista[0]) { Invoke-HangarNative $exe $lista $entrada; return }
    # `--mode rpc`/`json` e consumidor programatico por stdio: envolver no psmux o quebraria.
    if (Test-HangarArg $lista '^(-p|--print|--mode|--mode=.*|--alias|--alias=.*|--export|--export=.*|--help|-h|--version|-v)$') {
        Invoke-HangarNative $exe $lista $entrada; return
    }

    Sync-HangarSkills

    # Sobrepor `--session` a um `omp -c` abriria sessao FRESCA em vez de continuar.
    if (Test-HangarArg $lista '^(--session|--session=.*|--resume|--resume=.*|-r|--continue|-c|--no-session|--session-dir|--session-dir=.*)$') {
        Invoke-HangarNative $exe $lista $entrada; return
    }

    $perfil = Get-HangarOmpProfile $lista
    if ($perfil) { $env:OMP_PROFILE = $perfil }

    Clear-HangarStaleTmux

    if (-not $env:TMUX -and $null -eq $entrada -and (Test-HangarInteractive)) {
        Open-HangarSession 'omp' 'omp' $lista
        if ($LASTEXITCODE -ne 100) { return }
        $id = [guid]::NewGuid().ToString()
        $envs = @("CP_PI_SESSION=$id")
        if ($perfil) { $envs += "OMP_PROFILE=$perfil" }
        Open-HangarFolderSession $envs (@('omp', '--session', (Get-HangarOmpTarget $id)) + $lista)
        return
    }

    # Cru com o caminho: ja no psmux ou entrada redirecionada.
    $id = [guid]::NewGuid().ToString()
    $env:CP_PI_SESSION = $id
    Invoke-HangarNative $exe (@('--session', (Get-HangarOmpTarget $id)) + $lista) $entrada
}
