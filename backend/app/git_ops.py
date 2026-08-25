import logging
import os
import re
import subprocess
import time
from pathlib import Path

_log = logging.getLogger("hangar.git_ops")

# Git pela sessao (cwd da sessao tmux). Tudo via argv list -> nunca string de shell (sem injecao).
# Acoes fixas: listar/trocar branch, status/pull/fetch/stash, e o write-path (commit/push) + navegacao
# de historico (commit_files/commit_file_diff). Comando git arbitrario NAO e exposto (o usuario digita
# direto na sessao claude se precisar); input do usuario (paths/sha/mensagem) sempre validado e passado
# como argv apos `--` -> sem superficie de RCE.
_TIMEOUT = 20


def _scrub(text: str) -> str:
    """Redige userinfo (user:token@) de URLs no texto -> um remote HTTPS com PAT embutido nao vaza a
    credencial no stderr do push (que vai pro git.error da UI / estado do celular)."""
    return re.sub(r"(://)[^/@\s]+@", r"\1***@", text)


def branch_of(cwd: str | None) -> str | None:
    """Branch atual do repo em cwd, lida direto de .git/HEAD (sem subprocess -> barato pra rodar
    por sessao na listagem). 'ref: refs/heads/<b>' -> <b>; detached/nao-repo/worktree -> None."""
    if not cwd:
        return None
    try:
        head = Path(cwd, ".git", "HEAD").read_text(encoding="utf-8", errors="replace").strip()
    except OSError:
        return None
    prefix = "ref: refs/heads/"
    return (head[len(prefix):] or None) if head.startswith(prefix) else None


class GitError(Exception):
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def _run(cwd: str, *args: str, timeout: float | None = None) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(
            ["git", "-C", cwd, *args],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
            timeout=timeout or _TIMEOUT,
            # LC_ALL=C: a saida do git aqui e LIDA POR CODIGO (ex: detectar "not a git
            # repository" pra esconder o menu de git em vez de reportar erro). Numa maquina com
            # catalogo NLS do git instalado, a mensagem sairia traduzida e a checagem quebraria
            # calada. Idioma da mensagem nao e superficie de usuario -- ela e dado.
            env={**os.environ, "LC_ALL": "C", "LANGUAGE": "C"},
        )
    except FileNotFoundError:
        raise GitError(500, "git nao encontrado")
    except subprocess.TimeoutExpired:
        raise GitError(504, "git timeout")
    except OSError as e:
        # ex: sem permissao de executar git -> erro limpo em vez de 500 com traceback.
        raise GitError(500, f"git falhou: {e}")


_AHEAD_RE = re.compile(r"ahead (\d+)")
_BEHIND_RE = re.compile(r"behind (\d+)")


def _parse_status_branch(out: str) -> dict | None:
    """Parseia `git status --porcelain=v1 --branch`. dirty = linhas de arquivo (todas menos o
    cabeçalho `## ...`; rename é UMA linha em porcelain=v1). ahead/behind só com upstream REAL:
    header sem `...` (detached/sem-upstream/sem-commits) ou com `[gone]` -> None."""
    lines = out.splitlines()
    if not lines or not lines[0].startswith("## "):
        return None
    header = lines[0]
    dirty = len(lines) - 1
    if "..." not in header or "[gone]" in header:
        return {"dirty": dirty, "ahead": None, "behind": None}
    a = _AHEAD_RE.search(header)
    b = _BEHIND_RE.search(header)
    return {"dirty": dirty,
            "ahead": int(a.group(1)) if a else 0,
            "behind": int(b.group(1)) if b else 0}


# Cache de módulo por cwd (TTL curto): git_summary roda por sessão na decoração da listagem, e o
# poll do painel é 2s. Ele é chamado de dentro de um asyncio.to_thread (ver registry), então
# requests concorrentes podem bater no dict de threads distintas -> pior caso um fork redundante,
# sem corrupção (benigno por idempotência, mesma classe do _status_cache de classe). Sem lock.
# cache: cwd -> (ts, result, ttl). O ttl e gravado por-entrada: SO o timeout (GitError) ganha o TTL
# longo (30s); resultado bom E falha transitoria (returncode!=0, ex. index.lock) ficam no TTL curto.
_summary_cache: dict[str, tuple[float, dict | None, float]] = {}
_SUMMARY_TTL = 3.0          # resultado bom OU falha transitoria: curto (volta rapido quando normaliza)
_SUMMARY_TTL_NEG = 30.0     # SO timeout de git (repo enorme/NFS): longo — nao re-forka o lento a cada poll
_SUMMARY_TIMEOUT = 2.0      # proprio (nao o _TIMEOUT global de 20s de push/log): um git status pendurado
                            # num NFS/repo enorme custava 20s de tick re-pago a cada poll -> watchdog
                            # de 25s dos clientes estourava em massa. 2s corta a cauda cedo.


def git_summary(cwd: str | None) -> dict | None:
    """{dirty, ahead, behind} do repo em cwd, ou None (não-repo/erro). Gate em `.git` existir pra
    não forkar `git status` numa pasta sem repo (ex.: sessão em ~). `.git` como ARQUIVO (worktree)
    passa o gate — git status funciona; só `.git` ausente cai fora. NUNCA levanta: _run estoura
    GitError no timeout (repo enorme/NFS) e isso não pode virar 500 no /api/sessions."""
    if not cwd or not os.path.exists(os.path.join(cwd, ".git")):
        return None
    now = time.monotonic()
    hit = _summary_cache.get(cwd)
    if hit and now - hit[0] < hit[2]:
        return hit[1]
    ttl = _SUMMARY_TTL
    try:
        p = _run(cwd, "status", "--porcelain=v1", "--branch", timeout=_SUMMARY_TIMEOUT)
    except GitError as e:
        # timeout / git ausente: sem badge, nunca 500. SO este caso ganha o TTL longo (30s) pra não
        # re-forkar o git lento a cada poll — a causa das rajadas que estouravam o watchdog.
        # Falha APARECE (padrao da casa): loga o warning — o cache negativo de 30s evita spam.
        _log.warning("git_summary timeout/erro em %s: %s (badge omitido, retry em %ss)",
                     cwd, e.detail, _SUMMARY_TTL_NEG)
        result = None
        ttl = _SUMMARY_TTL_NEG
    else:
        if p.returncode == 0:
            result = _parse_status_branch(p.stdout)
        else:
            # returncode != 0 com repo SAO (ex. index.lock transitorio durante um commit concorrente):
            # o badge some, mas TTL NORMAL (3s) -> volta rapido quando o lock sai, NAO fica 30s mudo.
            # Falha aparece: log.
            _log.warning("git_summary returncode=%s em %s (badge omitido, retry em %ss)",
                         p.returncode, cwd, _SUMMARY_TTL)
            result = None
    _summary_cache[cwd] = (now, result, ttl)
    return result


