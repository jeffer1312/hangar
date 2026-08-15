"""Busca por NOME e por CONTEUDO, os dois via git.

Por que git e nao os.walk + leitura: o `ls-files` ja respeita o .gitignore (senao a busca
mergulharia em node_modules) e o `grep -I` ja pula binario. Preco: os dois modos exigem um
repositorio git, e isso e dito na cara em vez de virar lista vazia.
"""

from app import git_ops

MAX_HITS = 200


class SearchError(Exception):
    def __init__(self, status: int, code: str, msg: str):
        super().__init__(msg)
        self.status, self.code, self.msg = status, code, msg


def _e_repo(cwd: str) -> bool:
    p = git_ops._run(cwd, "rev-parse", "--is-inside-work-tree")
    return p.returncode == 0 and p.stdout.strip() == "true"


def search(cwd: str, q: str, mode: str) -> dict:
    if not q or not q.strip():
        raise SearchError(400, "erro_arq_busca_vazia", "digite algo pra buscar")
    if mode not in ("names", "contents"):
        raise SearchError(400, "erro_arq_modo_invalido", "modo de busca invalido")
    if not _e_repo(cwd):
        # Vale pros DOIS modos: names usa ls-files, contents usa grep.
        raise SearchError(409, "erro_arq_nao_e_repo_git", "a busca precisa de um repositorio git")

    hits = (_por_nome if mode == "names" else _por_conteudo)(cwd, q)
    return {"hits": hits[:MAX_HITS], "truncated": len(hits) > MAX_HITS, "mode": mode}


def _por_nome(cwd: str, q: str) -> list[dict]:
    p = git_ops._run(cwd, "-c", "core.quotePath=false",
                     "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    if p.returncode != 0:
        raise SearchError(409, "erro_arq_busca_falhou", (p.stderr or "git ls-files falhou").strip())
    alvo = q.lower()
    return [{"path": c, "line": None, "text": None}
            for c in p.stdout.split("\0") if c and alvo in c.lower()]


def _por_conteudo(cwd: str, q: str) -> list[dict]:
    # -e <termo>: a forma documentada de dizer "isto e o padrao, nao uma flag".
    p = git_ops._run(cwd, "-c", "core.quotePath=false",
                     "grep", "-n", "-I", "--untracked", "-F", "-e", q)
    if p.returncode == 1:                      # 1 = nao achou nada. NAO e erro.
        return []
    if p.returncode != 0:
        raise SearchError(409, "erro_arq_busca_falhou", (p.stderr or "git grep falhou").strip())
    fora = []
    for linha in p.stdout.splitlines():
        partes = linha.split(":", 2)
        if len(partes) == 3 and partes[1].isdigit():
            fora.append({"path": partes[0], "line": int(partes[1]), "text": partes[2]})
    return fora
