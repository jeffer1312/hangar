"""Poda periodica de sidecars de sessao morta.

O app espalha sidecars por sessao em `<config>/.claude-pocket-*`: statusline, estado,
preview, fila, bilhetes pane->sessao e catalogo de modelos do Pi. Nada disso sumia quando a
sessao morria (medido 18/08/2026: status 183, state 178, preview 142, fila 139 entradas,
pi 5,7 MB — e o backend ativo acumula mais; a mais antiga de 01/07/2026).

Por que nao e so arrumacao: sidecar de sessao morta MENTE. Dois arquivos velhos com nome de
pane reaproveitado reportavam um executor rodando outro modelo com custo 4,5x maior — o
perigo e ler dado velho achando que e o de agora.

Criterio CONSERVADOR e escrito (nada de "limpa tudo ao subir"):

1. A chave e o session-id (session_key() do jsonl), o nome da sessao (fila) ou o pane_id
   (bilhete) — NUNCA o nome do pane como identidade de conteudo. Sidecar cuja chave pertence
   a uma sessao viva NUNCA e apagado, por mais velho que seja.
2. Idade minima de _MIN_AGE: sessao morta RECENTE preserva os sidecars de proposito — foi
   lendo sidecar de execucao morta que o achado do custo errado foi descoberto, e as leituras
   ja tem tetos proprios (statusline 1 dia, preview 10 min) que impedem dado velho de mentir
   na tela. Depois de _MIN_AGE, sessao inexistente = lixo.

Onde roda: no lifespan do backend (app/api.py), primeira varredura no boot e depois a cada
_INTERVALO. O registry e polled (nao tem evento de "sessao sumiu"), entao a poda no registry
deixaria lixo de sessao que morreu fora da vista dele; e so no startup deixaria o backend de
longa duracao (unit systemd do usuario) acumulando para sempre. Periodica cobre os dois.
"""

import logging
import time
from pathlib import Path

from app import tmux
from app.config import _backend_config_base, list_config_dirs

_log = logging.getLogger("claude_pocket.prune")

# Idade minima (s) de um sidecar orfao para entrar na poda. 7 dias: preserva a materia-prima
# de diagnostico de execucao morta recente (o caso t1/t2 da Task foi achado lendo sidecar de
# sessao morta) sem deixar o lixo acumular.
_MIN_AGE = 7 * 86400.0
# Intervalo (s) entre varreduras periodicas: 1x/dia basta — a leitura ja recusa sidecar velho
# (teto de 1 dia na statusline, 10 min na preview), entao poda e higiene, nao seguranca.
_INTERVALO = 24 * 3600.0

# Keyed pela chave de sessao (session_key do jsonl): statusline, estado, preview, askq e o
# catalogo de modelos do Pi (subdir de .claude-pocket-pi).
_STEM_KEYED = (".claude-pocket-status", ".claude-pocket-state", ".claude-pocket-preview",
               ".claude-pocket-askq", ".claude-pocket-pi/models")
# Keyed pelo NOME da sessao (sanitizado): a fila sobrevive ao /clear de proposito (o
# session-id muda no /clear), entao o nome e a chave certa dela.
_NOME_KEYED = ".claude-pocket-queue"
# Keyed pelo pane_id (bilhete pane->sessao do Pi e do Kimi). O tmux REUSA %pane_id, entao o
# bilhete nunca decide quem e a sessao (isso e o frescor de pi_session_file) — a poda so tira
# bilhete de pane que NAO existe mais; pane vivo nunca tem bilhete podado.
_PANE_KEYED = (".claude-pocket-pi", ".claude-pocket-kimi")


def _config_bases() -> list[Path]:
    # Mesmas bases dos outros leitores (statusline/hook_state): todos os config dirs das
    # contas + o base do backend.
    try:
        return list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    except OSError:
        return [_backend_config_base()]


