"""Tabela markdown de cabeçalho fixo dentro de um arquivo que é, no resto, prosa de outra pessoa.

É o que a política de contas (`~/.hangar/orquestracao-contas.md`) e o contrato do grupo
(`regras-<gid>.md`) têm em comum: o app é dono de UMA tabela (e de uma seção gerada), o árbitro
e o usuário são donos de todo o resto. Daí só três verbos — ler a tabela, trocar uma linha dela
no lugar, trocar uma seção inteira — e nunca "reescrever o arquivo".

Cabeçalho casa por forma normalizada (sem acento, sem backtick, sem negrito, minúsculo), porque
a mesma tabela escrita à mão vem `| Papel | Sessão |` e escrita pelo app vem `| papel | sessão |`.
Bloco cercado (```/~~~) é apagado ANTES da busca, preservando os offsets, mesma regra do
`planprog`: contrato traz exemplo de tabela dentro de fence.
"""
from __future__ import annotations

import os
import re
import threading
import unicodedata
from pathlib import Path

from . import atomico

_FENCE_RE = re.compile(r"^(```|~~~).*?^\1[^\n]*$", re.M | re.S)
_SEP_RE = re.compile(r"^\s*:?-{2,}:?\s*$")


def _sem_fences(texto: str) -> str:
    return _FENCE_RE.sub(lambda m: re.sub(r"[^\n]", " ", m.group(0)), texto)


def normalizar(celula: str) -> str:
    s = celula.strip().strip("`").replace("**", "").strip()
    s = unicodedata.normalize("NFKD", s)
    s = "".join(c for c in s if not unicodedata.combining(c))
    return re.sub(r"\s+", " ", s).lower()


def limpar(celula: str) -> str:
    """Valor da célula sem enfeite de markdown. `-` (vazio por convenção) vira ''."""
    s = celula.strip()
    s = re.sub(r"^\*\*(.*)\*\*$", r"\1", s).strip()
    s = s.strip("`").strip()
    return "" if s in ("-", "—") else s


def validar_celula(valor: str) -> str:
    """O arquivo é obedecido como instrução por um agente e digitado num terminal: `|` quebra a
    tabela, quebra de linha vira Enter."""
    # Também controle (ESC e afins): o recado é digitado no terminal, e uma sequência ANSI numa
    # "conta" vira título/tela forjada na sessão do árbitro.
    if "|" in valor or any(ord(c) < 0x20 or c == "\x7f" for c in valor):
        raise ValueError("celula com '|', quebra de linha ou caractere de controle")
    return valor.strip()


def _celulas(linha: str) -> list[str]:
    s = linha.strip()
    if s.startswith("|"):
        s = s[1:]
    if s.endswith("|"):
        s = s[:-1]
    return s.split("|")


def _e_separador(linha: str) -> bool:
    cels = _celulas(linha)
    return bool(cels) and all(_SEP_RE.match(c) for c in cels)


def _achar_tabela(texto: str, cabecalho: tuple[str, ...]) -> tuple[int, int] | None:
    """(índice da linha do cabeçalho, índice da linha DEPOIS da última linha da tabela), em
    `texto.splitlines()`. Só a primeira tabela cujo cabeçalho casa."""
    alvo = [normalizar(c) for c in cabecalho]
    linhas = _sem_fences(texto).splitlines()
    for i, ln in enumerate(linhas):
        if not ln.lstrip().startswith("|"):
            continue
        if [normalizar(c) for c in _celulas(ln)] != alvo:
            continue
        fim = i + 1
        while fim < len(linhas) and linhas[fim].lstrip().startswith("|"):
            fim += 1
        return i, fim
    return None


def ler_tabela(texto: str, cabecalho: tuple[str, ...]) -> list[dict[str, str]]:
    pos = _achar_tabela(texto, cabecalho)
    if pos is None:
        return []
    linhas = texto.splitlines()
    out: list[dict[str, str]] = []
    for ln in linhas[pos[0] + 1:pos[1]]:
        if _e_separador(ln):
            continue
        cels = [limpar(c) for c in _celulas(ln)]
        cels += [""] * (len(cabecalho) - len(cels))
        out.append(dict(zip(cabecalho, cels)))
    return out


def _render(cabecalho: tuple[str, ...], valores: dict[str, str]) -> str:
    return "| " + " | ".join(validar_celula(valores.get(c, "")) or "-" for c in cabecalho) + " |"


def trocar_linha(texto: str, cabecalho: tuple[str, ...], chave: str | tuple[str, ...],
                 valores: dict[str, str], titulo_se_ausente: str) -> str:
    """Troca no lugar a linha cujas PRIMEIRAS colunas casam `chave` (normalizada); ausente, entra no
    fim da tabela; tabela ausente, nasce sob `## <titulo_se_ausente>` no fim do arquivo.

    `chave` como tupla casa as N primeiras colunas em vez de só a primeira — é o que permite um
    mesmo papel ocupar várias linhas (o rodízio de contas), sem que gravar a segunda sobrescreva
    a primeira."""
    nova = _render(cabecalho, valores)
    linhas = texto.splitlines()
    pos = _achar_tabela(texto, cabecalho)
    if pos is None:
        bloco = [f"## {titulo_se_ausente}", "",
                 "| " + " | ".join(cabecalho) + " |",
                 "|" + "---|" * len(cabecalho), nova]
        base = texto.rstrip("\n")
        return (base + "\n\n" if base else "") + "\n".join(bloco) + "\n"
    ini, fim = pos
    alvo = [normalizar(c) for c in ((chave,) if isinstance(chave, str) else chave)]
    for i in range(ini + 1, fim):
        if _e_separador(linhas[i]):
            continue
        cels = [normalizar(c) for c in _celulas(linhas[i])]
        if len(cels) >= len(alvo) and cels[:len(alvo)] == alvo:
            linhas[i] = nova
            break
    else:
        linhas.insert(fim, nova)
    return "\n".join(linhas) + ("\n" if texto.endswith("\n") or not texto else "")


