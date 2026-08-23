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

...MAS o pane só é único no tmux. No psmux (Windows) ele é numerado por SESSÃO, então TODA sessão
Pi se declarava `%1` e as duas dividiam o MESMO slot de linha — a última a conectar ficava com ele.
Medido em 22/08/2026, com `pi-teste` e `pi-medir` vivas: um `POST /input` endereçado à `pi-teste`
respondeu `delivered: true` e a mensagem apareceu na conversa da `pi-medir`. É a mesma família do
bug do bilhete pane→sessão (que o `paneKey()` da extensão já resolve com `PSMUX_SESSION`), numa
porta que ficou de fora.

Por isso a extensão declara uma CHAVE ao conectar: o nome da sessão do psmux quando existe, o pane
quando não (tmux). Quem procura usa `linha_de()`, que tenta o nome primeiro e o pane depois — no
tmux nada é registrado por nome, então o primeiro tento sempre erra e o caminho fica byte-idêntico
ao de antes, sem nenhum teste de sistema operacional no meio.
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

from app import atomico

_log = logging.getLogger(__name__)

# Prazo da confirmação. Chute inicial: é chamada local, e a confirmação atesta só a CHAMADA, não o
# turno do agente. Medir com dado real e trocar — do jeito que o terminal_input.py documenta as
# constantes de tempo dele.
PRAZO_ACK = 3.0

# Prazo da PERGUNTA (`perguntar`), separado do PRAZO_ACK de propósito: uma pergunta é leitura de
# estado dentro do processo do Pi (`ctx.ui.getEditorText()` é um acesso a string, não uma chamada
# de rede), e quem espera por ela está segurando o `_send_lock` da sessão antes de digitar. Não
# respondeu em 1s, não vale a pena esperar: quem pergunta tem plano B (raspar o pane).
PRAZO_PERGUNTA = 1.0

Envia = Callable[[dict], Awaitable[None]]


