"""Tabela de papéis do contrato do grupo (`regras-<gid>.md`, seção `## Quem é quem`).

O árbitro é dono do resto do arquivo; o app troca só a linha do papel (orq_md). `sessão` aceita
`*` no fim (`trab-t*`: uma sessão por Task) — casa a viva mais recente.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass
from pathlib import Path

from . import orq_md, pair

_log = logging.getLogger(__name__)

# Dois formatos da tabela, e a leitura aceita os dois. O de 6 colunas é o original (uma linha por
# papel); o de 7 acrescenta `vez`, que é o que deixa um papel ocupar VÁRIAS linhas — o rodízio de
# contas. A coluna só aparece no arquivo quando algum papel de fato reveza: contrato sem rodízio
# continua byte a byte como estava, o que importa porque estes arquivos são de trabalhos em curso.
CABECALHO = ("papel", "sessão", "provider", "conta", "modelo", "esforço")
CABECALHO_VEZ = ("papel", "vez", "sessão", "provider", "conta", "modelo", "esforço")
SECAO = "Quem é quem"
ARBITRO = "arbitro"
# `vez` só assume "1", "2", "3"…: a Task N cabe à conta de índice (N-1) % total, então a ordem é a
# própria ordem das linhas e não há estado a guardar. Não existe valor para "rodar em paralelo" —
# Tasks em paralelo são outra coisa (uma worktree por Task, cada uma com seu executor e seu
# revisor) e se declaram no PLANO, não aqui: skills/.../references/paralelo-worktree.md.


@dataclass(frozen=True)
class Papel:
    papel: str
    sessao: str
    provider: str
    conta: str
    modelo: str
    esforco: str
    vez: str = ""

    def e_arbitro(self) -> bool:
        return orq_md.normalizar(self.papel) == ARBITRO


GID_PADRAO = "padrao"   # regras-padrao.md: o time que o árbitro copia ao montar um grupo novo


def regras_path(gid: str) -> Path:
    return pair._pair_dir() / f"regras-{gid}.md"


def tem_coluna_vez(texto: str) -> bool:
    """O arquivo já está no formato de 7 colunas? Decide entre reescrever no lugar e promover."""
    return bool(orq_md.ler_tabela(texto, CABECALHO_VEZ))


def ler(texto: str) -> list[Papel]:
    """Lê a tabela nos dois formatos. Tenta o de 7 colunas primeiro: se o arquivo já tem `vez`, o
    cabeçalho de 6 não casa (a comparação é exata) e a leitura devolveria vazio — que na tela é
    'este grupo não tem papel nenhum', o pior erro possível aqui."""
    for cab in (CABECALHO_VEZ, CABECALHO):
        linhas = orq_md.ler_tabela(texto, cab)
        if linhas:
            return [Papel(r["papel"], r["sessão"], r["provider"].lower(), r["conta"],
                          r["modelo"], r["esforço"], r.get("vez", ""))
                    for r in linhas if r.get("papel")]
    return []


def promover(texto: str) -> str:
    """Acrescenta a coluna `vez` à tabela, no lugar, com `-` em quem já estava. Só é chamada quando
    um papel passa a revezar: um contrato que nunca usou rodízio nunca é tocado.

    A conversão é NO LUGAR de propósito — nada além da tabela muda de posição, e as outras seções
    do contrato (`## Gates`, `## Réguas`, escritas à mão) ficam onde estavam."""
    if tem_coluna_vez(texto):
        return texto
    return orq_md.inserir_coluna(texto, CABECALHO, "vez", 1)


def _valores(p: Papel, com_vez: bool) -> dict[str, str]:
    v = {"papel": p.papel, "sessão": p.sessao, "provider": p.provider,
         "conta": p.conta, "modelo": p.modelo, "esforço": p.esforco}
    if com_vez:
        v["vez"] = p.vez
    return v


def _escrever_vez(texto: str, p: Papel) -> str:
    # Chave composta (papel, vez): sem ela, gravar a 2ª conta de um papel sobrescreveria a 1ª.
    return orq_md.trocar_linha(texto, CABECALHO_VEZ, (p.papel, p.vez or "-"),
                               _valores(p, True), SECAO)


def escrever_papel(texto: str, p: Papel) -> str:
    for v in (p.papel, p.sessao, p.provider, p.conta, p.modelo, p.esforco, p.vez):
        orq_md.validar_celula(v)
    # Papel sem `vez` num arquivo que ainda não tem a coluna: nada muda de formato.
    if not p.vez and not tem_coluna_vez(texto):
        return orq_md.trocar_linha(texto, CABECALHO, p.papel, _valores(p, False), SECAO)
    return _escrever_vez(promover(texto), p)


def _casa_nome(padrao: str, nome: str) -> bool:
    padrao = padrao.strip()
    if not padrao:
        return False
    return nome.startswith(padrao[:-1]) if padrao.endswith("*") else nome == padrao


def gid_por_sessao(nome: str) -> str | None:
    """Grupo de uma sessão SEM sidecar de pareamento: o contrato já diz quem está nele — a coluna
    `sessão` da tabela. Um grupo tocado fora do `--pair` (já aconteceu num trabalho real) continua visível.
    Mais de um contrato casando → o mais recente."""
    achados: list[tuple[float, str]] = []
    for p in pair._pair_dir().glob("regras-*.md"):
        if p.stem == f"regras-{GID_PADRAO}":
            continue
        try:
            texto, mtime = orq_md.ler_arquivo(p)
        except (OSError, ValueError) as e:
            # Um contrato ilegível (encoding quebrado por edição à mão) não pode tirar a tela de
            # TODAS as sessões — pula este e diz qual foi.
            _log.warning("regras ilegível, ignorado: %s (%s)", p, e)
            continue
        if any(_casa_nome(r.sessao, nome) for r in ler(texto)):
            achados.append((mtime, p.stem.removeprefix("regras-")))
    return max(achados)[1] if achados else None


def casar_viva(papel: Papel, sessoes) -> str | None:
    """Nome da sessão viva deste papel: exato, ou o glob `x*` → a mais recente por last_activity."""
    alvo = papel.sessao.strip()
    if not alvo:
        return None
    if alvo.endswith("*"):
        pref = alvo[:-1]
        cands = [s for s in sessoes if s.name.startswith(pref)]
        if not cands:
            return None
        return max(cands, key=lambda s: s.last_activity or 0).name
    return next((s.name for s in sessoes if s.name == alvo), None)
