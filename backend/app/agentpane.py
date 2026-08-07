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
_ERROS_AVISADOS: set[str] = set()   # falha inesperada do pane_info; separado do de cima (ver la)


def invalidate(name: Optional[str] = None) -> None:
    if name is None:
        _cache.clear()
        _AVISADAS.clear()
        _ERROS_AVISADOS.clear()
    else:
        _cache.pop(name, None)
        _AVISADAS.discard(name)
        _ERROS_AVISADOS.discard(name)


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


def pane_info(name: str) -> tuple[str, Optional[str]]:
    """(provider, pane_id) do pane do AGENTE de `name`. Erro/sessao sumida -> ("claude", None).

    UMA resolucao pra todo caminho de ENVIO -- o /input (api._pane_info), o drain da fila duravel
    (terminal_input.drain) e o adapter do Pi. Antes os dois ultimos liam `list_panes_active()`, o
    pane ATIVO: numa sessao Pi com split manual (ou com o shell escondido da Task 6 na frente) eles
    pegavam o `pane_id` do SHELL, o `INBOX.tem_linha()` falhava e a linha rapida do Pi se perdia --
    o mesmo bug que a Task 4 matou no /input, deixado vivo nos irmaos (achado I1 da revisao final).

    Sem cache de proposito, ao contrario do `resolve_target`: aqui o pane_id vira BILHETE (a
    extensao do Pi so aceita o pane certo) e o caminho e de ENVIO, nao de poll -- um valor de 60s
    atras entregaria a mensagem no lugar errado, que e exatamente o que se esta consertando.
    """
    from app import registry as registry_mod   # tardio: registry importa tmux, que importa isto
    try:
        panes = tmux.list_panes_all().get(name)
        if not panes:
            return "claude", None
        children = _proc_children_map()
        p = registry_mod.SessionRegistry._agent_pane(panes, children)
        return registry_mod.provider_of_pane(p["pid"], children), p.get("pane_id")
    except Exception as e:                       # noqa: BLE001
        # "Nao achei pane de agente" e resposta normal e segue silenciosa (e o `if not panes` acima).
        # Aqui embaixo e OUTRA coisa: AttributeError/KeyError de verdade no _agent_pane, no
        # provider_of_pane ou no _proc_children_map. O catch-all fundia os dois e devolvia
        # ("claude", None) calado — e como esta funcao virou a resolucao dos TRES caminhos de envio
        # (/input, drain da fila, adapter do Pi), o silencio triplicou. Mesma politica de avisar
        # UMA vez por sessao do `resolve_target` abaixo: com este caminho rodando a cada envio, um
        # log por chamada viraria enxurrada.
        # Conjunto PROPRIO, nao o `_AVISADAS` do resolve_target: compartilhar faria um aviso calar o
        # outro (a sessao que ja avisou "nenhum pane parece do agente" nunca logaria a falha real).
        if name not in _ERROS_AVISADOS:
            _ERROS_AVISADOS.add(name)
            _log.warning("agentpane: falha inesperada ao resolver o pane de %r (%s: %s); alvo volta "
                         "a ser a janela ativa", name, type(e).__name__, e)
        return "claude", None


def resolve_target(name: str) -> Optional[str]:
    """`"%3"` = o pane do agente. `None` = nao sei, e o chamador usa o alvo antigo."""
    agora = time.monotonic()
    achado = _cache.get(name)
    if achado and agora - achado[1] < _TTL:
        return achado[0]

    # Ordena com o pane ATIVO na frente: o desempate tem que ser o MESMO do
    # registry._agent_pane (achado I2 da revisao final). Com dois panes de agente na mesma sessao,
    # um pegando "o primeiro da varredura" e o outro "o ativo" resolvia provider/pane_id por um
    # pane e digitava no outro -- e o cache de 60s daqui, contra o _agent_pane sem cache, ainda
    # esticava a janela em que os dois discordavam. Ativo primeiro = o comportamento de antes da
    # Task 1 pra esse caso.
    panes = sorted(tmux.list_panes_of(name), key=lambda p: not p.get("active"))
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
