"""Entrega de texto pro Pi pela extensão, sem digitar no tmux.

POR QUE EXISTE (medido 01-02/08/2026): o caminho de tecla confirma a entrega LENDO A TELA, e isso
falhou em produção — o aviso de subagente do Pi dentro da caixa do composer fazia o guarda adiar
pra sempre, e o usuário só descobria abrindo o terminal. Aqui a entrega é chamada de função dentro
do processo do Pi (`pi.sendUserMessage`), a mesma fila que o Enter do TUI usa.

O QUE A CONEXÃO PROVA: que ninguém fechou o socket. Nada além disso. Quem prova entrega é a
confirmação por mensagem — e mesmo ela atesta só que a extensão CHAMOU a API sem erro, porque
`sendUserMessage` é `void` (types.d.ts:915). É menos que "o agente leu", e é o máximo que existe.

CHAVE = `pane_id` do tmux (`%33`), NÃO o stem do .jsonl: o caminho quente do envio não tem o stem
à mão, e resolvê-lo certo pro Pi custaria um `registry.list()` (fork + /proc). O pane sai de graça
do `tmux list-panes` que o envio já faz, e a extensão o conhece pelo TMUX_PANE.
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import tempfile
import time
import uuid
from pathlib import Path
from typing import Awaitable, Callable

_log = logging.getLogger(__name__)

# Prazo da confirmação. Chute inicial: é chamada local, e a confirmação atesta só a CHAMADA, não o
# turno do agente. Medir com dado real e trocar — do jeito que o terminal_input.py documenta as
# constantes de tempo dele.
PRAZO_ACK = 3.0

Envia = Callable[[dict], Awaitable[None]]


class Linha:
    """Uma extensão conectada. `envia` põe o payload no socket; `pendentes` casa id -> resposta."""

    def __init__(self, envia: Envia) -> None:
        self.envia = envia
        self.pendentes: dict[str, asyncio.Future] = {}
        # Serializa por sessão, igual ao _send_lock do caminho de tecla (terminal_input.py:508):
        # sem isso duas mensagens simultâneas chegam fora de ordem do outro lado.
        self.lock = asyncio.Lock()


class PiInbox:
    def __init__(self) -> None:
        self._linhas: dict[str, Linha] = {}
        self._loop: asyncio.AbstractEventLoop | None = None

    def ligar_loop(self, loop: asyncio.AbstractEventLoop) -> None:
        """Guarda o loop do servidor pra `entregar_sync` conseguir atravessar da thread."""
        self._loop = loop

    def registrar(self, pane: str, envia: Envia) -> Linha:
        """Registra a linha. A ÚLTIMA vence: o caso real é processo zumbi no mesmo pane, e
        entregar pro zumbi é entregar pro nada (ninguém confirma)."""
        antiga = self._linhas.get(pane)
        if antiga is not None:
            for fut in antiga.pendentes.values():
                if not fut.done():
                    fut.set_result((False, "linha substituida"))
        linha = Linha(envia)
        self._linhas[pane] = linha
        return linha

    def remover(self, pane: str, linha: Linha) -> None:
        # Só remove se ainda for ESTA linha: uma reconexão que já registrou a nova não pode ser
        # apagada pelo desligamento tardio da antiga.
        if self._linhas.get(pane) is linha:
            del self._linhas[pane]
        for fut in linha.pendentes.values():
            if not fut.done():
                fut.set_result((False, "linha caiu"))

    def tem_linha(self, pane: str) -> bool:
        return pane in self._linhas

    def confirmar(self, pane: str, msg_id: str, ok: bool, erro: str | None) -> None:
        linha = self._linhas.get(pane)
        if linha is None:
            return
        fut = linha.pendentes.get(msg_id)
        if fut is not None and not fut.done():
            fut.set_result((ok, erro))

    async def entregar(self, pane: str, texto: str) -> str:
        """`sent` | `deferred` | `sem-linha`.

        `sem-linha` é o ÚNICO retorno que autoriza o chamador a cair pro caminho de tecla. Depois
        de ter mandado qualquer coisa pela linha, digitar por cima poderia entregar a mesma
        instrução duas vezes — e num canal de agente isso é ação executada duas vezes.
        """
        linha = self._linhas.get(pane)
        if linha is None:
            return "sem-linha"
        async with linha.lock:
            msg_id = uuid.uuid4().hex
            fut: asyncio.Future = asyncio.get_running_loop().create_future()
            linha.pendentes[msg_id] = fut
            try:
                # deliverAs SEMPRE explícito: com a sessão streamando, o prompt() do Pi levanta erro
                # se o comportamento não vier (agent-session.js:827-840). "steer" = entra na primeira
                # brecha entre ferramentas, que é o que serve pra corrigir rumo pelo celular.
                await linha.envia({"id": msg_id, "text": texto, "deliverAs": "steer"})
            except Exception as e:
                # Socket morto: tira do registro pra próxima mensagem já ir pela tecla sem pagar o
                # prazo de novo.
                _log.warning("pi_inbox: envio falhou pane=%s: %r", pane, e)
                linha.pendentes.pop(msg_id, None)
                self.remover(pane, linha)
                return "deferred"
            try:
                ok, erro = await asyncio.wait_for(fut, PRAZO_ACK)
            except asyncio.TimeoutError:
                _log.error("pi_inbox: sem confirmacao em %.1fs pane=%s — a mensagem fica na fila e "
                           "NAO vai por tecla (duplicaria)", PRAZO_ACK, pane)
                return "deferred"
            finally:
                linha.pendentes.pop(msg_id, None)
            if not ok:
                _log.error("pi_inbox: extensao recusou pane=%s: %s", pane, erro)
                return "deferred"
            return "sent"

    def entregar_sync(self, pane: str, texto: str) -> str:
        """Ponte pro mundo síncrono: o `send_prompt` roda em thread (o `_send_executor` do api.py),
        e a linha vive no loop do servidor. Sem isto, chamar `entregar` de lá não roda."""
        loop = self._loop
        if loop is None or pane not in self._linhas:
            return "sem-linha"
        fut = None
        try:
            fut = asyncio.run_coroutine_threadsafe(self.entregar(pane, texto), loop)
            # Quem manda no relógio é o `entregar`; este teto só evita travar a thread pra sempre
            # se o loop morrer no meio.
            return fut.result(PRAZO_ACK + 2.0)
        except Exception as e:
            _log.warning("pi_inbox: ponte sync falhou pane=%s: %r", pane, e)
            # Sem isto a corrotina do `entregar` segue viva no loop e pode terminar DEPOIS de o
            # chamador já ter decidido "deferred" — a fila reenvia pela mesma linha e a mesma
            # instrução chega duas vezes ao agente (achado da revisão final). cancel() so tem
            # efeito se a corrotina ainda não passou do próximo `await` (normalmente o
            # `linha.envia` em si, se o loop estava tão faminto a ponto de nem ter chegado lá).
            fut.cancel()
            return "deferred"


INBOX = PiInbox()


def escrever_endpoint() -> list[Path]:
    """Onde a extensão descobre pra onde ligar. A porta NÃO é fixa, então sem isso ela não sabe.

    Escreve em TODOS os diretórios de configuração: o projeto suporta sessão com
    CLAUDE_CONFIG_DIR alternativo (worktree), e o hook_installer.py:153 já resolve o mesmo problema
    iterando list_config_dirs(). Escrever só no padrão faria a sessão de um worktree nunca achar o
    arquivo e ficar PARA SEMPRE no fallback de tecla, em silêncio.
    """
    from app.config import list_config_dirs, resolve_bind_ip, settings

    # O uvicorn escuta em resolve_bind_ip(settings) (main.py), NAO em 127.0.0.1 fixo — no modo
    # celular documentado (CP_LAN_BIND_IP=auto) ou com IP fixo de LAN o bind e so naquela interface,
    # e ws://127.0.0.1 nunca conecta (recusado em silencio, extensao cai sempre pro caminho de tecla).
    # "0.0.0.0" e o unico caso em que 127.0.0.1 continua certo: bind em toda interface inclui loopback.
    bind = resolve_bind_ip(settings)
    host = "127.0.0.1" if bind == "0.0.0.0" else bind
    destinos: list[Path] = []
    dados = {"url": f"ws://{host}:{settings.port}/api/pi/inbox",
             "token": settings.auth_token, "ts": time.time()}
    for cfg in list_config_dirs():
        alvo = Path(cfg.path) / ".claude-pocket-conn.json"
        tmp: Path | None = None
        try:
            alvo.parent.mkdir(parents=True, exist_ok=True)
            # mkstemp CRIA o arquivo já em 0600 (é o open() com O_EXCL que fixa o modo na
            # criação — não um chmod depois). A versão anterior fazia write_text() (nasce com o
            # umask padrão, tipicamente 0644) e só DEPOIS chmod(0600): a janela com o token
            # legível por outro usuário acontecia na criação, e nenhum chmod posterior desfaz um
            # instante que já passou. mkstemp também garante nome único sozinho, então dispensa
            # o pid manual que outros sidecars do projeto (pricing.py:203) usam pra isso.
            fd, tmp_nome = tempfile.mkstemp(dir=alvo.parent, prefix=alvo.name + ".", suffix=".tmp")
            tmp = Path(tmp_nome)
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(json.dumps(dados))
            tmp.replace(alvo)
            destinos.append(alvo)
        except OSError as e:
            _log.warning("pi_inbox: nao consegui escrever %s: %r", alvo, e)
            if tmp is not None:
                # mkstemp não limpa sozinho se algo falhar no meio (só o replace bem-sucedido
                # "consome" o tmp) — sem isto um erro no meio do caminho deixa lixo .tmp
                # acumulando no diretório de config do usuário a cada tentativa.
                tmp.unlink(missing_ok=True)
    return destinos