def _parse_numstat(out: str) -> dict:
    """Parseia `git diff --numstat HEAD`: uma linha '<added>\\t<deleted>\\t<path>' por arquivo.
    Binario vem como '-\\t-\\t<path>' -> conta 0 (o numero nao existe). Linha malformada: ignora."""
    added = removed = 0
    for line in out.splitlines():
        parts = line.split("\t", 2)
        if len(parts) < 3 or parts[0] == "-":
            continue
        try:
            added += int(parts[0])
            removed += int(parts[1])
        except ValueError:
            continue
    return {"added": added, "removed": removed}


# Cache proprio, mesmo contrato do _summary_cache acima (por cwd, TTL curto/longo por-entrada,
# sem lock): o diff roda por sessao na decoracao da listagem junto do git_summary e o poll e 2s.
_diffstat_cache: dict[str, tuple[float, dict | None, float]] = {}


def git_diffstat(cwd: str | None) -> dict | None:
    """{added, removed} do working tree vs HEAD (staged + unstaged somados) do repo em cwd, ou
    None (nao-repo/erro). Pro "+N -M" do card de sessao. Untracked NAO entra: `git diff HEAD` nao
    os ve, e contar linhas lendo cada arquivo novo seria I/O por arquivo a cada poll — o badge
    git_dirty ja sinaliza "ha coisa nao-commitada". Repo SEM commits nao tem HEAD pra comparar ->
    None tambem. Mesmo contrato do git_summary: gate em .git, cache por cwd, timeout proprio de
    2s, NUNCA levanta (nao pode virar 500 no /api/sessions)."""
    if not cwd or not os.path.exists(os.path.join(cwd, ".git")):
        return None
    now = time.monotonic()
    hit = _diffstat_cache.get(cwd)
    if hit and now - hit[0] < hit[2]:
        return hit[1]
    ttl = _SUMMARY_TTL
    try:
        p = _run(cwd, "diff", "--numstat", "HEAD", timeout=_SUMMARY_TIMEOUT)
    except GitError as e:
        # Mesma regra do git_summary: so o timeout ganha TTL longo (30s). Falha APARECE no log.
        _log.warning("git_diffstat timeout/erro em %s: %s (badge omitido, retry em %ss)",
                     cwd, e.detail, _SUMMARY_TTL_NEG)
        result = None
        ttl = _SUMMARY_TTL_NEG
    else:
        if p.returncode == 0:
            result = _parse_numstat(p.stdout)
        elif "ambiguous argument" in (p.stderr or "") or "unknown revision" in (p.stderr or ""):
            # Repo sem commits (HEAD nao resolve): caso ESPERADO, nao falha — sem warning pra nao
            # spam o log a cada poll. None (sem "+N -M"), TTL curto: o 1o commit liga o badge.
            result = None
        else:
            # Transiente de verdade (ex. index travado no meio de um commit concorrente): warning,
            # TTL curto -> volta em 3s.
            _log.warning("git_diffstat returncode=%s em %s (badge omitido, retry em %ss)",
                         p.returncode, cwd, _SUMMARY_TTL)
            result = None
    _diffstat_cache[cwd] = (now, result, ttl)
    return result


def list_branches(cwd: str) -> dict:
    """Branches locais + remotas (sem local correspondente) + a atual + se a working tree esta
    suja (pro front avisar antes do checkout: `git switch` carrega mudancas nao-conflitantes pra
    outra branch silenciosamente). As remotas vem pelo nome curto (sem o prefixo do remote): trocar
    pra uma delas faz o DWIM do `git switch` (cria a local rastreando origin/<nome>). Ficam
    defasadas ate um fetch."""
    # --sort=-committerdate: mais recentemente commitada primeiro (nao alfabetico).
    p = _run(cwd, "branch", "--sort=-committerdate", "--format=%(refname:short)")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git branch falhou").strip() or "git branch falhou")
    branches = [b.strip() for b in p.stdout.splitlines() if b.strip()]
    local = set(branches)

    # Remotas: -r lista refs/remotes/*. Nome curto = tira o primeiro segmento (o nome do remote).
    # Ignora o 'origin/HEAD' simbolico (refname:short = so 'origin', sem '/') e dedup pelo nome
    # curto contra as locais (uma remota que ja tem local nao precisa aparecer duas vezes).
    r = _run(cwd, "branch", "-r", "--sort=-committerdate", "--format=%(refname:short)")
    remotes: list[str] = []
    seen: set[str] = set()
    if r.returncode == 0:
        for full in r.stdout.splitlines():
            full = full.strip()
            if not full or "/" not in full:   # '' ou 'origin' (o HEAD simbolico) -> ignora
                continue
            short = full.split("/", 1)[1]
            if short == "HEAD" or short in local or short in seen:
                continue
            seen.add(short)
            remotes.append(short)

    cur = _run(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    current = cur.stdout.strip() if cur.returncode == 0 else None
    st = _run(cwd, "status", "--porcelain")
    dirty = st.returncode == 0 and bool(st.stdout.strip())
    return {"current": current, "branches": branches, "remotes": remotes, "dirty": dirty}


def switch_branch(cwd: str, branch: str) -> dict:
    """Troca pra uma branch EXISTENTE — local OU remota (nome curto -> DWIM cria a local
    rastreando o remote). Valida contra a lista real (rejeita injecao/typo/flag-like). Falha (tree
    suja, nome curto ambiguo entre remotes, etc) volta como 409 com o stderr do git."""
    info = list_branches(cwd)
    valid = set(info["branches"]) | set(info["remotes"])
    if branch not in valid:
        raise GitError(400, "branch inexistente")
    p = _run(cwd, "switch", branch)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "switch falhou").strip() or "switch falhou")
    return {"current": branch, "output": (p.stdout + p.stderr).strip()}


# Allowlist de acoes sem argumento -> argv fixo, zero entrada do usuario no comando.
_ACTIONS = {
    "status": ["status", "--short", "--branch"],
    "pull": ["pull", "--ff-only"],
    "fetch": ["fetch", "--all", "--prune"],
    # stash guarda TUDO (inclui untracked) -> deixa a tree limpa pra trocar de branch; pop reaplica.
    "stash": ["stash", "push", "--include-untracked"],
    "stash-pop": ["stash", "pop"],
    # log: ultimos 30 commits, uma linha cada (hash curto + msg + autor + data relativa).
    "log": ["log", "-n", "30", "--pretty=format:%h  %s  (%an, %ar)"],
    # Saida de emergencia de revert/cherry-pick que conflitaram (sequencer em andamento).
    "revert-abort": ["revert", "--abort"],
    "cherry-pick-abort": ["cherry-pick", "--abort"],
}


def git_action(cwd: str, action: str) -> dict:
    argv = _ACTIONS.get(action)
    if argv is None:
        raise GitError(400, "acao invalida")
    p = _run(cwd, *argv)
    return {"ok": p.returncode == 0, "output": (p.stdout + p.stderr).strip()}


