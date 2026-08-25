"""O que cada versão exige que seja feito nesta máquina, e o registro do que já foi.

Um arquivo por passo em `docs/atualizacoes/<id>.md`, versionado junto com o código que o exige.
Um arquivo por passo, e não um `.json` único com todos: dois commits que mexem no mesmo arquivo
conflitam, e um arquivo por passo nunca conflita com o passo de outra pessoa.

**O registro é do que JÁ RODOU aqui, não de onde a máquina veio.** A alternativa — comparar o
intervalo `commit_antigo..commit_novo` e rodar o que entrou no meio — parece mais simples e o git
até faz a conta sozinho, mas fura em dois casos que acontecem de verdade: instalação nova (não há
"commit antigo", e rodar a história inteira de passos seria errado) e máquina que reclonou ou
resetou (o intervalo mente). Guardar os ids aplicados num sidecar é o padrão de migração de banco
(Django, Rails, Flyway) e não depende de saber o passado da máquina.

**A prova é o que separa "rodou" de "deu certo".** Um passo só entra no registro depois do seu
comando de verificação passar. Sem isso, "sucesso" quer dizer "o comando saiu com 0", que foi
exatamente o que produziu o `-Update` dizendo ok com o processo antigo ainda no ar
(`install.ps1:1242`).
"""
from __future__ import annotations

import json
import logging
import os
import subprocess
from pathlib import Path

from app import atomico

_log = logging.getLogger("hangar.atualizacoes")

REPO = Path(__file__).resolve().parents[2]

_TIMEOUT = 600.0


def _dir() -> Path:
    return REPO / "docs" / "atualizacoes"


def _base() -> Path:
    raiz = Path(os.environ.get("CLAUDE_CONFIG_DIR") or (Path.home() / ".claude"))
    return raiz / ".hangar-update"


def _caminho_aplicados() -> Path:
    return _base() / "aplicados.json"


# ─── Ler os passos ─────────────────────────────────────────────────────────────────────────────

def _frontmatter(texto: str) -> tuple[dict, str]:
    """Frontmatter `chave: valor` entre `---`, e o corpo.

    Parser próprio de duas dezenas de linhas em vez de YAML: o formato aqui é chave e valor, um por
    linha, e trazer uma dependência (ou reimplementar YAML de verdade, com listas, aninhamento e
    âncoras) para isso seria pagar caro por nada. Valor com `:` dentro sobrevive — só o primeiro
    separa.
    """
    linhas = texto.splitlines()
    if not linhas or linhas[0].strip() != "---":
        return {}, texto
    try:
        fim = linhas.index("---", 1)
    except ValueError:
        return {}, texto
    campos: dict = {}
    for linha in linhas[1:fim]:
        if not linha.strip() or linha.lstrip().startswith("#"):
            continue
        chave, sep, valor = linha.partition(":")
        if not sep:
            continue
        campos[chave.strip()] = valor.strip()
    return campos, "\n".join(linhas[fim + 1:]).strip()


def _passo(arquivo: Path) -> dict | None:
    campos, corpo = _frontmatter(arquivo.read_text(encoding="utf-8"))
    ident = campos.get("id") or arquivo.stem
    if not campos.get("titulo"):
        # Falha aparece, mas não derruba a leitura: um arquivo malformado não pode impedir que os
        # outros passos rodem — seria uma versão nova travando a atualização de todo mundo.
        _log.warning("passo %s ignorado: falta 'titulo'", arquivo.name)
        return None
    return {
        "id": ident,
        "titulo": campos["titulo"],
        "comando": campos.get("comando", "").strip(),
        "prova": campos.get("prova", "").strip(),
        "destrutivo": campos.get("destrutivo", "").strip().lower() in ("true", "sim", "1"),
        "texto": corpo,
        "arquivo": arquivo.name,
    }


