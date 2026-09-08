"""Acesso remoto ao navegador embutido da sessão: quadros para fora, toque e tecla para dentro.

NÃO é um espelho nem uma cópia. É O navegador da sessão — o mesmo que o agente dirige pelo
`hangar-preview` —, visto de outra tela. Quem olha pelo celular vê o que está acontecendo aqui e,
se quiser, mexe: o clique dele e o meu chegam no mesmo lugar.

Três decisões que valem saber:

*  **O backend fala CDP direto** (`127.0.0.1:9223`, porta que `shell/main.cjs` abre), num segundo
   cliente ao lado do `webContents.debugger` que o shell já mantém anexado. Medido em 07/09/2026:
   o Chromium aceita os dois, e o screencast rendeu 59,6 quadros/s. O caminho pelo servidor HTTP do
   shell (`preview_srv`) não serve para quadro: é pergunta-e-resposta, não fluxo.
*  **O layout (desktop/celular) NÃO vai por aqui** — vai pelo verbo `layout` do shell. A emulação se
   perde ao navegar e é o controlador de lá que sabe repor (`aplicarViewport`/`aoNavegar`); aplicar
   por fora criaria uma segunda fonte de verdade que a primeira navegação desfaz, calada.
*  **Com o painel do navegador fechado no desktop o view perde o compositor** e o screencast para de
   emitir. Aí o caminho é `Page.captureScreenshot` em laço — mais lento (a emulação de tamanho que o
   controlador aplica no modo oculto é o que o mantém respondendo).
"""
from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import secrets
import time
from pathlib import Path
from typing import Any, Optional

import websockets
from fastapi import WebSocket, WebSocketDisconnect

from app import tmux
from app.auth import _LOOPBACK, _blocked, _record_fail
from app.config import settings

_log = logging.getLogger("hangar.navsock")

CDP_HOST = "127.0.0.1:9223"
# 12 quadros/s: acima disso o ganho é imperceptível numa tela de celular e a conta pelo túnel da VPS
# fica cara (medido: 42 KiB por quadro; a 60/s são 2,5 MiB/s).
INTERVALO_QUADRO = 1 / 12
# Sem quadro de screencast por este tempo = view sem compositor (painel fechado no desktop). Cai pro
# print em laço.
ESPERA_SCREENCAST = 1.5
INTERVALO_PRINT = 0.4
TETO_MSG_CLIENTE = 64 * 1024


def _pasta_nav() -> Path:
    return Path.home() / ".hangar" / "nav"


def alvo_da_sessao(name: str) -> Optional[dict[str, Any]]:
    """Sidecar do navegador daquela sessão: `targetId` e `url`. `None` quando não há navegador.

    O shell grava um arquivo por navegador; a chave é `<servidor>::<sessão>`, e o mesmo casamento
    por sufixo que `GET /api/sessions/{name}/navegador` usa vale aqui.
    """
    pasta = _pasta_nav()
    if not pasta.is_dir():
        return None
    for arq in pasta.glob("*.json"):
        if arq.name.startswith("_"):        # _srv.json e _pendentes.json não são navegador
            continue
        try:
            sc = json.loads(arq.read_text(encoding="utf-8"))
        except (OSError, ValueError) as e:
            _log.warning("sidecar de navegador ilegivel %s: %s", arq, e)
            continue
        if not isinstance(sc, dict):
            continue
        chave = str(sc.get("chave", ""))
        if (chave == name or chave.endswith(f"::{name}")) and sc.get("targetId"):
            return sc
    return None