# Log estruturado (view dedicada de commits): campos delimitados por bytes de controle. Superset —
# inclui parents (%P) e refs (%D) pra servir a lista, o detalhe-de-commit e o grafo (fase 2) sem
# trocar de formato depois. %x1f (unit sep) entre campos, %x1e (record sep) entre commits: nenhum dos
# dois aparece em texto de commit -> split trivial e a prova de espacos/quebras no assunto/autor.
_LOG_FMT = "%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%at%x1f%ar%x1f%s%x1f%b%x1e"


def git_log(cwd: str, n: int = 50, grep: str | None = None) -> list[dict]:
    """Ultimos n commits, estruturados. --topo-order (nao por data) pro grafo nao intercalar branches.
    grep filtra por texto da mensagem. Tres cuidados: (1) o texto vai GRUDADO na flag
    (`--grep=<txt>`), nunca argv separado — assim um texto que comeca com '-' e valor, nao flag;
    (2) `-F` porque --grep e REGEX por padrao: sem isso, buscar 'c++' ou '(fix)' devolve
    "Invalid regular expression" na cara do usuario, e um '.' casaria qualquer caractere calado;
    (3) `-i` pra busca no celular nao depender de maiuscula."""
    argv = ["log", "--topo-order", "-n", str(n), f"--pretty=format:{_LOG_FMT}"]
    if grep:
        argv += [f"--grep={grep}", "-F", "-i"]
    p = _run(cwd, *argv)
    if p.returncode != 0:
        # repo sem nenhum commit ainda: git log sai !=0 -> lista vazia em vez de erro.
        if "does not have any commits" in p.stderr or "bad default revision" in p.stderr:
            return []
        raise GitError(409, (p.stderr or "git log falhou").strip() or "git log falhou")
    out = []
    for rec in p.stdout.split("\x1e"):
        rec = rec.strip("\n")
        if not rec:
            continue
        # maxsplit=8: o corpo (%b) e texto livre e pode conter o proprio \x1f — sem o limite, um
        # commit com esse byte no corpo sairia truncado calado.
        f = rec.split("\x1f", 8)
        if len(f) < 9:
            continue
        full, short, parents, refs, author, ts, rel, subject = f[:8]
        body = f[8].strip("\n")
        out.append({
            "hash": full,
            "short": short,
            "parents": parents.split() if parents else [],
            "refs": refs.strip(),
            "author": author,
            "ts": int(ts) if ts.isdigit() else 0,
            "rel": rel,
            "subject": subject,
            "body": body,
        })
    return out


def assign_lanes(commits: list[dict]) -> list[dict]:
    """Atribui uma coluna (lane) a cada commit pro grafo, a partir da saida de git_log (topo-order,
    child antes de parent). Mantem 'lanes' ativas (cada slot = hash esperado naquela coluna). O 1o
    parent HERDA a coluna do commit (linha reta pra baixo); parents extras de um merge abrem colunas
    novas (aresta curva). Colunas que esperavam o mesmo commit convergem nele. Acrescenta a cada
    commit: 'col' (int), 'edges' (arestas descendo pros parents: [{'to_col': int, 'curved': bool}]) e
    'passthrough' (colunas de outras lanes que atravessam esta linha sem dot — o front desenha uma
    vertical cheia nelas). Muta e devolve a mesma lista. Historico linear -> todos col=0, passthrough=[]."""
    lanes: list[str | None] = []

    def first_free() -> int:
        for i, x in enumerate(lanes):
            if x is None:
                return i
        lanes.append(None)
        return len(lanes) - 1

    for c in commits:
        h = c["hash"]
        waiting = [i for i, x in enumerate(lanes) if x == h]
        if waiting:
            col = waiting[0]
            for extra in waiting[1:]:
                lanes[extra] = None          # convergem neste commit
        else:
            col = first_free()               # tip/head sem filho na vista atual
        parents = c.get("parents", [])
        edges: list[dict] = []
        if not parents:
            lanes[col] = None                # root: coluna fecha
        else:
            for idx, p in enumerate(parents):
                existing = next((i for i, x in enumerate(lanes) if x == p), None)
                if existing is not None:
                    # esse parent JA e esperado noutra coluna -> a aresta converge pra la
                    # (nao abre lane nova). Sem isto, uma branch mesclada depois da main avancar
                    # ficava com a aresta solta, apontando pra uma coluna vazia.
                    edges.append({"to_col": existing, "curved": existing != col})
                    if idx == 0 and existing != col:
                        lanes[col] = None    # 1o parent vive noutra coluna -> a do commit fecha
                elif idx == 0:
                    lanes[col] = p           # 1o parent segue reto na coluna do commit
                    edges.append({"to_col": col, "curved": False})
                else:
                    fc = first_free()        # parent de merge sem lane -> coluna nova
                    lanes[fc] = p
                    edges.append({"to_col": fc, "curved": True})
        # Lanes que ATRAVESSAM esta linha sem serem tocadas pelo commit (passam reto, sem dot). Sem
        # isto o grafo perde a linha vertical de qualquer branch que nao seja a do commit atual.
        touched = {e["to_col"] for e in edges} | {col}
        c["col"] = col
        c["edges"] = edges
        c["passthrough"] = [i for i, x in enumerate(lanes) if x is not None and i not in touched]
    return commits


def _desescapa(p: str) -> str:
    """Desfaz a citacao em C do porcelain. `-c core.quotePath=false` cobre o acento, mas NAO o
    espaco: `git status --porcelain` devolve `"com espaco.txt"` entre aspas de qualquer jeito."""
    if not (p.startswith('"') and p.endswith('"') and len(p) >= 2):
        return p
    return (p[1:-1].encode("utf-8").decode("unicode_escape")
            .encode("latin-1").decode("utf-8", "replace"))


def changed_files(cwd: str) -> list[dict]:
    """Arquivos com mudanca nao-commitada (git status --porcelain). Cada item: `path`, `code` (os 2
    chars XY do porcelain: ' M', 'M ', '??', 'A '...) e `staged`. So leitura."""
    p = _run(cwd, "-c", "core.quotePath=false", "status", "--porcelain")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git status falhou").strip() or "git status falhou")
    out = []
    for line in p.stdout.splitlines():
        if len(line) < 4:
            continue
        code, path = line[:2], line[3:]
        if " -> " in path:                      # rename/copy: "old -> new" -> usa o novo path
            path = path.split(" -> ", 1)[1]
        out.append({"path": _desescapa(path), "code": code, "staged": code[0] not in " ?"})
    return out


def _changed_map(cwd: str) -> dict:
    return {f["path"]: f for f in changed_files(cwd)}


def file_diff(cwd: str, path: str) -> dict:
    """Diff (staged + unstaged, vs HEAD) de UM arquivo. Valida o path contra a lista real de
    alterados (rejeita traversal/injecao/flag-like); `--` separa o path de flags no argv."""
    files = _changed_map(cwd)
    if path not in files:
        raise GitError(400, "arquivo nao esta na lista de alterados")
    if files[path]["code"] == "??":             # untracked: HEAD nao tem -> mostra tudo como adicao
        p = _run(cwd, "diff", "--no-index", "--", "/dev/null", path)
    else:
        p = _run(cwd, "diff", "HEAD", "--", path)
    # git diff sai 1 quando HA diferenca (normal) -> so 128+ e erro real (path invalido, etc)
    if p.returncode >= 128:
        raise GitError(409, (p.stderr or "git diff falhou").strip() or "git diff falhou")
    texto, cortou = _cap(p.stdout)
    return {"path": path, "diff": texto, "truncated": cortou}


