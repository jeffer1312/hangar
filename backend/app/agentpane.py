"""Descobre em QUAL pane de uma sessao o agente roda.

O alvo antigo (`={nome}:`, tmux.py:85) aponta pro pane ATIVO. Basta o usuario abrir uma janela nova
(`Ctrl-b c`, ou o botao `+` do painel) pra que send-keys e capture-pane passem a falar com o shell:
a mensagem do app e digitada no bash e o Enter que o send_prompt manda junto a EXECUTA.

Por que nao reusar registry.provider_of_pane: ela devolve "claude" por PADRAO pra pane nao
reconhecido (registry.py:240-242), de proposito — nao serve como predicado, porque um pane de shell
tambem responderia "claude". Aqui a resposta e estrita.
"""
import logging
import os
import time
from typing import Optional

from app import tmux
from app.procinfo import _cmdline, _descendant_pids, _proc_children_map

_log = logging.getLogger(__name__)

# TTL longo de proposito: o que se guarda aqui e QUAL pane roda o agente, e isso nao muda quando o
# usuario abre janela nova — muda so se o agente morrer. E _pane_target roda em TODO send-keys e
# TODO capture-pane (por poll, por sessao): um fork por chamada seria inaceitavel.
_TTL = 60.0
_cache: dict[str, tuple[Optional[str], float]] = {}
_AVISADAS: set[str] = set()


def invalidate(name: Optional[str] = None) -> None:
    if name is None:
        _cache.clear()
        _AVISADAS.clear()
    else:
        _cache.pop(name, None)
        _AVISADAS.discard(name)


def _pane_do_agente(pid: int, children: dict[int, list[int]]) -> bool:
    # Mesma descida do registry.provider_of_pane, com as mesmas exclusoes, mas devolvendo NAO em vez
    # de "claude" quando nada casa. `codex` entra na lista porque _EXEC_PROVIDER so tem pi/claude
    # (registry.py:231) — sem ele, TODA sessao Codex com 2+ panes cairia no aviso abaixo.
    from app.registry import _EXEC_PROVIDER   # tardio: registry importa tmux, que importa isto
    conhecidos = set(_EXEC_PROVIDER) | {"codex"}
    for p in _descendant_pids(pid, children):
        cmd = _cmdline(p)
        if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
            continue
        argv0 = cmd.strip().split()[:1]
        if argv0 and os.path.basename(argv0[0]) in conhecidos:
            return True
    return False


def resolve_target(name: str) -> Optional[str]:
    """`"%3"` = o pane do agente. `None` = nao sei, e o chamador usa o alvo antigo."""
    agora = time.monotonic()
    achado = _cache.get(name)
    if achado and agora - achado[1] < _TTL:
        return achado[0]

    panes = tmux.list_panes_of(name)
    alvo: Optional[str] = None
    if len(panes) == 1:
        # Caso normal (todas as sessoes desta maquina hoje): um pane so, `=nome:` e `%N` apontam pro
        # MESMO lugar -> devolver `%N` aqui nao ganha nada e faz a mudanca valer pra 100% das sessoes
        # em vez de so as que tem janela extra. O preco: no Windows o app roda sobre psmux, cuja
        # compatibilidade e MEDIDA (scripts/test-psmux.py) contra alvo `=NOME:` exato — `%N` nao esta
        # medido ali. Se o psmux nao resolver `%N` do jeito esperado, todo send-keys/capture-pane do
        # app quebra no caminho NORMAL (1 pane), nao so no caso raro de janela extra. Raio de
        # explosao, nao desconhecimento (achado 2 da revisao, rodada 1).
        alvo = None
    elif panes:
        children = _proc_children_map()
        for p in panes:
            if _pane_do_agente(p["pid"], children):
                alvo = p["pane_id"]
                break
        if alvo is None and name not in _AVISADAS:
            # Falha aparece, nao some — mas UMA vez por sessao ate mudar (politica do api.py:237-244):
            # com TTL de 60s, um log por resolucao viraria enxurrada.
            _AVISADAS.add(name)
            _log.warning("agentpane: %r tem %d panes e nenhum parece do agente; alvo volta a ser "
                         "a janela ativa", name, len(panes))
    _cache[name] = (alvo, agora)
    return alvo
