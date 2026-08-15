"""Busca por NOME e por CONTEUDO, os dois via git.

Por que git e nao os.walk + leitura: o `ls-files` ja respeita o .gitignore (senao a busca
mergulharia em node_modules) e o `grep -I` ja pula binario. Preco: os dois modos exigem um
repositorio git, e isso e dito na cara em vez de virar lista vazia.
"""

import os

from app import git_ops

MAX_HITS = 200


class SearchError(Exception):
    def __init__(self, status: int, code: str, msg: str):
        super().__init__(msg)
        self.status, self.code, self.msg = status, code, msg


def _git(cwd: str, *args: str):
    """Unico ponto que fala com o git no modulo: falha de subprocesso (git ausente,
    timeout, exec) vira SearchError no envelope da busca, nunca excecao solta."""
    try:
        return git_ops._run(cwd, *args)
    except git_ops.GitError as e:
        raise SearchError(e.status, "erro_arq_busca_falhou", e.detail or str(e)) from None


def _e_repo(cwd: str) -> bool:
    p = _git(cwd, "-c", "core.quotePath=false", "rev-parse", "--is-inside-work-tree")
    return p.returncode == 0 and p.stdout.strip() == "true"


def search(cwd: str, q: str, mode: str) -> dict:
    if not q or not q.strip():
        raise SearchError(400, "erro_arq_busca_vazia", "digite algo pra buscar")
    # NUL quebra subprocess com ValueError solto — recusar antes de chegar no git.
    if "\x00" in q:
        raise SearchError(400, "erro_arq_busca_falhou", "termo de busca invalido")
    if mode not in ("names", "contents"):
        raise SearchError(400, "erro_arq_modo_invalido", "modo de busca invalido")
    if not _e_repo(cwd):
        # Vale pros DOIS modos: names usa ls-files, contents usa grep.
        raise SearchError(409, "erro_arq_nao_e_repo_git", "a busca precisa de um repositorio git")

    hits = (_por_nome if mode == "names" else _por_conteudo)(cwd, q)
    return {"hits": hits[:MAX_HITS], "truncated": len(hits) > MAX_HITS, "mode": mode}


def _por_nome(cwd: str, q: str) -> list[dict]:
    p = _git(cwd, "-c", "core.quotePath=false",
             "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    if p.returncode != 0:
        raise SearchError(409, "erro_arq_busca_falhou", (p.stderr or "git ls-files falhou").strip())
    alvo = q.lower()
    vistos = set()
    fora = []
    for c in p.stdout.split("\0"):
        if not c or c in vistos:
            continue                      # conflito de merge repete o path (um por stage)
        vistos.add(c)
        # Rastreado que foi apagado da working tree nao abre na tela: nao lista.
        # lexists testa a entrada (inclusive symlink) sem seguir o alvo.
        if not os.path.lexists(os.path.join(cwd, c)):
            continue
        if alvo in c.lower():
            fora.append({"path": c, "line": None, "text": None})
    return fora


def _por_conteudo(cwd: str, q: str) -> list[dict]:
    # -z: o framing vira `path\0line\0text\n` — o path sai LITERAL (nao cotado),
    # entao nome com dois-pontos, aspas, tab ou quebra de linha sobrevive.
    # -e <termo>: a forma documentada de dizer "isto e o padrao, nao uma flag".
    p = _git(cwd, "-c", "core.quotePath=false",
             "grep", "-z", "-n", "-I", "--untracked", "-F", "-e", q)
    if p.returncode == 1:                      # 1 = nao achou nada. NAO e erro.
        return []
    if p.returncode != 0:
        raise SearchError(409, "erro_arq_busca_falhou", (p.stderr or "git grep falhou").strip())
    fora = []
    resto = p.stdout
    while True:
        path, sep, resto = resto.partition("\0")
        if not sep:
            break
        linha, sep, resto = resto.partition("\0")
        if not sep:
            break
        # O texto e UMA linha do arquivo (nunca contem \n); o \n seguinte e o
        # terminador que o git poe no fim de cada registro. O path pode conter
        # \n, por isso ele sai ANTES, por delimitador NUL, e nunca por split de linha.
        texto, sep, resto = resto.partition("\n")
        fora.append({"path": path, "line": int(linha), "text": texto})
    return fora