def discard_file(cwd: str, path: str) -> dict:
    """DESTRUTIVO: descarta a mudanca nao-commitada de UM arquivo. Valida o path contra a lista real.
    Untracked -> remove o arquivo do disco (git clean); tracked -> git restore ao HEAD (staged+tree)."""
    files = _changed_map(cwd)
    if path not in files:
        raise GitError(400, "arquivo nao esta na lista de alterados")
    if files[path]["code"] == "??":
        p = _run(cwd, "clean", "-f", "--", path)
    else:
        p = _run(cwd, "restore", "--staged", "--worktree", "--source=HEAD", "--", path)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "descartar falhou").strip() or "descartar falhou")
    return {"ok": True, "path": path}


_SHA_RE = re.compile(r"^[0-9a-f]{7,40}$")


def commit_files(cwd: str, sha: str) -> list[dict]:
    """Arquivos alterados num commit especifico (git show --name-status). Valida o sha (hex 7-40)
    antes de passar pro git -> rejeita injecao/flag-like. `code` = a 1a letra do status (M/A/D/R/C).
    `-m --first-parent`: sem isso, um merge commit vem VAZIO (heuristica de combined-diff do git); com,
    lista o que o merge trouxe vs o 1o parent. Em commit normal/root o par de flags e no-op."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "show", "--name-status", "--format=", "-m", "--first-parent", sha)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git show falhou").strip() or "git show falhou")
    out = []
    for line in p.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        parts = line.split("\t")
        code = parts[0][:1]
        path = parts[-1]                 # rename/copy: "R100\told\tnew" -> usa o novo path
        out.append({"path": path, "code": code})
    return out


def commit_file_diff(cwd: str, sha: str, path: str) -> dict:
    """Diff de UM arquivo num commit. Valida o sha E o path contra a lista real de arquivos do commit
    (mesmo invariante de file_diff/commit/discard); o path ainda entra apos `--` (nunca como flag). Usa
    `git show <sha> -- <path>` (cobre o root commit, sem precisar de <sha>^)."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    if path not in {f["path"] for f in commit_files(cwd, sha)}:
        raise GitError(400, "arquivo nao esta nesse commit")
    # -m --first-parent: consistente com commit_files -> num merge, diff vs o 1o parent (senao vem vazio).
    p = _run(cwd, "show", "--format=", "-m", "--first-parent", sha, "--", path)
    if p.returncode >= 128:
        raise GitError(409, (p.stderr or "git show falhou").strip() or "git show falhou")
    return {"path": path, "diff": p.stdout}


# Referencias que valem como MAINLINE ao procurar onde a branch nasceu. `origin/HEAD` sozinho nao
# serve: ele aponta pra branch padrao do remoto (master, tipicamente), e num fluxo que trabalha em
# develop a base com master fica anos atras do que a branch mudou de fato — foi assim que um
# arquivo criado na develop apareceu INTEIRO como adicao (medido 20/08/2026: a arvore de arquivos
# dizia +24 e o diff aberto dizia +414, no mesmo arquivo).
_MAINLINES = ("origin/HEAD", "origin/develop", "origin/main", "origin/master")


def _base_da_branch(cwd: str) -> tuple[str | None, str | None]:
    """Onde esta branch nasceu, e o PORQUE quando nao da pra saber.

    O segundo elemento e um CODIGO (chave de i18n do front, prefixo `arq_`), nunca frase:
    o front traduz, mesmo desenho do envelope de erro (frontend/src/lib/errosApi.ts)."""
    cabeca = _run(cwd, "rev-parse", "--verify", "-q", "HEAD")
    if cabeca.returncode != 0:
        return None, "arq_motivo_sem_commit"
    atual = cabeca.stdout.strip()

    up = _run(cwd, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}")
    upstream = up.stdout.strip() if up.returncode == 0 else ""

    candidatas: list[str] = [r for r in (upstream,) if r] + list(_MAINLINES)
    melhor, em_cima_da_mainline = None, False
    for ref in candidatas:
        b = _run(cwd, "merge-base", "HEAD", ref)
        base = b.stdout.strip() if b.returncode == 0 else ""
        if not base:
            continue
        if base == atual:
            # HEAD e ancestral desta ref. Se a ref for uma MAINLINE, a branch nao tem nada
            # proprio: o que ela mostraria "desde que nasceu" seria a historia da mainline
            # inteira. O upstream que e copia da propria branch (branch pushada, nada novo desde
            # o push) nao conta aqui — ali a mainline ainda da a base certa.
            if ref in _MAINLINES or (upstream and ref == upstream and _e_mainline(cwd, ref)):
                em_cima_da_mainline = True
            continue
        # "Mais proxima" por ANCESTRALIDADE, nao por data: dois commits podem ter o mesmo
        # segundo (e num teste tem), e ai a data escolheria pela ordem da lista.
        if melhor is None or _run(cwd, "merge-base", "--is-ancestor", melhor, base).returncode == 0:
            melhor = base

    if em_cima_da_mainline:
        return None, "arq_motivo_sem_commit_proprio"
    if melhor:
        return melhor, None
    if upstream:
        return None, "arq_motivo_sem_commit_proprio"
    return None, "arq_motivo_sem_base_conhecida"


