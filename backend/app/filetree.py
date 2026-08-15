"""Arvore de arquivos do repo DA SESSAO.

Nao reusa a allowlist do fs.py de proposito: la a raiz e a lista de projetos (o seletor
de pasta na criacao de sessao), aqui a raiz e o cwd da sessao. Mesma trava de caminho,
politica de raiz diferente.
"""

import os
from pathlib import Path

MAX_ENTRADAS = 1000
MAX_BYTES = 512 * 1024


class FileError(Exception):
    def __init__(self, status: int, code: str, msg: str):
        super().__init__(msg)
        self.status, self.code, self.msg = status, code, msg


def _real(p: str) -> Path:
    return Path(os.path.realpath(os.path.expanduser(p)))


def _within(child: Path, root: Path) -> bool:
    return child == root or root in child.parents


def _git_dir_por_fs(cwd: str) -> Path | None:
    """Fallback conservador quando o git nao responde (config malformada, repo quebrado):
    caminha do cwd RESOLVIDO pra cima e so trata como git-dir um diretorio componente
    chamado `.git` COM os quatro marcadores administrativos (HEAD, config, objects, refs).
    Pasta ancestral chamada .git sem os marcadores (ex: /tmp/.git que so contem um projeto)
    nao conta. O realpath no comeco e obrigatorio: um cwd que seja SYMLINK DIRETO para o
    git-dir (git-area-alias -> /repo/.git) nao tem componente .git no caminho lexical, e a
    caminhada lexical devolvia None — o guard nao rodava e a config vazava (medido no
    parecer 5ded6dbe)."""
    atual = Path(os.path.realpath(cwd))
    while True:
        for cand in (atual, atual / ".git"):
            if cand.name == ".git" and (cand / "HEAD").is_file() and (cand / "config").is_file() \
                    and (cand / "objects").is_dir() and (cand / "refs").is_dir():
                return Path(os.path.realpath(cand))
        if atual == atual.parent:
            return None
        atual = atual.parent


def _git_dir(cwd: str) -> Path | None:
    """O git-dir REAL do repo que contem o cwd (resolve .git FILE, worktree, GIT_DIR).
    Fora de repo, None. Duas vias: o proprio git responde, ou (git quebrado/config
    malformada — o rev-parse sai 128) a descoberta conservadora por filesystem acima.
    Sem o fallback, config malformada liberava o cwd /repo/.git e o read lia o proprio
    .git/config com o token (medido no parecer 72e866e3).
    Falha de subprocesso (timeout) vira FileError: sem o guard, o git-dir fica exposto."""
    from app import git_ops
    try:
        p = git_ops._run(cwd, "rev-parse", "--absolute-git-dir")
    except git_ops.GitError as e:
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    if p.returncode != 0:
        return _git_dir_por_fs(cwd)
    # realpath dos DOIS lados da comparacao: o stdout do git e caminho fisico (getcwd),
    # mas normalizar nao custa nada e garante que o _within casa com raiz/alvo resolvidos.
    return Path(os.path.realpath(p.stdout.strip()))


def _resolver(cwd: str, path: str | None) -> tuple[Path, Path]:
    """Devolve (raiz, alvo) com o alvo provado dentro da raiz."""
    raiz = _real(cwd)
    # Recusado ANTES de virar caminho: um path assim acabaria como flag num comando git.
    if path and path.startswith("-"):
        raise FileError(400, "erro_arq_caminho_invalido", "caminho invalido")
    # NUL quebra realpath() e subprocess com ValueError solto — recusar antes dos dois.
    if path and "\x00" in path:
        raise FileError(400, "erro_arq_caminho_invalido", "caminho invalido")
    # So caminho RELATIVO: absoluto dentro do cwd tambem e recusado (o front so manda relativo).
    if path and os.path.isabs(path):
        raise FileError(400, "erro_arq_caminho_invalido", "caminho absoluto nao aceito")
    alvo = _real(os.path.join(cwd, path)) if path else raiz
    if not _within(alvo, raiz):
        raise FileError(400, "erro_arq_fora_da_raiz", "caminho sai da raiz da sessao")
    # Area interna do git fora do alcance: o .git/config carrega o token do remote, e o
    # reflog e o objects carregam historia. Duas reguas, as duas sobre o caminho JA resolvido:
    # (1) o GIT-DIR REAL, que o proprio git identifica (resolve .git FILE e worktree) — e o
    #     que recusa o cwd DENTRO do git-dir (raiz == gitdir) sem confundir uma PASTA
    #     ANCESTRAL chamada .git (um repo em /tmp/.git/projeto tem o git-dir em
    #     /tmp/.git/projeto/.git, e a raiz fica FORA dele — medido no parecer 2ac646c);
    # (2) por COMPONENTE do alvo relativo a raiz, que continua valendo mesmo com o git
    #     quebrado (repo corrompido, .git orfao): .gitignore e .github/ ficam legiveis.
    gitdir = _git_dir(cwd)
    if gitdir is not None and _within(raiz, gitdir):
        raise FileError(403, "erro_arq_area_do_git", "area interna do git")
    if ".git" in (alvo.relative_to(raiz).parts if alvo != raiz else ()):
        raise FileError(403, "erro_arq_area_do_git", "area interna do git")
    if gitdir is not None and _within(alvo, gitdir):
        raise FileError(403, "erro_arq_area_do_git", "area interna do git")
    if not alvo.exists():
        raise FileError(404, "erro_arq_inexistente", "caminho nao existe")
    return raiz, alvo


