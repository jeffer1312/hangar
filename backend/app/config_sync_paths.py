"""Caminhos da configuração compartilhada.

A origem troca cada caminho absoluto por um marcador e o destino troca o marcador pelo valor
dele. Trocar `/home/x` por `~` não serve: o destino pode ser Windows, e o programa do comando (o
node de uma versão do fnm, o python de um venv) pode nem existir lá.
"""
import os
import re
import shutil
from dataclasses import dataclass
from pathlib import Path, PurePath
from typing import Callable

MARKERS = ("HANGAR", "CLAUDE", "CODEX", "HOME")
# Recriáveis e grandes: ficam fora do pacote e, no destino, sobrevivem à troca da pasta.
HEAVY_DIRS = frozenset({".git", "node_modules", ".venv", "venv", "__pycache__"})
# Programa que muda de lugar entre máquinas: o destino procura pelo nome no próprio PATH.
PROGRAMS = frozenset({"python", "python3", "node", "bash", "sh", "pwsh", "powershell", "uv",
                      "bun", "deno", "npx"})
_SHELL_WORDS = frozenset({"if", "then", "else", "fi", "[", "[[", "test", "exec", "env", "cd",
                          "for", "while", "do", "done", "!"})
_END = r"(?=$|[\\/\s'\"`;|&)])"
_ANY = r"⟦(?:HANGAR|CLAUDE|CODEX|HOME)⟧"
_QUOTED = re.compile(r"(['\"])(" + _ANY + r"[^'\"]*)\1")
_BARE = re.compile(_ANY + r"[^\s'\"`;|&)]*")
# ponytail: o resto do caminho para no primeiro espaço; caminho com espaço vindo de origem
# Windows fica com contrabarra depois do espaço. Resolver aspas aqui se isso aparecer.
_WITH_REST = re.compile(r"⟦(HANGAR|CLAUDE|CODEX|HOME)⟧([^\s'\"`;|&)]*)")
_TOKEN = re.compile(r"""(['"])([^'"]*)\1|([^\s'"`;|&()]+)""")
_ABSOLUTE = re.compile(r"^(?:/|[A-Za-z]:[\\/])")


def mark(name: str) -> str:
    return f"⟦{name}⟧"


@dataclass(frozen=True)
class Roots:
    hangar: str
    claude: str
    codex: str
    home: str

    @classmethod
    def this_machine(cls) -> "Roots":
        from app import codex_contas, contas
        return cls(hangar=str(Path(__file__).resolve().parents[2]),
                   claude=str(contas.compartilhado()),
                   codex=str(codex_contas.default_home()),
                   home=str(Path.home()))

    def value(self, marker: str) -> str:
        return {"HANGAR": self.hangar, "CLAUDE": self.claude,
                "CODEX": self.codex, "HOME": self.home}[marker]


def canonicalize(text: str, roots: Roots) -> str:
    """Caminho desta máquina -> marcador. O valor mais longo primeiro, e só em limite de caminho:
    `/home/joao2` não pode virar `⟦HOME⟧2`."""
    pairs = {(form, marker) for marker in MARKERS
             for form in (roots.value(marker), roots.value(marker).replace("\\", "/"))}
    for form, marker in sorted(pairs, key=lambda p: len(p[0]), reverse=True):
        text = re.sub(re.escape(form) + _END, mark(marker), text)
    return text


def resolve(text: str, roots: Roots) -> str:
    """Marcador -> valor desta máquina, sempre com `/` (no Windows, `C:/Users/x`): Python, Node,
    Git Bash e cmd aceitam, e o resto do caminho vindo de origem Windows perde a contrabarra."""
    def sub(m: re.Match) -> str:
        base = roots.value(m.group(1)).replace("\\", "/").rstrip("/")
        return base + m.group(2).replace("\\", "/")
    return _WITH_REST.sub(sub, text)


def marked_paths(text: str) -> list[str]:
    """Caminhos com marcador dentro de um comando, com ou sem aspas, na ordem, sem repetir."""
    found = [m.group(2) for m in _QUOTED.finditer(text)]
    found += _BARE.findall(_QUOTED.sub(" ", text))
    return list(dict.fromkeys(found))


def local_path(marked: str, roots: Roots) -> Path:
    return Path(resolve(marked, roots))


def map_strings(value, fn: Callable[[str], str]):
    if isinstance(value, str):
        return fn(value)
    if isinstance(value, list):
        return [map_strings(v, fn) for v in value]
    if isinstance(value, dict):
        return {(fn(k) if isinstance(k, str) else k): map_strings(v, fn) for k, v in value.items()}
    return value


def _find_program(name: str, which) -> str | None:
    fallback = {"python3": "python", "python": "python3"}.get(name)
    for candidate in (name, fallback):
        if candidate and (found := which(candidate)):
            return found
    return None


def fix_programs(command: str, which=None, exists=None) -> tuple[str, list[str]]:
    """Troca o programa que não existe nesta máquina pelo que o PATH daqui tem.

    Caminho absoluto que não existe é trocado quando é a primeira palavra do comando ou tem nome
    de interpretador conhecido. Primeira palavra sem caminho (`rtk`, `fnm`) só é conferida: se não
    existe aqui, entra na lista de faltando, para o relatório dizer por que o hook não roda.
    """
    which = which or shutil.which
    exists = exists or os.path.exists
    missing: list[str] = []
    first = True

    def swap(m: re.Match) -> str:
        nonlocal first
        quote, inside, bare = m.group(1), m.group(2), m.group(3)
        token = inside if inside is not None else bare
        is_first, first = first, False
        if not _ABSOLUTE.match(token):
            if (is_first and quote is None and token not in _SHELL_WORDS and "=" not in token
                    and token[:1] not in ("$", "~") and not which(token)):
                missing.append(token)
            return m.group(0)
        if exists(token):
            return m.group(0)
        base = PurePath(token.replace("\\", "/")).name.removesuffix(".exe").lower()
        if base not in PROGRAMS and not is_first:
            return m.group(0)
        found = _find_program(base, which)
        if not found:
            missing.append(base)
            return m.group(0)
        new = found.replace("\\", "/")
        if quote:
            return f"{quote}{new}{quote}"
        return f'"{new}"' if " " in new else new

    return _TOKEN.sub(swap, command), missing