def remover_linha(texto: str, cabecalho: tuple[str, ...], chave: str | tuple[str, ...]) -> str:
    """Tira UMA linha. `chave` como tupla casa as N primeiras colunas — mesmo motivo do
    `trocar_linha`: num papel que reveza, remover pelo nome sozinho apagaria a linha errada."""
    pos = _achar_tabela(texto, cabecalho)
    if pos is None:
        return texto
    linhas = texto.splitlines()
    alvo = [normalizar(c) for c in ((chave,) if isinstance(chave, str) else chave)]
    for i in range(pos[0] + 1, pos[1]):
        if _e_separador(linhas[i]):
            continue
        cels = [normalizar(c) for c in _celulas(linhas[i])]
        if len(cels) >= len(alvo) and cels[:len(alvo)] == alvo:
            del linhas[i]
            break
    return "\n".join(linhas) + ("\n" if texto.endswith("\n") else "")


def inserir_coluna(texto: str, cabecalho: tuple[str, ...], nome: str, em: int, valor: str = "-") -> str:
    """Acrescenta uma coluna à tabela, NO LUGAR: reescreve o cabeçalho, o separador e cada linha
    onde já estavam.

    Foi tentado antes apagar a tabela e deixar `trocar_linha` recriá-la: o título `## Quem é quem`
    ficava órfão (só as linhas com `|` saíam) e a tabela nova nascia no FIM do arquivo, depois de
    seções que a pessoa escreveu à mão — arquivo com o título duplicado e a tabela fora de lugar.
    Converter no lugar não mexe em mais nada do documento."""
    pos = _achar_tabela(texto, cabecalho)
    if pos is None:
        return texto
    linhas = texto.splitlines()
    novo_cab = list(cabecalho[:em]) + [nome] + list(cabecalho[em:])
    for i in range(pos[0], pos[1]):
        ln = linhas[i]
        if i == pos[0]:
            linhas[i] = "| " + " | ".join(novo_cab) + " |"
        elif _e_separador(ln):
            linhas[i] = "|" + "---|" * len(novo_cab)
        else:
            cels = [limpar(c) for c in _celulas(ln)]
            cels += [""] * (len(cabecalho) - len(cels))
            cels = cels[:em] + [valor] + cels[em:]
            linhas[i] = "| " + " | ".join(c or "-" for c in cels) + " |"
    return "\n".join(linhas) + ("\n" if texto.endswith("\n") else "")


def trocar_secao(texto: str, titulo: str, corpo: str) -> str:
    """Substitui o corpo sob `## <titulo>` (até o próximo `## `); ausente, anexa no fim."""
    linhas = texto.splitlines()
    alvo = normalizar(titulo)
    ini = next((i for i, ln in enumerate(linhas)
                if ln.startswith("## ") and normalizar(ln[3:]) == alvo), None)
    novo = [f"## {titulo}", ""] + corpo.rstrip("\n").splitlines() + [""]
    if ini is None:
        base = texto.rstrip("\n")
        return (base + "\n\n" if base else "") + "\n".join(novo).rstrip("\n") + "\n"
    fim = ini + 1
    while fim < len(linhas) and not linhas[fim].startswith("## "):
        fim += 1
    out = linhas[:ini] + novo + linhas[fim:]
    return "\n".join(out).rstrip("\n") + "\n"


def ler_arquivo(path: Path) -> tuple[str, float]:
    """(texto, mtime). Ausente = ('', 0.0) — o chamador decide se cria."""
    try:
        return path.read_text(encoding="utf-8"), path.stat().st_mtime
    except FileNotFoundError:
        return "", 0.0


class Conflito(Exception):
    """O arquivo mudou entre a leitura e a escrita (o árbitro edita o mesmo arquivo com Edit)."""


# ponytail: trava única do processo — conferir o mtime e gravar têm de ser um ato só, senão dois
# pedidos (celular + desktop) que leram o mesmo mtime passam os dois e o segundo sobrescreve calado.
_TRAVA = threading.Lock()


def gravar(path: Path, texto: str, mtime_lido: float | None = None) -> float:
    """tmp+rename com o pid no nome (dois pedidos simultâneos com nome fixo entrelaçam bytes).
    `mtime_lido` fornecido e diferente do atual → `Conflito`, sem tocar no arquivo."""
    with _TRAVA:
        if mtime_lido is not None:
            try:
                atual = path.stat().st_mtime
            except FileNotFoundError:
                atual = 0.0
            if abs(atual - mtime_lido) > 1e-6:
                raise Conflito(str(path))
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
        tmp.write_text(texto, encoding="utf-8")
        atomico.substituir(tmp, path)
        return path.stat().st_mtime