class Linha:
    """Uma extensão conectada. `envia` põe o payload no socket; `pendentes` casa id -> resposta."""

    def __init__(self, envia: Envia) -> None:
        self.envia = envia
        self.pendentes: dict[str, asyncio.Future] = {}
        # Perguntas ficam num dicionário SEPARADO das entregas: a resposta de uma entrega é
        # `(ok, erro)` e a de uma pergunta é o valor lido. Um dicionário só faria o `confirmar` de
        # uma entrega resolver a future de uma pergunta com a tupla errada — e o desempacotamento
        # estouraria dentro do `perguntar`, no caminho que existe justamente pra ser à prova de
        # falha (o chamador tem plano B).
        self.perguntas: dict[str, asyncio.Future] = {}
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
            for fut in antiga.perguntas.values():
                if not fut.done():
                    fut.set_result(None)     # None = "nao sei" -> quem perguntou cai no plano B
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
        for fut in linha.perguntas.values():
            if not fut.done():
                fut.set_result(None)

    def tem_linha(self, pane: str) -> bool:
        return pane in self._linhas

    def confirmar(self, pane: str, msg_id: str, ok: bool, erro: str | None) -> None:
        linha = self._linhas.get(pane)
        if linha is None:
            return
        fut = linha.pendentes.get(msg_id)
        if fut is not None and not fut.done():
            fut.set_result((ok, erro))

    def responder(self, pane: str, msg_id: str, valor: str | None) -> None:
        """Resposta de uma PERGUNTA. `valor=None` = a extensão não soube responder (versão antiga,
        API ausente, sem contexto ainda) — que é diferente de `""`, "o campo está vazio"."""
        linha = self._linhas.get(pane)
        if linha is None:
            return
        fut = linha.perguntas.get(msg_id)
        if fut is not None and not fut.done():
            fut.set_result(valor)

    async def entregar(self, pane: str, texto: str, msg_id: str | None = None) -> str:
        """`sent` | `deferred` | `sem-linha`.

        `sem-linha` é o ÚNICO retorno que autoriza o chamador a cair pro caminho de tecla. Depois
        de ter mandado qualquer coisa pela linha, digitar por cima poderia entregar a mesma
        instrução duas vezes — e num canal de agente isso é ação executada duas vezes.

        `msg_id`: identidade ESTÁVEL entre reentregas da MESMA mensagem (achado ALTA da revisão
        02/08/2026 — "Porta A"). A extensão chama `sendUserMessage` ANTES de confirmar (ver
        cp-state.ts) — se o ACK atrasar/perder, este método devolve "deferred" mas a instrução JÁ
        pode ter chegado no agente. Sem um id que sobreviva ao retry, a extensão não tem como saber
        que é a MESMA tentativa e chamaria `sendUserMessage` de novo. Quem tem uma identidade
        durável pra oferecer (o id da entrada na `PromptQueue`, ver `terminal_input.drain`/`send_prompt`)
        passa `msg_id`; sem ele, cai no uuid4 por tentativa de sempre — só serve pra casar pedido e
        resposta DENTRO desta única chamada, não protege contra retry (risco documentado no relatório).
        """
        linha = self._linhas.get(pane)
        if linha is None:
            return "sem-linha"
        async with linha.lock:
            msg_id = msg_id or uuid.uuid4().hex
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

    async def perguntar(self, pane: str, o_que: str) -> str | None:
        """Pergunta de LEITURA pra extensão. Devolve o valor, ou `None` quando não dá pra saber.

        Existe porque ler a tela não responde a pergunta que o `terminal_input` precisa fazer antes
        de digitar — "tem rascunho do usuário na caixa?". Medido 22/08/2026: o Pi desenha aviso de
        extensão (`console.error`) DENTRO da faixa do composer, com o MESMO ANSI do texto digitado
        (`\\x1b[0m … \\x1b[0m`), e o `cursor_flag` do psmux é 0 — não há nada na tela que separe
        aviso de rascunho. Dentro do processo do Pi a pergunta é trivial: `ctx.ui.getEditorText()`
        devolveu `""` com o aviso na faixa e o texto exato com um rascunho parado.

        `None` cobre TODOS os "não sei" — sem linha, extensão velha que não entende a pergunta,
        socket morto, prazo estourado. Nunca levanta: quem chama tem plano B e um erro aqui viraria
        um envio travado por causa de uma leitura auxiliar.
        """
        linha = self._linhas.get(pane)
        if linha is None:
            return None
        # SEM o `linha.lock`: a pergunta é leitura e não pode ficar atrás de uma entrega no meio do
        # ACK (até 3s) — o lock existe pra ordenar ESCRITAS no agente, e duas respostas não se
        # confundem porque cada uma tem seu id.
        msg_id = uuid.uuid4().hex
        fut: asyncio.Future = asyncio.get_running_loop().create_future()
        linha.perguntas[msg_id] = fut
        try:
            await linha.envia({"id": msg_id, "pedir": o_que})
        except Exception as e:                       # noqa: BLE001
            _log.warning("pi_inbox: pergunta %r falhou pane=%s: %r", o_que, pane, e)
            linha.perguntas.pop(msg_id, None)
            self.remover(pane, linha)
            return None
        try:
            return await asyncio.wait_for(fut, PRAZO_PERGUNTA)
        except asyncio.TimeoutError:
            # Nível debug, não warning: extensão anterior a esta versão simplesmente ignora um
            # `pedir` que não conhece, e isso é normal até todo mundo dar /reload — não é defeito
            # pra encher log.
            _log.debug("pi_inbox: sem resposta pra %r em %.1fs pane=%s", o_que, PRAZO_PERGUNTA, pane)
            return None
        finally:
            linha.perguntas.pop(msg_id, None)

    def perguntar_sync(self, pane: str, o_que: str) -> str | None:
        """Ponte pro mundo síncrono, igual à do `entregar_sync` (o `send_prompt` roda em thread)."""
        loop = self._loop
        if loop is None or pane not in self._linhas:
            return None
        fut = None
        try:
            fut = asyncio.run_coroutine_threadsafe(self.perguntar(pane, o_que), loop)
            return fut.result(PRAZO_PERGUNTA + 1.0)
        except Exception as e:                       # noqa: BLE001
            _log.debug("pi_inbox: ponte sync da pergunta falhou pane=%s: %r", pane, e)
            if fut is not None:
                fut.cancel()
            return None

    def entregar_sync(self, pane: str, texto: str, msg_id: str | None = None) -> str:
        """Ponte pro mundo síncrono: o `send_prompt` roda em thread (o `_send_executor` do api.py),
        e a linha vive no loop do servidor. Sem isto, chamar `entregar` de lá não roda.
        `msg_id`: repassado pra `entregar` sem alteração — ver o docstring de lá (identidade
        estável entre reentregas)."""
        loop = self._loop
        if loop is None or pane not in self._linhas:
            return "sem-linha"
        fut = None
        try:
            fut = asyncio.run_coroutine_threadsafe(self.entregar(pane, texto, msg_id), loop)
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
            # `fut` continua None se o PRÓPRIO run_coroutine_threadsafe levantou (loop fechou entre
            # o guard da linha 133 e a chamada — corrida real de restart/shutdown, e é pra cobrir
            # ISSO que este try existe): sem o guard, o cancel() estoura AttributeError e escapa,
            # quebrando o contrato "nunca levanta" (achado da re-revisão final).
            if fut is not None:
                fut.cancel()
            return "deferred"