def _e_mainline(cwd: str, ref: str) -> bool:
    """A ref e uma linha principal, e nao a copia remota DESTA branch?

    Comparacao por nome curto: `origin/develop` rastreado por uma branch `tarefa-x` e mainline;
    `origin/tarefa-x` e a copia dela. Sem isso, uma branch pushada (upstream = copia, merge-base
    = HEAD) seria lida como "esta em cima da mainline" e perderia o diff de branch que tem."""
    nome = _run(cwd, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
    return not ref.endswith("/" + nome) if nome else True


def path_diff(cwd: str, path: str, escopo: str) -> dict:
    """Diff de UM arquivo. escopo="branch" soma desde onde a branch nasceu ate o disco agora."""
    if escopo not in ("branch", "nao_commitado"):
        raise GitError(400, "escopo invalido")
    if path.startswith("-"):
        raise GitError(400, "caminho invalido")
    # NUL quebra realpath() e subprocess com ValueError solto — recusar antes dos dois.
    if "\x00" in path:
        raise GitError(400, "caminho invalido")
    # So caminho RELATIVO (mesma regua do filetree._resolver).
    if os.path.isabs(path):
        raise GitError(400, "caminho absoluto nao aceito")

    # Contencao contra o CWD DA SESSAO, ANTES da do topo do repo: o topo pode ser o PAI do
    # cwd, e a sessao aberta em /repo/sub nao pode diff `../segredo.txt` nem ler o que esta
    # fora dela. O realpath resolve symlink antes de comparar (recusa `pastaelo/cofre.txt`
    # apontando pra fora) e o isfile recusa pathspec magica (':/', '*', '.') que o `--` nao
    # desliga. Trade-off declarado: arquivo APAGADO do disco nao tem diff de branch.
    cwd_real = os.path.realpath(cwd)
    alvo = os.path.realpath(os.path.join(cwd, path))
    if alvo != cwd_real and not alvo.startswith(cwd_real + os.sep):
        raise GitError(400, "caminho fora do cwd da sessao")

    topo = _run(cwd, "rev-parse", "--show-toplevel")
    if topo.returncode != 0:
        raise GitError(409, "nao e um repositorio git")
    raiz = os.path.realpath(topo.stdout.strip())
    if alvo != raiz and not alvo.startswith(raiz + os.sep):
        raise GitError(400, "caminho fora do repositorio")
    if not os.path.isfile(alvo):
        raise GitError(404, "arquivo nao encontrado")

    # O ramo --no-index le o arquivo do DISCO, e `.git` esta dentro da raiz: sem isto,
    # `path_diff(cwd, ".git/config")` devolve o remote com o token embutido, o mesmo que o
    # _scrub existe pra impedir. Comparacao por COMPONENTE: `.gitignore` continua passando.
    if ".git" in os.path.relpath(alvo, raiz).split(os.sep):
        raise GitError(403, "area interna do git")

    usado, base, motivo = escopo, None, None
    if escopo == "branch":
        base, motivo = _base_da_branch(cwd)
        if base is None:
            usado = "nao_commitado"

    # Arquivo ainda nao rastreado nao aparece em `git diff` — e e o caso MAIS comum aqui, porque
    # a sessao acabou de criar o arquivo. Mesmo tratamento que file_diff ja da (git_ops.py:338).
    # `--literal-pathspecs` vai como opcao GLOBAL (antes do subcomando): `git diff --literal-`
    # e usage erro nesta versao, e a config core.literalPathspecs nao existe. Sem ele, um
    # arquivo real chamado `*` fazia o `--` nao desligar o pathspec e o diff vinha com TODOS
    # os arquivos modificados (medido no parecer 35a69cd).
    rastreado = _run(cwd, "--literal-pathspecs", "ls-files", "--error-unmatch", "--", path).returncode == 0
    if not rastreado:
        # Arquivo novo: a "versao original" e o vazio, e dizer isso deixa o visor desenhar o
        # arquivo inteiro como adicao sem precisar interpretar o unified diff.
        # --no-index NAO aceita --literal-pathspecs, e tambem nao precisa: os dois paths
        # posicionais sao tratados como literais pelo proprio git (medido).
        p = _run(cwd, "-c", "core.quotePath=false", "diff", "--no-index", "--", "/dev/null", path)
        if p.returncode >= 128 or (p.returncode != 0 and not p.stdout):
            raise GitError(409, (p.stderr or "git diff falhou").strip() or "git diff falhou")
        texto, cortou = _cap(p.stdout)      # --no-index sai com 1 quando ha diferenca: normal
        return {"path": path, "diff": texto, "truncated": cortou, "original": "",
                "escopo_pedido": escopo, "escopo_usado": usado, "base": base, "motivo": motivo}

    args = ["-c", "core.quotePath=false", "--literal-pathspecs", "diff"]
    if usado == "branch":
        args.append(base)
    elif _run(cwd, "rev-parse", "--verify", "-q", "HEAD").returncode == 0:
        args.append("HEAD")                                # sem commit nenhum, diff sem ref
    args += ["--", path]

    p = _run(cwd, *args)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git diff falhou").strip() or "git diff falhou")
    texto, cortou = _cap(p.stdout)
    return {"path": path, "diff": texto, "truncated": cortou,
            "original": _versao_na_base(cwd, path, base if usado == "branch" else "HEAD"),
            "escopo_pedido": escopo, "escopo_usado": usado, "base": base, "motivo": motivo}


def _versao_na_base(cwd: str, path: str, rev: str | None) -> str | None:
    """Conteudo do arquivo NAQUELA revisao — o lado esquerdo do diff.

    O visor desenha o diff dentro do editor (unifiedMergeView), e pra isso ele precisa do texto
    original, nao do unified diff: interpretar o `@@` no cliente pra reconstruir o original seria
    reimplementar o git em JavaScript.

    `rev:./path` e relativo ao CWD (medido) — sem o `./`, o git resolve a partir da RAIZ do repo e
    uma sessao aberta numa subpasta pediria o arquivo errado. `None` quando nao ha versao la
    (arquivo criado depois da base): quem chama trata como "tudo novo".
    """
    if not rev:
        return None
    p = _run(cwd, "show", f"{rev}:./{path}")
    if p.returncode != 0:
        return None
    if len(p.stdout) > _DIFF_MAX:
        return None            # arquivo enorme: o visor cai no diff de texto, sem travar a tela
    return p.stdout


# Teto do diff do commit inteiro. O diff POR ARQUIVO e seguro por construcao; o do commit inteiro
# nao: um commit que toca 500 arquivos vira megabytes que atravessam a rede e ainda passam pelo
# Shiki num bloco so, travando o celular. 200KB ja mostra qualquer commit humano por completo.
_DIFF_MAX = 200_000


def _cap(diff: str) -> tuple[str, bool]:
    if len(diff) <= _DIFF_MAX:
        return diff, False
    return diff[:_DIFF_MAX], True


def commit_diff(cwd: str, sha: str) -> dict:
    """Unified diff do commit INTEIRO (todos os arquivos) — a "Show changes as unified diff" do
    Tortoise. Mesmas flags -m --first-parent de commit_files (merge = diff vs o 1o parent)."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "show", "--format=", "-m", "--first-parent", sha)
    if p.returncode >= 128:
        raise GitError(409, (p.stderr or "git show falhou").strip() or "git show falhou")
    diff, truncated = _cap(p.stdout)
    return {"sha": sha, "diff": diff, "truncated": truncated}


def revert_commit(cwd: str, sha: str) -> dict:
    """git revert --no-edit <sha>: cria um NOVO commit desfazendo <sha>. Conflito -> returncode!=0:
    o repo fica em revert-in-progress; o stderr vai pro usuário e a saída é a ação 'revert-abort'."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "revert", "--no-edit", sha)
    if p.returncode != 0:
        # (stdout + stderr), nao "stderr ou stdout": num conflito o git lista os arquivos
        # conflitantes no STDOUT, e so pegar um dos dois some com a informacao que o usuario precisa
        # pra decidir entre abortar e resolver. Mesma juncao ja usada no `output` de sucesso.
        raise GitError(409, (p.stdout + p.stderr).strip() or "revert falhou")
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


def cherry_pick(cwd: str, sha: str) -> dict:
    """git cherry-pick <sha>: reaplica o commit em cima do HEAD. Mesmo contrato de erro do revert
    (abort via ação 'cherry-pick-abort')."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "cherry-pick", sha)
    if p.returncode != 0:
        raise GitError(409, (p.stdout + p.stderr).strip() or "cherry-pick falhou")
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


_RESET_MODES = {"soft", "mixed", "hard"}


def reset_to(cwd: str, sha: str, mode: str) -> dict:
    """git reset --<mode> <sha>. mode vem de ENUM (nunca do usuário como string livre). hard é
    destrutivo — a UI exige confirm em 2 passos; aqui a única guarda é o enum + sha validado."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    if mode not in _RESET_MODES:
        raise GitError(400, "modo invalido")
    p = _run(cwd, "reset", f"--{mode}", sha)
    if p.returncode != 0:
        # stdout JUNTO do stderr: um reset recusado lista no stdout os arquivos que seriam
        # sobrescritos — e e essa lista que o usuario precisa pra decidir. Mesma juncao do
        # revert_commit/cherry_pick.
        raise GitError(409, (p.stdout + p.stderr).strip() or "reset falhou")
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


def create_branch_at(cwd: str, name: str, sha: str | None = None, switch_after: bool = False) -> dict:
    """Cria branch em <sha> (ou HEAD). NÃO troca por padrão (Tortoise: 'Create branch from revision'
    não muda a working tree); switch_after=True cria E troca num UNICO comando (`switch -c`), nao
    branch+switch separados: dois comandos deixavam uma janela de falha parcial -- se o switch
    recusasse (sha != HEAD + working tree suja), a branch ja tinha sido criada e um retry dava
    "branch ja existe" em vez de repetir a operacao do zero."""
    _validate_new_ref(cwd, "heads", name)
    if sha is not None and not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    if switch_after:
        argv = ["switch", "-c", name] + ([sha] if sha else [])
    else:
        argv = ["branch", name] + ([sha] if sha else [])
    p = _run(cwd, *argv)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "criar branch falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


def create_tag(cwd: str, name: str, sha: str | None = None, message: str | None = None) -> dict:
    """Tag anotada (com message) ou leve. Nome validado igual branch (refs/tags/)."""
    _validate_new_ref(cwd, "tags", name)
    if sha is not None and not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    argv = ["tag"]
    if message and message.strip():
        argv += ["-a", "-m", message]
    argv += [name] + ([sha] if sha else [])
    p = _run(cwd, *argv)
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "criar tag falhou").strip())
    return {"ok": True, "output": (p.stdout + p.stderr).strip()}


