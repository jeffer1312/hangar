"""Leitura e troca do modo de permissão do Claude Code via rodapé do pane.

Mecanismo medido em 2026-08-20 (docs/superpowers/specs/2026-08-19-medicao-permissao-viva.md):
- stdin da statusline NÃO traz o modo; sidecar também não.
- /permissions não aceita argumento.
- Shift+Tab (BTab) cicla os modos, com confirmação legível no rodapé.
- Ciclo tem 4 posições numa sessão comum, 5 se nasceu em bypassPermissions, dontAsk só no arranque.

Este módulo é stdlib + tmux, no padrão de model_picker.py.
"""

import re
import time

from app import tmux

# Regex que casa a frase do rodapé, sem depender do glifo. O glifo (⏸/⏵⏵) é usado só pra
# priorizar linhas de rodapé e evitar falso positivo com texto da conversa que cite o modo.
_FOOTER_RE = re.compile(
    r"(plan mode on|auto mode on|manual mode on|accept edits on|bypass permissions on|don['’]t ask on)",
    re.IGNORECASE,
)

# Normaliza a frase casada para a chave do dict (minúscula, apóstrofe reto, espaço único).
def _norm(s: str) -> str:
    s = s.lower().replace("’", "'")
    s = re.sub(r"\s+", " ", s).strip()
    return s

_CANON = {
    "plan mode on": "plan",
    "auto mode on": "auto",
    "manual mode on": "manual",
    "accept edits on": "acceptEdits",
    "bypass permissions on": "bypassPermissions",
    "don't ask on": "dontAsk",
}

# Ordem canônica dos modos como aparecem no help (não é ordem do ciclo; ciclo é descoberto ao vivo).
ORDEM_CANONICA = ("plan", "auto", "manual", "acceptEdits", "bypassPermissions", "dontAsk")


def parse_permission_mode(pane: str) -> str | None:
    """Devolve o id canônico do modo lido no rodapé, ou None se não achou.

    Casa a ÚLTIMA linha que contém o glifo e a frase (⏸/⏵⏵ + modo). Se nenhuma linha
    com glifo casar, faz segunda passada sem filtro de glifo (captura truncada).
    Puro, testável com fixture de texto.
    """
    lines = pane.splitlines()
    # primeira passada: exige glifo (evita casar texto da conversa)
    for line in reversed(lines):
        if "⏸" not in line and "⏵" not in line:
            continue
        m = _FOOTER_RE.search(line)
        if m:
            canon = _CANON.get(_norm(m.group(1)))
            if canon:
                return canon
    # segunda passada: sem filtro de glifo (pane estreito que cortou o glifo)
    for line in reversed(lines):
        m = _FOOTER_RE.search(line)
        if m:
            canon = _CANON.get(_norm(m.group(1)))
            if canon:
                return canon
    return None


def ler_modo(name: str) -> str | None:
    """Captura o pane e devolve o modo atual, ou None se ilegível."""
    pane = tmux.capture_pane(name)
    return parse_permission_mode(pane)


# Teto de teclas por troca e espera por tecla (regras do grupo).
TETO_TECLAS = 6
ESPERA_POR_TECLA = 2.0
INTERVALO_POLL = 0.2
SETTLE_ANTES = 0.3


def _espera_modo(name: str, anterior: str | None = None, timeout: float = ESPERA_POR_TECLA) -> str | None:
    """Espera até o rodapé refletir um modo (diferente do anterior, se dado) ou timeout."""
    fim = time.monotonic() + timeout
    ultimo: str | None = None
    while time.monotonic() < fim:
        time.sleep(INTERVALO_POLL)
        cur = ler_modo(name)
        if cur is None:
            continue
        # se anterior é None, qualquer modo vale; se anterior dado, espera mudar
        if anterior is None or cur != anterior:
            return cur
        ultimo = cur
    # timeout: devolve o último lido (pode ser igual ao anterior) ou None
    if ultimo is not None:
        return ultimo
    return ler_modo(name)