def _pane_ids_vivos() -> set[str]:
    out: set[str] = set()
    try:
        for panes in tmux.list_panes_all().values():
            for p in panes:
                pid = p.get("pane_id") or ""
                if pid:
                    out.add(pid.lstrip("%"))
    except Exception:  # noqa: BLE001 — tmux fora: nao poda pane-keyed, nunca derruba a poda
        _log.warning("prune: sem tmux, bilhetes pane->sessao ficam", exc_info=True)
    return out


def _podar_dir(d: Path, chaves_vivas: set[str], agora: float, pattern: str = "*.json") -> int:
    """Apaga de UM diretorio os arquivos cuja chave nao esta viva E que passaram de _MIN_AGE.

    Falha-soft: arquivo que some no meio (sessao encerrando) ou sem stat nao derruba a
    varredura. O .tmp meio-escrito nunca casa glob (suffix diferente).
    """
    if not d.is_dir():
        return 0
    n = 0
    try:
        for f in d.glob(pattern):
            try:
                if f.is_file() and f.stem not in chaves_vivas and agora - f.stat().st_mtime >= _MIN_AGE:
                    f.unlink()
                    n += 1
            except OSError:
                continue
    except OSError:
        pass
    return n


def _podar(bases: list[Path], chaves_stem: set[str], chaves_nome: set[str],
           chaves_pane: set[str], agora: float) -> dict[str, int]:
    """Varre as bases e devolve {subdir: quantos apagou}. Separada de prune_sidecars para o
    teste rodar com tmp_path, sem tocar nos config dirs reais do usuario."""
    apagados: dict[str, int] = {}
    for base in bases:
        for sub in _STEM_KEYED:
            apagados[sub] = apagados.get(sub, 0) + _podar_dir(base / sub, chaves_stem, agora)
        apagados[_NOME_KEYED] = (apagados.get(_NOME_KEYED, 0)
                                 + _podar_dir(base / _NOME_KEYED, chaves_nome, agora, "*.jsonl"))
        for sub in _PANE_KEYED:
            apagados[sub] = (apagados.get(sub, 0)
                             + _podar_dir(base / sub, chaves_pane, agora))
    return apagados


def prune_sidecars(infos=None, agora: float | None = None,
                   bases: list[Path] | None = None) -> dict[str, int]:
    """Poda de sidecars orfaos em todos os config dirs. Devolve {subdir: apagados}.

    `infos`: snapshot de SessionInfo (registry.list()) — sem ele, resolve ao vivo.
    `agora`/`bases`: injecao de teste. Com bases injetadas, NUNCA toca o disco do usuario.
    """
    if infos is None:
        # Import local: registry importa pqueue/adapters no boot; aqui o prune so precisa da
        # listagem, e o import local evita qualquer ciclo futuro (padrao do resto do app).
        from app.registry import SessionRegistry

        infos = SessionRegistry().list()
    if agora is None:
        agora = time.time()
    if bases is None:
        bases = _config_bases()

    from app.models import session_key
    from app.pqueue import _sanitize

    chaves_stem = {session_key(i.jsonl) for i in infos if i.jsonl}
    chaves_nome = {_sanitize(i.name) for i in infos}
    chaves_pane = _pane_ids_vivos()
    apagados = _podar(bases, chaves_stem, chaves_nome, chaves_pane, agora)
    for sub, n in apagados.items():
        if n:
            _log.info("prune: %d sidecar(s) de sessao morta removidos em %s", n, sub)
    return apagados


async def prune_loop() -> None:
    """Loop periodico do lifespan: varre na subida e depois a cada _INTERVALO.

    O primeiro disparo e imediato (backend que reinicia todo dia — o caso comum — nao espera
    24h); o intervalo cobre o backend de longa duracao. A varredura roda no thread (registry
    faz fork de tmux + /proc). Falha de UMA rodada nao mata o loop: loga e segue.
    """
    import asyncio

    while True:
        try:
            await asyncio.to_thread(prune_sidecars)
        except Exception:  # noqa: BLE001
            _log.exception("prune: varredura falhou; proxima em %ds", _INTERVALO)
        await asyncio.sleep(_INTERVALO)