def _prefixo_no_repo(cwd: str) -> str:
    """Onde o cwd fica DENTRO do repo, com barra no fim ('' se for o topo).

    Existe porque o git devolve caminho relativo ao TOPO do repositorio, e esta arvore
    lista relativo ao cwd DA SESSAO. Uma sessao aberta em /repo/backend recebia
    {"backend/app/x.py": "M"} do git e comparava com "app/x.py" — nunca casava, e a arvore
    voltava VAZIA no modo so_modificados (que e o padrao). Bug encontrado na auditoria do
    plano, antes de custar uma Task.
    """
    from app import git_ops
    try:
        p = git_ops._run(cwd, "rev-parse", "--show-prefix")
    except git_ops.GitError as e:
        # Falha de subprocesso (git ausente/timeout) NAO e "fora de repo" — e erro.
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    return p.stdout.strip() if p.returncode == 0 else ""


def _e_repo(cwd: str) -> bool:
    from app import git_ops
    try:
        p = git_ops._run(cwd, "rev-parse", "--is-inside-work-tree")
    except git_ops.GitError as e:
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    return p.returncode == 0 and p.stdout.strip() == "true"


def _marcas(cwd: str) -> dict[str, str]:
    """path RELATIVO AO CWD -> letra do porcelain. Fora de repo git, ERRO no envelope:
    arvore vazia escondia a falha (antes: except Exception -> {}, e so_modificados
    devolvia 200 [] como se nada tivesse mudado)."""
    from app import git_ops
    if not _e_repo(cwd):
        raise FileError(409, "erro_arq_nao_e_repo_git", "a arvore precisa de um repositorio git")
    pref = _prefixo_no_repo(cwd)
    try:
        brutas = git_ops.changed_files(cwd)
    except git_ops.GitError as e:
        # Status do git PRESERVADO (504 de timeout chega como 504): 500 fixo comeria o
        # status que o cliente usa pra decidir retry. O detalhe vai so pro log.
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    fora = {}
    for c in brutas:
        p = c["path"]
        if pref:
            if not p.startswith(pref):
                continue                    # mudou fora do cwd desta sessao: nao e da arvore
            p = p[len(pref):]
        fora[p.rstrip("/")] = c["code"].strip()[:1] or "?"
    return fora