def diff_vs_worktree(cwd: str, sha: str) -> dict:
    """Unified diff do commit ate o DISCO agora — o "Compare with working tree" do Tortoise.
    `git diff <sha>` = arvore de trabalho vs aquele commit (inclui o que nao esta staged)."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "diff", sha)
    if p.returncode >= 128:
        raise GitError(409, (p.stderr or "git diff falhou").strip() or "git diff falhou")
    diff, truncated = _cap(p.stdout)      # mesmo teto do commit_diff
    return {"sha": sha, "diff": diff, "truncated": truncated}


def sequencer_state(cwd: str) -> str | None:
    """Detecta um revert/cherry-pick em andamento (sequencer que conflitou e aguarda --abort/
    --continue). `--git-path` em vez de montar `cwd/.git/<marker>` na mao: `.git` pode ser um
    ARQUIVO (worktree), e o caminho real do marcador mora noutro lugar nesse caso. O caminho que o
    git devolve e RELATIVO a `cwd` (medido: `.git/CHERRY_PICK_HEAD`, nao absoluto) -> precisa do join."""
    for marker, label in (("CHERRY_PICK_HEAD", "cherry-pick"), ("REVERT_HEAD", "revert")):
        p = _run(cwd, "rev-parse", "--git-path", marker)
        if p.returncode == 0 and os.path.exists(os.path.join(cwd, p.stdout.strip())):
            return label
    return None


def branches_containing(cwd: str, sha: str) -> dict:
    """Branches locais e remotas que contem o commit — o "Shows branches this commit is on".
    Usa o refname COMPLETO, nao o :short: um branch local chamado 'origin' e o ref simbolico
    origin/HEAD saem com o MESMO texto no :short (medido), e nao havia como distinguir os dois —
    com refname a classificacao vem do prefixo (refs/heads/ vs refs/remotes/), sem ambiguidade."""
    if not _SHA_RE.match(sha):
        raise GitError(400, "sha invalido")
    p = _run(cwd, "branch", "-a", "--contains", sha, "--format=%(refname)")
    if p.returncode != 0:
        raise GitError(409, (p.stderr or "git branch --contains falhou").strip())
    local, remote = [], []
    for line in p.stdout.splitlines():
        ref = line.strip()
        if ref.startswith("refs/heads/"):
            local.append(ref[len("refs/heads/"):])
        elif ref.startswith("refs/remotes/"):
            short = ref[len("refs/remotes/"):]
            if short.endswith("/HEAD"):      # refs/remotes/origin/HEAD: ref simbolico, nao branch
                continue
            remote.append(short)
    return {"local": local, "remote": remote}


_BRANCH_REF_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$")
_REF_KIND_LABEL = {"heads": "branch", "tags": "tag"}


def _validate_new_ref(cwd: str, kind: str, name: str) -> None:
    """kind = 'heads' | 'tags'. Rejeita nome invalido pro git (check-ref-format) ou ja existente.
    O regex previo bara flag-like ('-D', '../') antes do subprocesso; o check-ref-format pega o resto
    (espaco, '..', '~', '^', ':', barra final...)."""
    label = _REF_KIND_LABEL[kind]
    if not _BRANCH_REF_RE.match(name) or ".." in name:
        raise GitError(400, f"nome de {label} invalido")
    p = _run(cwd, "check-ref-format", f"refs/{kind}/{name}")
    if p.returncode != 0:
        raise GitError(400, f"nome de {label} invalido")
    lst = _run(cwd, "branch" if kind == "heads" else "tag", "--format=%(refname:short)")
    if name in {l.strip() for l in lst.stdout.splitlines()}:
        raise GitError(400, f"{label} ja existe: {name}")


def last_commit_message(cwd: str) -> dict:
    """Mensagem completa (%B) do HEAD — pra pre-preencher o amend. Repo sem commit -> 409."""
    p = _run(cwd, "log", "-1", "--pretty=%B")
    if p.returncode != 0:
        raise GitError(409, "sem commits pra amend")
    return {"message": p.stdout.rstrip("\n")}


def commit(cwd: str, message: str, paths: list[str], amend: bool = False,
           new_branch: str | None = None) -> dict:
    """Commita SO os paths marcados (checkbox estilo Tortoise). Valida cada path contra a lista real
    de alterados (anti-traversal/flag-like); mensagem nao pode ser vazia. `-m` recebe a mensagem como
    argv (nunca shell) -> sem injecao.
    amend=True permite paths=[] (so reword) e dobra os paths marcados no commit anterior. Sempre
    `--amend --only`: sem isso um reword puro (`--amend -m`) dobraria mudancas staged por fora do app
    (verificado: --only sem paths = reword, index intocado; com paths = dobra so eles).
    new_branch cria a branch (switch -c) ANTES do commit — se o commit falhar, o usuario fica na
    branch nova com as mudancas intactas (o erro do git e reportado, falha aparece)."""
    if not message.strip():
        raise GitError(400, "mensagem vazia")
    if not paths and not amend:
        raise GitError(400, "nenhum arquivo selecionado")
    # TODAS as validacoes (sem efeito colateral) antes do switch/add:
    valid = {f["path"] for f in changed_files(cwd)}
    for p in paths:
        if p not in valid:
            raise GitError(400, f"arquivo nao esta na lista de alterados: {p}")
    if amend:
        last_commit_message(cwd)   # falha 409 se nao ha HEAD pra amend
    if new_branch is not None:
        _validate_new_ref(cwd, "heads", new_branch)
    # Efeitos colaterais:
    if new_branch is not None:
        s = _run(cwd, "switch", "-c", new_branch)
        if s.returncode != 0:
            raise GitError(409, (s.stderr or "criar branch falhou").strip())
    # Renomeados: o git colapsa "R old -> new" e changed_files so expoe `new`. Committar so `new`
    # transformaria o rename num "add" e deixaria a delecao de `old` staged e orfã. Detecta o par pelo
    # status e inclui `old` no pathspec do commit pra manter o rename atômico. (`old` vem do git, nao do
    # usuario -> nao precisa da validacao anti-traversal que os `paths` ja passaram.)
    renames: dict[str, str] = {}
    for line in _run(cwd, "-c", "core.quotePath=false", "status", "--porcelain").stdout.splitlines():
        if line[:1] == "R" and " -> " in line[3:]:
            old, new = line[3:].split(" -> ", 1)
            renames[_desescapa(new)] = _desescapa(old)
    extra = [renames[p] for p in paths if p in renames]
    # `add` primeiro: --only sozinho falha em arquivo untracked ("pathspec did not match").
    # `commit --only -- <paths>` grava SO esses paths, mesmo que outros estejam staged no indice.
    if paths:
        _run(cwd, "add", "--", *paths)
        argv = ["commit", "--only", "-m", message, "--", *paths, *extra]
        if amend:
            argv.insert(1, "--amend")
    else:
        argv = ["commit", "--amend", "--only", "-m", message]   # reword puro (amend garantido acima)
    r = _run(cwd, *argv)
    if r.returncode != 0:
        raise GitError(409, (r.stderr or r.stdout or "commit falhou").strip() or "commit falhou")
    return {"ok": True, "output": (r.stdout + r.stderr).strip()}


def push(cwd: str) -> dict:
    """Push da branch atual. Se ja tem upstream, `git push`; se nao, `git push -u origin <branch>`
    (1o push cria a branch no servidor e vincula). NUNCA --force. Sem 'origin' -> erro claro (409)."""
    br = _run(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    branch = br.stdout.strip()
    if not branch or branch == "HEAD":
        raise GitError(409, "sem branch atual (detached HEAD)")
    up = _run(cwd, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}")
    if up.returncode == 0 and up.stdout.strip():
        r = _run(cwd, "push")
    else:
        # sem upstream: exige o remote 'origin'
        rem = _run(cwd, "remote")
        if "origin" not in rem.stdout.split():
            raise GitError(409, "branch sem upstream e sem remote 'origin' — configure um remote antes")
        r = _run(cwd, "push", "-u", "origin", branch)
    if r.returncode != 0:
        raise GitError(409, _scrub((r.stderr or r.stdout or "push falhou").strip()) or "push falhou")
    return {"ok": True, "output": _scrub((r.stdout + r.stderr).strip())}


if __name__ == "__main__":
    # Self-check: repo temp, cria branch, valida switch + rejeicao + allowlist. Sem framework.
    import tempfile

    with tempfile.TemporaryDirectory() as d:
        _run(d, "init", "-q", "-b", "main")
        _run(d, "config", "user.email", "t@t")
        _run(d, "config", "user.name", "t")
        _run(d, "commit", "-q", "--allow-empty", "-m", "init")
        _run(d, "branch", "feature")

        info = list_branches(d)
        assert info["current"] == "main", info
        assert set(info["branches"]) == {"main", "feature"}, info

        assert switch_branch(d, "feature")["current"] == "feature"
        assert list_branches(d)["current"] == "feature"

        for bad in ["nope", "main; rm -rf /", "--orphan x"]:
            try:
                switch_branch(d, bad)
                raise AssertionError(f"deveria rejeitar: {bad!r}")
            except GitError as e:
                assert e.status == 400, e.status

        assert git_action(d, "status")["ok"] is True
        assert sequencer_state(d) is None, "repo limpo nao deveria ter sequencer"
        try:
            git_action(d, "rm -rf")
            raise AssertionError("acao invalida deveria falhar")
        except GitError as e:
            assert e.status == 400

        # Remotas: um segundo repo vira 'origin'; fetch traz as refs. Valida dedup + DWIM + fetch.
        with tempfile.TemporaryDirectory() as rd:
            _run(rd, "init", "-q", "-b", "main")
            _run(rd, "config", "user.email", "t@t")
            _run(rd, "config", "user.name", "t")
            _run(rd, "commit", "-q", "--allow-empty", "-m", "r")
            _run(rd, "branch", "only-remote")
            _run(d, "remote", "add", "origin", rd)

            assert git_action(d, "fetch")["ok"] is True
            info = list_branches(d)
            assert "only-remote" in info["remotes"], info      # remota sem local -> aparece
            assert "main" not in info["remotes"], info          # ja tem local -> dedup
            assert "only-remote" not in info["branches"], info  # ainda nao e local
            # switch DWIM: cria a local rastreando origin/only-remote
            assert switch_branch(d, "only-remote")["current"] == "only-remote"
            assert "only-remote" in list_branches(d)["branches"], "DWIM devia criar local"

            # push: com upstream configurado, sobe pro origin. rd precisa aceitar push (bare-like):
            _run(rd, "config", "receive.denyCurrentBranch", "ignore")
            _run(d, "switch", "-q", "-c", "pushme")
            _run(d, "commit", "-q", "--allow-empty", "-m", "p1")
            pr = push(d)               # sem upstream -> push -u origin pushme
            assert pr["ok"], pr
            assert _run(rd, "rev-parse", "pushme").returncode == 0, "branch nao chegou no remote"

        # git_log estruturado: parseia os commits do repo temp e valida os campos.
        commits = git_log(d, n=10)
        assert commits and all(c["hash"] and c["subject"] for c in commits), commits
        assert isinstance(commits[0]["parents"], list), commits[0]
        assert commits[0]["ts"] > 0, commits[0]

        # commit_files: os arquivos alterados de UM commit (name-status).
        _run(d, "commit", "-q", "--allow-empty", "-m", "empty2")
        head = _run(d, "rev-parse", "HEAD").stdout.strip()
        cf = commit_files(d, head)
        assert isinstance(cf, list), cf

        # commit com arquivo real -> exercita o parsing (code/path), nao so o tipo.
        import pathlib
        (pathlib.Path(d) / "cf.txt").write_text("x\n")
        _run(d, "add", "cf.txt")
        _run(d, "commit", "-q", "-m", "add cf")
        real = commit_files(d, _run(d, "rev-parse", "HEAD").stdout.strip())
        assert any(f["path"] == "cf.txt" and f["code"] == "A" for f in real), real

        for bad in ["nope; rm -rf /", "--all", "zzz"]:
            try:
                commit_files(d, bad)
                raise AssertionError(f"deveria rejeitar sha invalido: {bad!r}")
            except GitError as e:
                assert e.status == 400, e.status

        # commit_file_diff: diff de um arquivo dentro de um commit. Cria um commit com conteudo real.
        import pathlib
        (pathlib.Path(d) / "f.txt").write_text("linha1\n")
        _run(d, "add", "f.txt")
        _run(d, "commit", "-q", "-m", "add f")
        sha = _run(d, "rev-parse", "HEAD").stdout.strip()
        fd = commit_file_diff(d, sha, "f.txt")
        assert "linha1" in fd["diff"], fd
        try:
            commit_file_diff(d, "--all", "f.txt")
            raise AssertionError("deveria rejeitar sha invalido")
        except GitError as e:
            assert e.status == 400, e.status
        # path fora do commit -> 400 (mesmo invariante de file_diff/commit); e _scrub redige credencial.
        try:
            commit_file_diff(d, sha, "nao/existe.txt")
            raise AssertionError("deveria rejeitar path fora do commit")
        except GitError as e:
            assert e.status == 400, e.status
        assert "ghp_x" not in _scrub("https://u:ghp_x@h/r.git"), "scrub deve redigir userinfo"

        # assign_lanes: caso realista — feat baseada num commit antigo, main avanca 2x, depois merge.
        # Valida col/edges do merge E o passthrough (a lane do feat tem que atravessar as linhas da main).
        with tempfile.TemporaryDirectory() as gd:
            _run(gd, "init", "-q", "-b", "main")
            _run(gd, "config", "user.email", "t@t")
            _run(gd, "config", "user.name", "t")
            _run(gd, "commit", "-q", "--allow-empty", "-m", "base")
            _run(gd, "switch", "-q", "-c", "feat")
            _run(gd, "commit", "-q", "--allow-empty", "-m", "feat-1")
            _run(gd, "switch", "-q", "main")
            _run(gd, "commit", "-q", "--allow-empty", "-m", "main-1")
            _run(gd, "commit", "-q", "--allow-empty", "-m", "main-2")
            _run(gd, "merge", "-q", "--no-ff", "feat", "-m", "merge feat")
            g = assign_lanes(git_log(gd))
            # Asserts invariantes a ordem do topo-order (as colunas exatas variam, a estrutura nao):
            assert all("col" in c and "passthrough" in c for c in g), g
            merges = [c for c in g if len(c["parents"]) >= 2]
            assert len(merges) == 1 and len(merges[0]["edges"]) == 2, merges         # merge = 2 arestas
            assert any(e["curved"] for e in merges[0]["edges"]), merges[0]            # uma delas curva
            assert max(c["col"] for c in g) >= 1, [c["col"] for c in g]               # usa >1 coluna
            assert any(c["passthrough"] for c in g), [c["passthrough"] for c in g]    # lane atravessa
            # ha uma convergencia: um commit NAO-merge com aresta curva (branch voltando pro tronco)
            assert any(any(e["curved"] for e in c["edges"]) for c in g if len(c["parents"]) == 1), g

        # commit: grava so os paths marcados; valida path contra a lista real; mensagem vazia falha.
        with tempfile.TemporaryDirectory() as cd:
            _run(cd, "init", "-q", "-b", "main")
            _run(cd, "config", "user.email", "t@t")
            _run(cd, "config", "user.name", "t")
            (pathlib.Path(cd) / "a.txt").write_text("A\n")
            (pathlib.Path(cd) / "b.txt").write_text("B\n")
            r = commit(cd, "so o a", ["a.txt"])
            assert r["ok"], r
            # a.txt commitado, b.txt ainda untracked
            st = _run(cd, "status", "--porcelain").stdout
            assert "a.txt" not in st and "b.txt" in st, st

            # arquivo pré-staged NAO pode vazar pro commit de outro path.
            (pathlib.Path(cd) / "c.txt").write_text("C\n")
            _run(cd, "add", "b.txt")            # b fica staged, fora do commit pedido
            commit(cd, "so o c", ["c.txt"])
            names = _run(cd, "show", "--name-only", "--format=", "HEAD").stdout.split()
            assert "c.txt" in names and "b.txt" not in names, names

            for bad in [(["a.txt"], ""), (["../x"], "m"), (["--flag"], "m")]:
                try:
                    commit(cd, bad[1], bad[0])
                    raise AssertionError(f"deveria falhar: {bad!r}")
                except GitError:
                    pass
        # commit_files/commit_file_diff num MERGE commit: sem -m --first-parent viriam vazios.
        with tempfile.TemporaryDirectory() as md:
            _run(md, "init", "-q", "-b", "main")
            _run(md, "config", "user.email", "t@t")
            _run(md, "config", "user.name", "t")
            (pathlib.Path(md) / "base.txt").write_text("base\n")
            _run(md, "add", "base.txt")
            _run(md, "commit", "-q", "-m", "base")
            _run(md, "switch", "-q", "-c", "feat")
            (pathlib.Path(md) / "feat.txt").write_text("feat\n")
            _run(md, "add", "feat.txt")
            _run(md, "commit", "-q", "-m", "featfile")
            _run(md, "switch", "-q", "main")
            (pathlib.Path(md) / "base.txt").write_text("base2\n")
            _run(md, "commit", "-q", "-am", "mainadv")
            _run(md, "merge", "-q", "--no-ff", "feat", "-m", "merge")
            msha = _run(md, "rev-parse", "HEAD").stdout.strip()
            assert any(f["path"] == "feat.txt" for f in commit_files(md, msha)), "merge veio vazio"
            assert "feat" in commit_file_diff(md, msha, "feat.txt")["diff"], "diff do merge vazio"

        # commit de um arquivo RENOMEADO: mantem o rename atômico (nao vira add + delecao órfã).
        with tempfile.TemporaryDirectory() as rn:
            _run(rn, "init", "-q", "-b", "main")
            _run(rn, "config", "user.email", "t@t")
            _run(rn, "config", "user.name", "t")
            (pathlib.Path(rn) / "old.txt").write_text("X\n")
            _run(rn, "add", "old.txt")
            _run(rn, "commit", "-q", "-m", "base")
            _run(rn, "mv", "old.txt", "new.txt")
            commit(rn, "rename", ["new.txt"])
            assert _run(rn, "show", "--name-status", "--format=", "HEAD").stdout[:1] == "R", "rename perdido"
            assert not _run(rn, "status", "--porcelain").stdout.strip(), "sobrou delecao órfã staged"

        print("git_ops self-check OK")
