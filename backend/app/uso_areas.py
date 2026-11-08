"""Área do código (front, back, banco, infra, docs, outros) de um caminho tocado por uma tool.

Mapa editável em `~/.hangar/uso-areas.json` (opcional; sem ele vale `PADRAO`):

    {
      "padrao":   [["banco", ["*.sql"]], ["front", ["web/*"]]],
      "projetos": {"/home/eu/acme-web": [["front", ["src/app/*"]], ["back", ["src/server/*"]]],
                   "acme-api": [["back", ["*.ts"]]]}
    }

- A primeira regra que casa vence: as do projeto antes, depois `padrao` (que substitui `PADRAO`).
- Projeto = caminho absoluto (prefixo do cwd) ou nome de uma pasta do cwd.
- Padrão é `fnmatch` sobre o caminho relativo à raiz do repositório e casa em qualquer nível
  (`migrations/*` pega `backend/migrations/x.sql`). `skill:<nome>` casa a chamada de skill.
- O mapa é lido UMA vez por processo. O cache guarda os ALVOS de cada turno (caminhos, comandos,
  skills), não a área: editou, reinicie o backend e a próxima coleta refaz só as linhas de área
  a partir do cache, sem reler transcript.
Só stdlib.
"""
from __future__ import annotations

import fnmatch
import hashlib
import json
import re
from functools import lru_cache
from pathlib import Path

OUTROS = "outros"
CONVERSA = "conversa"
# Suba ao mudar a divisão ou a ordem das áreas: as linhas ficam gravadas no cache.
_DIVISAO = 3

PADRAO: list[tuple[str, list[str]]] = [
    ("banco", ["*.sql", "migrations/*", "prisma/*", "skill:*database*"]),
    ("infra", ["Dockerfile*", "*.dockerfile", "docker-compose*", "k8s/*", "helm/*", "charts/*",
               ".github/*", ".gitlab-ci.yml", "Jenkinsfile", "scripts/*", "deploy/*", "install.*",
               "*.sh", "*.ps1"]),
    ("docs", ["docs/*", "*.md"]),
    ("front", ["frontend/*", "packages/core/*", "mobile/*", "messages/*", "locales/*", "*.svelte",
               "*.tsx", "*.jsx", "*.css", "*.scss", "*.html", "*.dart"]),
    ("back", ["backend/*", "*pserver*", "*.py", "*.cs", "*.pas", "*.go"]),
]


def repartir(valor: int, pesos: dict[str, int]) -> dict[str, int]:
    """Divide `valor` na proporção dos pesos por maior resto: a soma fecha exata, sem sobra."""
    total = sum(pesos.values())
    exatos = {a: valor * n / total for a, n in pesos.items()}
    inteiros = {a: int(x) for a, x in exatos.items()}
    # Empate de resto decide pelo nome: a ordem dos pesos vem de um set e muda a cada processo.
    for a in sorted(exatos, key=lambda a: (inteiros[a] - exatos[a], a))[:valor - sum(inteiros.values())]:
        inteiros[a] += 1
    return inteiros


def _arquivo() -> Path:
    return Path.home() / ".hangar" / "uso-areas.json"


def _regras(bruto) -> list[tuple[str, list[str]]]:
    out = []
    for item in bruto if isinstance(bruto, list) else []:
        if (isinstance(item, list) and len(item) == 2 and isinstance(item[0], str)
                and isinstance(item[1], list)):
            out.append((item[0], [p for p in item[1] if isinstance(p, str)]))
    return out


@lru_cache(maxsize=1)
def _mapa() -> tuple[str, list, dict[str, list]]:
    # O PADRAO do código e a regra de divisão entram na assinatura: mudar qualquer um relê o cache.
    try:
        texto = _arquivo().read_text(encoding="utf-8")
        bruto = json.loads(texto)
    except (OSError, ValueError):
        texto, bruto = "", None
    # A seleção corrigida precisa reconstruir também as áreas já salvas no índice.
    assinatura = hashlib.sha256((f"divisao:{_DIVISAO}project-paths:1" + repr(PADRAO) + texto).encode()).hexdigest()[:12]
    if not isinstance(bruto, dict):
        return assinatura, PADRAO, {}
    padrao = _regras(bruto["padrao"]) if "padrao" in bruto else PADRAO
    projetos = bruto.get("projetos") if isinstance(bruto.get("projetos"), dict) else {}
    return assinatura, padrao, {k: _regras(v) for k, v in projetos.items()}


def assinatura() -> str:
    return _mapa()[0]


def recarregar() -> None:
    _mapa.cache_clear()


def regras_de(cwd: str) -> list[tuple[str, list[str]]]:
    # O transcript pode usar barras diferentes das do host que lê o mapa.
    cwd = cwd.replace("\\", "/")
    _, padrao, projetos = _mapa()
    partes = Path(cwd).parts
    do_projeto = []
    for chave, regras in projetos.items():
        chave = chave.replace("\\", "/")
        if "/" in chave:
            if cwd == chave or cwd.startswith(chave.rstrip("/") + "/"):
                do_projeto += regras
        elif chave in partes:
            do_projeto += regras
    return do_projeto + padrao


@lru_cache(maxsize=1024)
def _casador(padroes: tuple[str, ...]) -> re.Pattern:
    """Os padrões de uma área numa regex só, aplicada a `/` + alvo: casa o alvo inteiro (o `/`
    vira literal na frente) ou qualquer sufixo dele depois de uma barra (`*/padrao`)."""
    partes = [f"/{fnmatch.translate(p)}|{fnmatch.translate('*/' + p)}" for p in padroes]
    return re.compile("|".join(partes) or r"(?!)")


def area_do_alvo(alvo: str, regras: list[tuple[str, list[str]]]) -> str | None:
    for area, padroes in regras:
        if _casador(tuple(padroes)).match("/" + alvo):
            return area
    return None


def candidatos_do_comando(cmd: str) -> tuple[str, ...]:
    """Palavras de um comando que parecem caminho (com `/` ou extensão). Não depende do mapa:
    é o que o cache guarda do comando."""
    out = []
    for tok in cmd.replace("=", " ").split():
        t = tok.strip("'\"();&|<>")
        if not t or t.startswith("-") or "://" in t or "$" in t:
            continue
        if "/" not in t and "." not in t.lstrip("."):
            continue
        out.append(t)
    return tuple(out)