def trocar_modo(name: str, alvo: str) -> str:
    """Tenta levar a sessão `name` para `alvo` via BTab, até TETO_TECLAS.

    Devolve SEMPRE o modo que FICOU (nunca o pedido). Quem chama decide o status HTTP:
    se ficou != alvo, é 409 (teto ou alvo fora do ciclo).
    Levanta ValueError se alvo fora da lista fechada.
    """
    from app import model_args

    if alvo not in model_args.MODOS_PERMISSAO_CLAUDE:
        raise ValueError(f"permission_mode: use um de {', '.join(model_args.MODOS_PERMISSAO_CLAUDE)}")

    # lock por sessão: ciclo de BTab não pode intercalar com outro ciclo
    from app.terminal_input import _send_lock

    with _send_lock(name):
        cur = ler_modo(name)
        if cur is None:
            # pane ilegível: não dá pra decidir, devolve None e deixa o caller transformar em 409
            raise RuntimeError("não consegui ler o modo atual no rodapé")
        if cur == alvo:
            return cur
        # tenta até teto
        for _ in range(TETO_TECLAS):
            # manda BTab
            tmux.send_keys(name, "BTab")
            # espera o rodapé refletir (até 2s)
            time.sleep(SETTLE_ANTES)
            novo = _espera_modo(name, anterior=cur, timeout=ESPERA_POR_TECLA)
            if novo is None:
                # não leu nada: tenta reler uma vez sem comparar com anterior
                novo = ler_modo(name)
                if novo is None:
                    # pane ilegível no meio: para e devolve o que ficou (cur)
                    break
            cur = novo
            if cur == alvo:
                return cur
        # estourou ou alvo fora do ciclo: devolve o que ficou
        return cur


def listar_modos(name: str) -> tuple[str, list[str]]:
    """Descobre o ciclo ao vivo dando a volta completa de BTab.

    Devolve (ficou, modos) onde `ficou` é o modo em que a sessão FICOU após a
    descoberta (igual ao atual se conseguiu voltar, ou o último do ciclo) e `modos`
    é a lista de modos que aparecem no ciclo. Volta ao modo original se possível;
    se original é dontAsk, não sonda — dontAsk não tem volta (spec 5b).
    Levanta RuntimeError se não conseguir ler o modo inicial.
    """
    from app.terminal_input import _send_lock

    with _send_lock(name):
        orig = ler_modo(name)
        if orig is None:
            raise RuntimeError("não consegui ler o modo atual no rodapé")
        # dontAsk não tem volta: não sondar nunca ali (bloqueador 2)
        if orig == "dontAsk":
            return orig, []

        # ciclo começa com o original incluído, para preservar ordem natural
        vistos: list[str] = [orig]
        vistos_set: set[str] = {orig}
        cur = orig
        # até 6 BTab (ciclo máximo 5 + 1 folga)
        for _ in range(6):
            tmux.send_keys(name, "BTab")
            time.sleep(SETTLE_ANTES)
            novo = _espera_modo(name, anterior=cur, timeout=ESPERA_POR_TECLA)
            if novo is None:
                novo = ler_modo(name)
                if novo is None:
                    break
            cur = novo
            if cur == orig:
                # voltou ao original: ciclo fechou, sem duplicar
                break
            if cur in vistos_set:
                # sub-ciclo sem voltar ao orig — ciclo de 4 fechou em repetição
                break
            vistos.append(cur)
            vistos_set.add(cur)

        # se cur != orig, tenta voltar ao original (pane ilegível no meio etc.)
        if cur != orig and orig in vistos_set:
            for _ in range(TETO_TECLAS + 1):
                tmux.send_keys(name, "BTab")
                time.sleep(SETTLE_ANTES)
                novo = _espera_modo(name, anterior=cur, timeout=ESPERA_POR_TECLA)
                if novo is None:
                    novo = ler_modo(name)
                    if novo is None:
                        break
                cur = novo
                if cur == orig:
                    break
        # bloqueador 3: devolver o que FICOU, não o de antes
        return cur, vistos