class _Cdp:
    """Cliente CDP mínimo: manda comando, casa resposta por id, entrega evento por fila."""

    def __init__(self, target_id: str):
        self._url = f"ws://{CDP_HOST}/devtools/page/{target_id}"
        self._ws: Any = None
        self._id = 0
        self._pendentes: dict[int, asyncio.Future] = {}
        self.eventos: asyncio.Queue = asyncio.Queue(maxsize=64)
        self._bomba: Optional[asyncio.Task] = None
        # O loop só guarda referência FRACA de task: sem este conjunto, o coletor pode levar embora
        # uma task de input antes de ela rodar, e a tecla some sem erro nenhum.
        self._soltas: set[asyncio.Task] = set()

    async def __aenter__(self) -> "_Cdp":
        # max_size alto: um quadro jpeg em base64 de tela cheia passa de 1 MiB.
        self._ws = await websockets.connect(self._url, max_size=32 * 1024 * 1024,
                                            open_timeout=5, ping_interval=20)
        self._bomba = asyncio.create_task(self._bombear())
        return self

    async def __aexit__(self, *_exc) -> None:
        for t in list(self._soltas):
            t.cancel()
        if self._bomba:
            self._bomba.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._bomba
        if self._ws:
            await self._ws.close()

    async def _bombear(self) -> None:
        try:
            async for bruto in self._ws:
                msg = json.loads(bruto)
                ident = msg.get("id")
                if ident is not None:
                    fut = self._pendentes.pop(ident, None)
                    if fut and not fut.done():
                        fut.set_result(msg)
                elif msg.get("method"):
                    # Fila cheia = consumidor atrasado. Descarta o MAIS VELHO: num fluxo de quadros
                    # o antigo não interessa, e travar aqui pararia o CDP inteiro.
                    if self.eventos.full():
                        with contextlib.suppress(asyncio.QueueEmpty):
                            self.eventos.get_nowait()
                    self.eventos.put_nowait(msg)
        except (asyncio.CancelledError, websockets.ConnectionClosed):
            pass
        finally:
            for fut in self._pendentes.values():
                if not fut.done():
                    fut.set_exception(ConnectionError("CDP fechou"))
            self._pendentes.clear()

    async def cmd(self, metodo: str, params: Optional[dict] = None,
                  espera: float = 10.0) -> dict:
        self._id += 1
        ident = self._id
        fut: asyncio.Future = asyncio.get_running_loop().create_future()
        self._pendentes[ident] = fut
        try:
            # O `send` dentro do try: falhando com o CDP já fechado, o `finally` é o que impede a
            # entrada em `_pendentes` de ficar pra sempre — e cada input mandado depois da queda
            # deixaria uma.
            await self._ws.send(json.dumps({"id": ident, "method": metodo, "params": params or {}}))
            return await asyncio.wait_for(fut, timeout=espera)
        finally:
            self._pendentes.pop(ident, None)

    def envia(self, metodo: str, params: Optional[dict] = None) -> asyncio.Task:
        """Comando sem esperar resposta — o caminho do input, onde a latência é o produto."""
        t = asyncio.create_task(_calado(self.cmd(metodo, params)))
        self._soltas.add(t)
        t.add_done_callback(self._soltas.discard)
        return t


async def _calado(coro) -> None:
    with contextlib.suppress(Exception):
        await coro


# ── entrada vinda de quem está olhando ──────────────────────────────────────────
# Coordenadas chegam em FRAÇÃO da tela (0..1), não em pixel: o celular não sabe (nem precisa saber)
# o tamanho do viewport do outro lado, e ele muda quando o layout troca.

_BOTOES = {"left", "right", "middle", "none"}


def _px(valor: Any, tamanho: int) -> int:
    try:
        f = float(valor)
    except (TypeError, ValueError):
        return 0
    return int(max(0.0, min(1.0, f)) * tamanho)


async def _aplicar_entrada(cdp: _Cdp, msg: dict, largura: int, altura: int) -> None:
    tipo = msg.get("t")
    if tipo == "m":
        acao = {"press": "mousePressed", "release": "mouseReleased",
                "move": "mouseMoved"}.get(str(msg.get("acao")))
        if not acao:
            return
        botao = str(msg.get("b", "left"))
        cdp.envia("Input.dispatchMouseEvent", {
            "type": acao,
            "x": _px(msg.get("x"), largura), "y": _px(msg.get("y"), altura),
            "button": botao if botao in _BOTOES else "left",
            "clickCount": 1 if acao != "mouseMoved" else 0,
            "modifiers": int(msg.get("mod") or 0),
        })
    elif tipo == "w":
        cdp.envia("Input.dispatchMouseEvent", {
            "type": "mouseWheel",
            "x": _px(msg.get("x"), largura), "y": _px(msg.get("y"), altura),
            "deltaX": float(msg.get("dx") or 0), "deltaY": float(msg.get("dy") or 0),
            "button": "none", "clickCount": 0, "modifiers": 0,
        })
    elif tipo == "txt":
        texto = str(msg.get("s") or "")[:4096]
        if texto:
            cdp.envia("Input.insertText", {"text": texto})
    elif tipo == "k":
        tecla = str(msg.get("key") or "")[:32]
        if not tecla:
            return
        # `key` sozinho basta pras nomeadas (Enter, Backspace, setas) — é o mesmo atalho que o
        # controlador do shell usa; texto comum vai por `insertText`, não por tecla.
        for acao in ("keyDown", "keyUp"):
            cdp.envia("Input.dispatchKeyEvent", {"type": acao, "key": tecla,
                                                 "windowsVirtualKeyCode": int(msg.get("code") or 0),
                                                 "modifiers": int(msg.get("mod") or 0)})