def _numstat(cwd: str) -> dict[str, tuple[int, int]]:
    """path RELATIVO AO CWD -> (add, del). UMA chamada pro repo inteiro, nunca uma por arquivo.
    Contagem so sai quando o comando terminou bem: zero por falha seria resposta FALSA
    (marca M com add=0/del=0 mentirosos, medido no parecer 2ac646c)."""
    from app import git_ops
    pref = _prefixo_no_repo(cwd)
    # Sem HEAD nao ha o que contar. "Sem HEAD" e SO returncode 1 sem stderr (ausencia
    # normal); 128 com stderr e HEAD quebrado/falha real — virar lista vazia seria
    # esconder a falha (medido no parecer 72e866e3). GitError do probe escapa do envelope
    # se ficar fora do try — esta dentro, com o status preservado.
    try:
        probe = git_ops._run(cwd, "rev-parse", "--verify", "-q", "HEAD")
    except git_ops.GitError as e:
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    if probe.returncode == 1 and not probe.stderr.strip():
        return {}
    if probe.returncode != 0:
        raise FileError(500, "erro_arq_lista_falhou",
                        (probe.stderr or "rev-parse HEAD falhou").strip())
    try:
        p = git_ops._run(cwd, "-c", "core.quotePath=false", "diff", "--numstat", "HEAD")
    except git_ops.GitError as e:
        raise FileError(e.status, "erro_arq_lista_falhou", e.detail or "git falhou") from None
    if p.returncode != 0:
        raise FileError(500, "erro_arq_lista_falhou",
                        (p.stderr or "git diff --numstat falhou").strip())
    fora = {}
    for linha in p.stdout.splitlines():
        partes = linha.split("\t")
        if len(partes) != 3 or partes[0] == "-":       # "-" = binario
            continue
        cam = partes[2]
        if pref:
            if not cam.startswith(pref):
                continue
            cam = cam[len(pref):]
        fora[cam] = (int(partes[0]), int(partes[1]))
    return fora


def list_dir(cwd: str, path: str | None = None, so_modificados: bool = True) -> dict:
    raiz, alvo = _resolver(cwd, path)
    if not alvo.is_dir():
        raise FileError(400, "erro_arq_nao_e_pasta", "nao e uma pasta")

    marcas, nums = _marcas(cwd), _numstat(cwd)
    entradas, cortou = [], False
    try:
        bruto = sorted(os.scandir(alvo), key=lambda e: (not e.is_dir(), e.name.lower()))
    except PermissionError:
        raise FileError(403, "erro_arq_sem_permissao", "sem permissao de leitura")
    except FileNotFoundError:
        raise FileError(404, "erro_arq_inexistente", "a pasta sumiu")
    except OSError:
        raise FileError(500, "erro_arq_lista_falhou", "nao deu pra ler a pasta")
    for e in bruto:
        if e.name == ".git":
            continue
        filho = _real(e.path)
        if not _within(filho, raiz):        # symlink apontando pra fora
            continue
        try:
            tam = 0 if e.is_dir() else e.stat().st_size
        except OSError:                      # symlink quebrado: aparece, sem tamanho
            tam = 0
        rel = os.path.relpath(e.path, raiz)   # caminho LOGICO: o nome do link, nao o do alvo
        # Pasta herda a marca e SOMA o +N -M dos descendentes.
        dentro = [p for p in marcas if p == rel or p.startswith(rel + "/")]
        marca = marcas.get(rel) or (marcas[dentro[0]] if e.is_dir() and dentro else None)
        add = sum(nums.get(p, (0, 0))[0] for p in dentro)
        rem = sum(nums.get(p, (0, 0))[1] for p in dentro)
        if so_modificados and marca is None:
            continue
        if len(entradas) >= MAX_ENTRADAS:
            cortou = True
            break
        entradas.append({
            "name": e.name, "path": rel, "is_dir": e.is_dir(), "size": tam,
            "changed": marca, "add": add, "del": rem,
        })
    return {"entries": entradas, "truncated": cortou}


def read_file(cwd: str, path: str) -> dict:
    _raiz, alvo = _resolver(cwd, path)
    if alvo.is_dir():
        raise FileError(400, "erro_arq_e_pasta", "isso e uma pasta")
    if not alvo.is_file():
        raise FileError(400, "erro_arq_nao_e_arquivo", "nao e um arquivo comum")
    try:
        with alvo.open("rb") as fh:
            cabeca = fh.read(8192)
            if b"\x00" in cabeca:
                raise FileError(415, "erro_arq_binario", "arquivo binario")
            resto = fh.read(MAX_BYTES - len(cabeca) + 1)
            if b"\x00" in resto:
                raise FileError(415, "erro_arq_binario", "arquivo binario")
    except PermissionError:
        raise FileError(403, "erro_arq_sem_permissao", "sem permissao de leitura")
    bruto = cabeca + resto
    cortou = len(bruto) > MAX_BYTES
    return {
        "path": path,
        "text": bruto[:MAX_BYTES].decode("utf-8", errors="replace"),
        "size": alvo.stat().st_size,
        "truncated": cortou,
    }
