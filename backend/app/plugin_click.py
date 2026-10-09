"""Clique de botão de mod pedido pelo app: acha o botão na tela do pane e clica pelo mouse do terminal.

Nenhuma API do engine deixa um plugin disparar o botão de outro (o `onPress` mora no ambiente do mod).
O clique entra como clique de mouse SGR no pane, na célula do rótulo, e o plugin do Hangar confirma
pelo `ui.press` que o press chegou ao botão certo."""
import asyncio
import logging
import os
import time

from app import plugin_bridge, tmux
from app.mensagens import erro
from app.plugin_screen import anchor_row, band_start, find_label, prompt_top
from app.state import run_tmux

BAND_SITE = "above-prompt"
# A `key` com que o app de antes da rota `close` fecha o painel pelo `press`.
CLOSE_KEY = "__close__"
CLOSE_LABEL = "✕"
CONFIRM_S = 2.0
# O `onPress` do mod costuma copiar ou abrir sem `await`: o efeito pode chegar logo depois do press.
EFFECT_S = 0.3
_locks: dict[str, asyncio.Lock] = {}
_log = logging.getLogger("hangar.plugin_click")


class PressRefused(Exception):
    def __init__(self, code: str, msg: str, **params):
        super().__init__(msg)
        self.detail = erro(code, msg, **params)


def button_label(tree, key: str, plugin: str | None) -> str | None:
    """O rótulo do `Button` de `key` do mod `plugin`: `label`, ou o texto dos filhos.

    Sem o mod (app de antes desta versão), vale o único `Button` com a `key`: em mais de um mod, não há
    como saber de qual é, e nenhum é acionado, como o Rust."""
    rotulos = []
    pilha = [tree]
    while pilha:
        no = pilha.pop()
        if not isinstance(no, dict):
            continue
        props = no.get("props") or {}
        if (no.get("type") == "Button" and props.get("key") == key
                and (plugin is None or (no.get("press") or {}).get("plugin") == plugin)):
            texto = props.get("label") or "".join(c for c in no.get("children") or [] if isinstance(c, str))
            if plugin is not None:
                return texto.strip() or None
            rotulos.append(texto.strip() or None)
            continue
        pilha.extend(no.get("children") or [])
    return rotulos[0] if len(rotulos) == 1 else None


def _tmux_format(name: str, fmt: str) -> str:
    cp = tmux._run(["tmux", "display-message", "-p", "-t", tmux._pane_target(name), fmt])
    return cp.stdout.strip() if cp.returncode == 0 else ""


def terminal_refusal(name: str) -> str | None:
    """Por que o terminal não pode receber o clique agora; None quando pode."""
    # O psmux não tem as flags de mouse: lá o sinal é a tela alternativa, que o Claude Code só usa
    # em tela cheia, e é só em tela cheia que ele liga o mouse.
    flag = "#{alternate_on}" if os.name == "nt" else "#{mouse_sgr_flag}"
    mouse, em_modo = (_tmux_format(name, f"{flag} #{{pane_in_mode}}").split() + ["", ""])[:2]
    # Em copy-mode o ESC do clique cancela o modo e o resto da sequência vira texto no prompt.
    if em_modo == "1":
        return "erro_mod_terminal_em_modo"
    return None if mouse == "1" else "erro_mod_mouse_desligado"


_RECUSAS = {
    "erro_mod_terminal_em_modo": "O terminal da sessão está em modo de rolagem; saia dele e tente de novo.",
    "erro_mod_mouse_desligado": "O Claude Code da sessão não está em tela cheia, e só nela ele liga o mouse.",
}


def screen(name: str) -> list[str]:
    """Só a parte visível: é nela que as coordenadas do mouse valem."""
    return tmux.capture_pane(name, 0).split("\n")


def click(name: str, row: int, col: int) -> bool:
    return tmux.send_keys(name, f"\x1b[<0;{col + 1};{row + 1}M\x1b[<0;{col + 1};{row + 1}m", literal=True)


def _site(name: str, site: str) -> tuple[dict | None, str | None]:
    """Árvore e posição do site: a faixa, ou um painel aberto."""
    _, dados = plugin_bridge.band(name)
    if site == BAND_SITE:
        return dados["above"], None
    painel = next((p for p in dados["panes"] if p.get("id") == site), None)
    if painel is None:
        raise PressRefused("erro_mod_botao_inexistente", "O painel do mod não está aberto.")
    return painel.get("tree"), painel.get("placement")