# ── quadros ────────────────────────────────────────────────────────────────────

async def _fluxo_de_quadros(cdp: _Cdp, ws: WebSocket, estado: dict) -> None:
    """Screencast enquanto o view compõe; print em laço quando ele não compõe."""
    await cdp.cmd("Page.enable")
    # O quadro sai no tamanho que a tela de quem olha aguenta, não no tamanho da página: mandar
    # 1400px de largura pra um iPhone é pagar banda por pixel que a tela dele não mostra, e a conta
    # é paga no túnel da VPS. `quality` 45 já é o suficiente pra ler texto nesse tamanho.
    largura = int(estado.get("pedida") or 0) or 1400
    await cdp.cmd("Page.startScreencast", {"format": "jpeg", "quality": 45,
                                           "maxWidth": largura, "maxHeight": largura,
                                           "everyNthFrame": 1})
    ultimo_envio = 0.0
    while True:
        try:
            msg = await asyncio.wait_for(cdp.eventos.get(), timeout=ESPERA_SCREENCAST)
        except asyncio.TimeoutError:
            # Sem compositor: o print é o único caminho, e ele é caro — por isso só entra aqui.
            await _print_avulso(cdp, ws, estado)
            continue
        metodo = msg.get("method")
        if metodo == "Page.screencastFrame":
            p = msg.get("params") or {}
            # O ACK é obrigatório: sem ele o Chromium para de emitir. Vai SEMPRE, mesmo no quadro
            # que a gente descarta por taxa — descartar é decisão nossa, não motivo pra travar.
            cdp.envia("Page.screencastFrameAck", {"sessionId": p.get("sessionId")})
            meta = p.get("metadata") or {}
            estado["w"] = int(meta.get("deviceWidth") or estado["w"])
            estado["h"] = int(meta.get("deviceHeight") or estado["h"])
            agora = time.monotonic()
            if agora - ultimo_envio < INTERVALO_QUADRO:
                continue
            ultimo_envio = agora
            await ws.send_text(json.dumps({"t": "q", "d": p.get("data"),
                                           "w": estado["w"], "h": estado["h"]}))
        elif metodo == "Page.frameNavigated":
            frame = (msg.get("params") or {}).get("frame") or {}
            if not frame.get("parentId"):
                await ws.send_text(json.dumps({"t": "url", "url": frame.get("url")}))
        elif metodo == "Inspector.detached":
            raise ConnectionError("o navegador desta sessão fechou")


async def _print_avulso(cdp: _Cdp, ws: WebSocket, estado: dict) -> None:
    try:
        r = await cdp.cmd("Page.captureScreenshot", {"format": "jpeg", "quality": 55}, espera=20)
    except websockets.ConnectionClosed as e:
        # CDP morreu (o navegador fechou). Sem relevantar, o laço ficava tentando pra sempre: sem
        # quadro, sem erro na tela e sem nunca terminar o handler.
        raise ConnectionError("o navegador desta sessão fechou") from e
    except Exception as e:
        _log.debug("navsock: print falhou: %r", e)
        await asyncio.sleep(INTERVALO_PRINT)
        return
    dado = ((r.get("result") or {}).get("data"))
    if dado:
        await ws.send_text(json.dumps({"t": "q", "d": dado, "w": estado["w"], "h": estado["h"],
                                       "lento": True}))
    await asyncio.sleep(INTERVALO_PRINT)


# ── handler ────────────────────────────────────────────────────────────────────

async def _autorizado(ws: WebSocket) -> bool:
    from app import termsock                      # mesma porta de entrada do painel de terminal
    host = ws.client.host if ws.client else ""
    agora = time.time()
    if _blocked(host, agora):
        await ws.close(code=1008)
        return False
    tok = ws.query_params.get("token", "")
    if not settings.auth_token or not secrets.compare_digest(tok.encode(),
                                                             settings.auth_token.encode()):
        if host not in _LOOPBACK:
            _record_fail(host, agora)
        await ws.close(code=1008)
        return False
    origem = ws.headers.get("origin")
    if origem and not termsock._origem_aceita(origem, ws.headers.get("host")):
        _log.warning("navsock: origem %r recusada", origem)
        # Com motivo, ao contrário do token: origem recusada é CONFIGURAÇÃO (o app servido de outro
        # endereço, que se declara em CP_TERM_ORIGINS), não credencial errada — e sem essa frase ela
        # chega na tela como "conexão caiu", igual a cabo solto.
        await ws.close(code=1008, reason="origem nao autorizada (CP_TERM_ORIGINS)")
        return False
    return True