INBOX = PiInbox()


def linha_de(name: str | None, pane_id: str | None) -> str | None:
    """A chave da linha DESTA sessão, ou None se ela não tem linha.

    Nome primeiro, pane depois — e a ordem é o conserto (ver o topo do arquivo): no psmux o pane é
    `%1` em toda sessão, então procurar por pane entrega na conversa errada. No tmux a extensão
    nunca declara nome, o primeiro tento erra sempre e o resultado é o pane de sempre.

    Ambiguidade teórica: uma sessão chamada literalmente `%1` casaria com a chave de pane de outra.
    Nome de sessão criada pelo app passa por `sanitize_session_name`, que não deixa `%` passar; e
    quem batiza uma sessão à mão de `%1` já tem problema maior com o próprio tmux.
    """
    if name and INBOX.tem_linha(name):
        return name
    if pane_id and INBOX.tem_linha(pane_id):
        return pane_id
    return None


def escrever_endpoint() -> list[Path]:
    """Onde a extensão descobre pra onde ligar. A porta NÃO é fixa, então sem isso ela não sabe.

    Escreve em TODOS os diretórios de configuração: o projeto suporta sessão com
    CLAUDE_CONFIG_DIR alternativo (worktree), e o hook_installer.py:153 já resolve o mesmo problema
    iterando list_config_dirs(). Escrever só no padrão faria a sessão de um worktree nunca achar o
    arquivo e ficar PARA SEMPRE no fallback de tecla, em silêncio.
    """
    from app.config import list_config_dirs, resolve_bind_ip, settings

    try:
        # O uvicorn escuta em resolve_bind_ip(settings) (main.py), NAO em 127.0.0.1 fixo — no modo
        # celular documentado (CP_LAN_BIND_IP=auto) ou com IP fixo de LAN o bind e so naquela
        # interface, e ws://127.0.0.1 nunca conecta (recusado em silencio, extensao cai sempre pro
        # caminho de tecla). "0.0.0.0" e o unico caso em que 127.0.0.1 continua certo: bind em toda
        # interface inclui loopback.
        bind = resolve_bind_ip(settings)
        host = "127.0.0.1" if bind == "0.0.0.0" else bind
        cfgs = list_config_dirs()
        dados = {"url": f"ws://{host}:{settings.port}/api/pi/inbox",
                 "token": settings.auth_token, "ts": time.time()}
    except Exception as e:
        # Achado MEDIA da revisao 02/08/2026: isto roda na SUBIDA (main.py:91), logo depois de dois
        # hooks explicitamente "idempotente, fail-soft". So o OSError por diretorio (abaixo) era
        # pego — qualquer excecao de list_config_dirs()/resolve_bind_ip()/json.dumps subia CRUA e
        # derrubava a subida INTEIRA do backend por causa de um sidecar auxiliar (a linha do Pi e
        # best-effort, nao o nucleo de conexao). Fail-soft de verdade: loga e devolve vazio, como os
        # hooks vizinhos (hook_installer.ensure_*_hooks_installed).
        _log.warning("pi_inbox: nao consegui preparar o endpoint da linha, subindo sem ela: %r", e)
        return []

    destinos: list[Path] = []
    for cfg in cfgs:
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
            atomico.substituir(tmp, alvo)
            destinos.append(alvo)
        except Exception as e:
            # Exception generica (nao só OSError) pelo mesmo motivo do try de cima: um sidecar
            # auxiliar nao pode derrubar o loop por-diretorio nem a subida.
            _log.warning("pi_inbox: nao consegui escrever %s: %r", alvo, e)
            if tmp is not None:
                # mkstemp não limpa sozinho se algo falhar no meio (só o replace bem-sucedido
                # "consome" o tmp) — sem isto um erro no meio do caminho deixa lixo .tmp
                # acumulando no diretório de config do usuário a cada tentativa.
                tmp.unlink(missing_ok=True)
    return destinos