def _regiao(tela: list[str], name: str, site: str, placement: str | None) -> tuple[range, int, int | None] | None:
    top = prompt_top(tela)
    if top is None:
        return None
    largura = plugin_bridge.band_columns(name)
    ancora = plugin_bridge.band_anchor(name)
    if site == BAND_SITE:
        return range(band_start(tela, top, ancora), top), 0, largura
    if placement == "dock":
        return range(0, top), largura or 0, None
    # Inline fica acima da faixa; sem faixa visível (mod que só abre painel), logo acima do prompt.
    linha = anchor_row(tela, top, ancora)
    return range(0, top if linha is None else linha), 0, None


async def _click(name: str, linha: int, coluna: int) -> None:
    """O clique pelo terminal; sem a posse da escrita (Rust mudo, vínculo em dúvida) é recusa, não 500."""
    try:
        chegou = await run_tmux(click, name, linha, coluna)
    except (TimeoutError, RuntimeError) as exc:
        # As causas viram o mesmo 409 para o app; o log é o que separa uma da outra.
        _log.warning("clique de mod em %s recusado: %s", name, exc)
        chegou = False
    if not chegou:
        raise PressRefused("erro_mod_clique_sem_resposta", "O clique não chegou ao terminal.")


async def _celula(name: str, site: str, placement: str | None, rotulo: str) -> tuple[int, int]:
    """A célula única de `rotulo` na região do site, com o terminal em condição de receber o clique."""
    recusa = await run_tmux(terminal_refusal, name)
    if recusa:
        raise PressRefused(recusa, _RECUSAS[recusa])
    tela = await run_tmux(screen, name)
    regiao = _regiao(tela, name, site, placement)
    achados = find_label(tela, rotulo, *regiao) if regiao else []
    if not achados:
        raise PressRefused("erro_mod_botao_nao_achado", f"Não achei “{rotulo}” na tela do terminal.", rotulo=rotulo)
    if len(achados) > 1:
        raise PressRefused("erro_mod_botao_ambiguo", f"“{rotulo}” aparece mais de uma vez na tela.", rotulo=rotulo)
    return achados[0]


async def close(name: str, site: str) -> dict:
    """Fecha o painel `site` pelo `✕` do cabeçalho; a faixa não tem `✕`."""
    async with _locks.setdefault(name, asyncio.Lock()):
        if site == BAND_SITE:
            raise PressRefused("erro_mod_painel_inexistente", "Esse painel não está mais aberto no mod.")
        _, placement = _site(name, site)
        linha, coluna = await _celula(name, site, placement, CLOSE_LABEL)
        await _click(name, linha, coluna)
        if not await plugin_bridge.esperar_sem_painel(name, site, CONFIRM_S):
            raise PressRefused("erro_mod_clique_sem_resposta", "O painel não fechou.")
        return {"ok": True}


async def press(name: str, site: str, key: str, plugin: str | None) -> dict:
    async with _locks.setdefault(name, asyncio.Lock()):
        tree, placement = _site(name, site)
        rotulo = button_label(tree, key, plugin)
        if not rotulo:
            raise PressRefused("erro_mod_botao_inexistente", "O botão não está mais na tela do mod.")
        linha, coluna = await _celula(name, site, placement, rotulo)
        tentativa = plugin_bridge.esperar_clique_do_app(name, site, key, CONFIRM_S)
        try:
            desde = time.monotonic()
            await _click(name, linha, coluna)
            if not await plugin_bridge.esperar_press(name, site, key, desde, CONFIRM_S):
                raise PressRefused("erro_mod_clique_sem_resposta", "O mod não confirmou o clique.")
            copiado, aberto = await plugin_bridge.esperar_efeito(name, tentativa, EFFECT_S)
        finally:
            # Fechada aqui: efeito que chegar depois é recusado e acontece no terminal.
            plugin_bridge.encerrar_clique_do_app(name, tentativa)
        resposta: dict = {"ok": True}
        if copiado:
            resposta["copied"] = copiado
        if aberto:
            resposta["opened"] = aberto
        return resposta


# Na branch Rust só escreve no pane quem tem a posse da escrita: o clique é operação administrativa, como o /btw.
from app.runtime_terminal import wrap_driver as _wrap_terminal_driver
click = _wrap_terminal_driver(click, admin=True)