def _largura_pedida(ws: WebSocket) -> int:
    """Largura útil da tela de quem está olhando, em pixels de verdade. 0 = não disse."""
    try:
        return max(0, min(1400, int(ws.query_params.get("w", "0"))))
    except ValueError:
        return 0


async def _url_atual(cdp: _Cdp, sc: dict) -> Optional[str]:
    try:
        r = await cdp.cmd("Runtime.evaluate", {"expression": "location.href",
                                               "returnByValue": True}, espera=5)
        valor = (((r.get("result") or {}).get("result")) or {}).get("value")
        if isinstance(valor, str) and valor:
            return valor
    except Exception as e:
        _log.debug("navsock: url atual falhou: %r", e)
    return sc.get("url")


async def nav_ws(ws: WebSocket, name: str) -> None:
    if not await _autorizado(ws):
        return
    # `to_thread` nos dois: varrer os sidecars e perguntar ao tmux são disco e processo, e este é o
    # mesmo event loop que serve o SSE de todas as sessões (incidente de 2026-07-23 no CLAUDE.md).
    if not await asyncio.to_thread(tmux.has_session, name):
        await ws.close(code=1008, reason="sessao nao existe")
        return
    sc = await asyncio.to_thread(alvo_da_sessao, name)
    if not sc:
        await ws.close(code=1008, reason="sessao sem navegador")
        return
    await ws.accept()
    estado = {"w": 1280, "h": 800, "pedida": _largura_pedida(ws)}
    try:
        async with _Cdp(str(sc["targetId"])) as cdp:
            # A url do sidecar é a da última vez que o shell gravou; quem sabe a de agora é a
            # página. Sem isto a barra abre mostrando o endereço anterior até alguém navegar.
            await ws.send_text(json.dumps({"t": "url", "url": await _url_atual(cdp, sc)}))
            saida = asyncio.create_task(_fluxo_de_quadros(cdp, ws, estado))
            entrada = asyncio.create_task(_ler_cliente(cdp, ws, estado, name))
            prontas, pendentes = await asyncio.wait({saida, entrada},
                                                    return_when=asyncio.FIRST_COMPLETED)
            for t in pendentes:
                t.cancel()
                with contextlib.suppress(asyncio.CancelledError, Exception):
                    await t
            # A que TERMINOU tem o motivo: engolir aqui era ficar sem o aviso na tela e sem log de
            # uma queda de verdade — quem trata é o `except` lá embaixo.
            for t in prontas:
                await t
    except WebSocketDisconnect:
        pass
    except Exception as e:
        _log.warning("navsock %s: %r", name, e)
        with contextlib.suppress(Exception):
            await ws.send_text(json.dumps({"t": "erro", "m": str(e)}))
    finally:
        with contextlib.suppress(Exception):
            await ws.close()


async def _ler_cliente(cdp: _Cdp, ws: WebSocket, estado: dict, name: str) -> None:
    while True:
        bruto = await ws.receive_text()
        if len(bruto) > TETO_MSG_CLIENTE:
            continue
        try:
            msg = json.loads(bruto)
        except ValueError:
            continue
        if not isinstance(msg, dict):
            continue
        if msg.get("t") == "layout":
            await _trocar_layout(name, str(msg.get("modo") or ""), ws)
            continue
        await _aplicar_entrada(cdp, msg, estado["w"], estado["h"])


async def _trocar_layout(name: str, modo: str, ws: WebSocket) -> None:
    """Vai pelo servidor do shell, não por CDP — ver a nota no topo do módulo."""
    if modo not in ("mobile", "desktop"):
        return
    from app import navshell
    try:
        resposta = await asyncio.to_thread(navshell.verbo, name, "layout", [modo])
    except Exception as e:
        await ws.send_text(json.dumps({"t": "erro", "m": str(e)}))
        return
    await ws.send_text(json.dumps({"t": "layout", "modo": modo, "resposta": resposta}))
