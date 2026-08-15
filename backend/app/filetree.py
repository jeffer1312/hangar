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
    # reflog e o objects carregam historia. Comparacao por COMPONENTE (nao prefixo de texto):
    # .gitignore e .github/ continuam legiveis, e o realpath ja resolveu ./x/.git e sub/../.git.
    # A RAIZ tambem passa pela regua: uma sessao com cwd em /repo/.git leria `config` como
    # caminho relativo LIMPO — o guard do alvo nunca veria o .git.
    if ".git" in raiz.parts:
        raise FileError(403, "erro_arq_area_do_git", "area interna do git")
    if ".git" in (alvo.relative_to(raiz).parts if alvo != raiz else ()):
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
    p = git_ops._run(cwd, "rev-parse", "--show-prefix")
    return p.stdout.strip() if p.returncode == 0 else ""


def _e_repo(cwd: str) -> bool:
    from app import git_ops
    p = git_ops._run(cwd, "rev-parse", "--is-inside-work-tree")
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
        # O detalhe vai so pro log (via _erro_arq no api); o envelope e traduzivel.
        raise FileError(500, "erro_arq_lista_falhou", e.detail or "git falhou") from None
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
    """path RELATIVO AO CWD -> (add, del). UMA chamada pro repo inteiro, nunca uma por arquivo."""
    from app import git_ops
    pref = _prefixo_no_repo(cwd)
    p = git_ops._run(cwd, "-c", "core.quotePath=false", "diff", "--numstat", "HEAD")
    if p.returncode != 0:
        return {}
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