def todos() -> list[dict]:
    """Todos os passos declarados no repo, em ordem de id (que começa com a data)."""
    pasta = _dir()
    if not pasta.is_dir():
        return []
    achados = []
    for arquivo in sorted(pasta.glob("*.md")):
        if arquivo.name.upper().startswith("README"):
            continue
        try:
            p = _passo(arquivo)
        except OSError as e:
            _log.warning("passo %s ilegivel: %s", arquivo.name, e)
            continue
        if p:
            achados.append(p)
    return achados


# ─── Registro ──────────────────────────────────────────────────────────────────────────────────

def aplicados() -> set[str]:
    try:
        bruto = json.loads(_caminho_aplicados().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return set()
    # Lista de strings, e nada mais: JSON válido do tipo errado não pode virar AttributeError lá na
    # frente (mesma regra do `estado()` em atualizar.py).
    if not isinstance(bruto, list):
        return set()
    return {x for x in bruto if isinstance(x, str)}


def _gravar(ids: set[str]) -> None:
    alvo = _caminho_aplicados()
    alvo.parent.mkdir(parents=True, exist_ok=True)
    tmp = alvo.with_suffix(f".{os.getpid()}.tmp")
    tmp.write_text(json.dumps(sorted(ids), ensure_ascii=False), encoding="utf-8")
    atomico.substituir(tmp, alvo)


def marcar(ident: str) -> None:
    _gravar(aplicados() | {ident})


def marcar_todos() -> int:
    """Marca tudo como aplicado, sem rodar nada. É o que a instalação do zero chama.

    Sem isto, a primeira atualização de uma máquina recém-instalada rodaria a história inteira de
    passos — todos eles já satisfeitos pelo instalador que acabou de rodar.
    """
    ids = {p["id"] for p in todos()}
    _gravar(aplicados() | ids)
    return len(ids)


def pendentes(incluir_destrutivos: bool = True) -> list[dict]:
    ja = aplicados()
    return [p for p in todos()
            if p["id"] not in ja and (incluir_destrutivos or not p["destrutivo"])]


# ─── Aplicar ───────────────────────────────────────────────────────────────────────────────────

def _rodar(linha: str) -> subprocess.CompletedProcess:
    """Roda pelo shell: o `comando` do passo é escrito por gente, com pipe e `&&` quando precisa."""
    return subprocess.run(
        linha, shell=True, cwd=str(REPO), capture_output=True, text=True,
        encoding="utf-8", errors="replace", timeout=_TIMEOUT,
        env={**os.environ, "LC_ALL": "C", "LANGUAGE": "C"},
    )


class PassoFalhou(Exception):
    def __init__(self, passo: dict, motivo: str):
        super().__init__(f"{passo['titulo']}: {motivo}")
        self.passo = passo
        self.motivo = motivo


def aplicar(passo: dict) -> None:
    """Roda um passo e sua prova. Marca no registro só quando a prova passa."""
    if passo["comando"]:
        p = _rodar(passo["comando"])
        if p.returncode != 0:
            cauda = "\n".join((p.stderr or p.stdout or "").strip().splitlines()[-8:])
            raise PassoFalhou(passo, cauda or f"saiu com {p.returncode}")

    if passo["prova"]:
        v = _rodar(passo["prova"])
        if v.returncode != 0:
            # Comando ok e prova falhando é o caso que o registro existe pra pegar: sem isto o
            # passo entraria como aplicado e nunca mais rodaria, com o efeito dele ausente.
            raise PassoFalhou(passo, "o passo rodou mas a verificacao nao passou")

    marcar(passo["id"])
    _log.info("passo aplicado: %s (%s)", passo["id"], passo["titulo"])


def aplicar_pendentes(incluir_destrutivos: bool = True) -> list[str]:
    """Aplica o que falta, em ordem. Levanta `PassoFalhou` no primeiro que não passar.

    Para no primeiro erro de propósito: passo de versão costuma depender do anterior, e seguir em
    frente deixaria a máquina num estado que ninguém desenhou.
    """
    feitos = []
    for passo in pendentes(incluir_destrutivos):
        aplicar(passo)
        feitos.append(passo["id"])
    return feitos
