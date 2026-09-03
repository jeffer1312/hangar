import anyio.to_thread
import asyncio
import json
import logging
import mimetypes
import os
import re
import secrets
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.request
from datetime import datetime
from concurrent.futures import ThreadPoolExecutor
from contextlib import asynccontextmanager
from dataclasses import asdict
from pathlib import Path
from typing import Annotated, Literal, Optional
from fastapi import FastAPI, Depends, HTTPException, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import FileResponse, JSONResponse, Response
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, ConfigDict, Field
from sse_starlette.sse import EventSourceResponse
from app import atomico, atualizacoes, atualizar, diag, migracao_sidecars, pensamento_pt, tmux
from app.auth import require_auth, require_loopback
from app import bastao as bastao_mod   # `bastao` sem sufixo é a ROTA GET, mais abaixo neste arquivo
from app.bastao import montar as bastao_montar
from app.commands import list_commands
from app.fs import FsError, list_roots, scan_dir
from app.model_picker import PickerError
from app.mensagens import erro
from app import kimi_models
from app import codex_models
from app import model_args
from app import filesearch, filetree, git_ops
from app.filesearch import SearchError
from app.filetree import FileError
from app import orq, orq_md, orq_papeis, orq_politica
from app import pi_catalog
from app import cli_probe
from app import pi_models
from app import pi_inbox
from app.pi_inbox import INBOX
from app import registry as registry_mod
from app.registry import KillFailed, SessionRegistry, sanitize_cwd
from app.names import sanitize_session_name
from app.models import (SessionInfo, ChatEvent, CostReport, RunnersResponse, RunBody, RunInfo,
                        ProjectStatus, session_key)
from app.planprog import (plan_progress, list_plans, write_pin, is_safe_stem, _plans_dir,
                          PlanPinError, PIN_NONE, marcar_step, arquivar, caminho_do_plano,
                          PlanWriteError)
from app.pqueue import (PromptQueue, _transcript_start_ts, committed_user_lines,
                        linha_mais_parecida)
from app.prune import prune_loop as _prune_loop
from app.renova_token import laco as _renova_token_loop
from app.chain import ThenLink
from app import terminal_input
from app.terminal_input import TerminalInput, drain
from app.adapters import get_adapter
from app.adapters.codex import sessions as codex_sessions
from app.sse import merged_events
from app.state import corrige_ocioso_kimi
from app.uploads import save_upload, resolve_upload, prune_old, list_uploads, UploadError, MAX_BYTES
from app.video import is_video, extract_frames, extract_audio
from app.transcribe import transcribe, TranscribeError
from app.config import (list_config_dirs, ConfigDirInfo, _backend_config_base, settings,
                        automations_enabled, resolve_bind_ip)
from app import runtime_config
from app import tts
from app.tts_text import preparar as tts_preparar
from app import narrar
from app import contas, default_model, engine_probe, engines, procinfo
from app.costs import report as costs_report, usd_brl as _usd_brl, PERIODOS as _COST_PERIODOS
from app import pricing
from app.git_ops import (
    list_branches, switch_branch, git_action, git_log, assign_lanes, changed_files, file_diff, discard_file, commit_files, commit_file_diff, commit_diff, revert_commit, cherry_pick, reset_to, create_branch_at, create_tag, diff_vs_worktree, branches_containing, commit, last_commit_message, push as push_branch, sequencer_state, GitError, branch_of,
)
from app import loop as loop_mod
from app.transcript import last_assistant_text
from app import tunnel
from app import runner
from app import projects
from app import archive_providers
from app.archive import (ArchiveEntry, ArchiveFolder, archive_cwd, archive_jsonl, conta_de,
                         list_conversations, list_folders, tail_events)
from app.search import SearchHit, search, extract_terms, search_terms, build_ask_prompt
from app.askquestion import clear_pending_askq, read_pending_askq
from app import pair
from app import pair_texto
from app import peers
from app import alcance, conta_estado, cotas, credenciais, peers_api
from app.pair import PairLink, contract_path_for
from app.hook_state import hook_state
from app import push
from app import stall_watch
from app.sync import sync_router
from app.deploy import deploy_router
from app import desktop_palette

_log = logging.getLogger("hangar")


class _BodyTooLarge(Exception):
    """Sinaliza corpo da request acima do limite (estoura no receive, antes de bufferizar tudo)."""


class _BodySizeLimitMiddleware:
    # Limite GLOBAL de corpo, em ASGI: conta os bytes do stream e aborta com 413 ao passar de max_bytes.
    # Cobre o que o check de Content-Length do /upload NAO pega (chunked, sem header) e roda ANTES do
    # require_auth -> impede o buffer ilimitado pre-auth. ponytail: teto global unico (= MAX_BYTES do
    # upload); se um dia precisar cap menor por rota, da pra escopar por scope["path"].
    def __init__(self, app, max_bytes: int):
        self.app = app
        self.max_bytes = max_bytes

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        headers = dict(scope.get("headers") or [])
        clen = headers.get(b"content-length")
        if clen is not None and clen.isdigit() and int(clen) > self.max_bytes:
            await self._reject(send)
            return
        total = 0
        started = False

        async def limited_receive():
            nonlocal total
            message = await receive()
            if message["type"] == "http.request":
                total += len(message.get("body", b""))
                if total > self.max_bytes:
                    raise _BodyTooLarge()
            return message

        async def tracked_send(message):
            nonlocal started
            if message["type"] == "http.response.start":
                started = True
            await send(message)

        try:
            await self.app(scope, limited_receive, tracked_send)
        except _BodyTooLarge:
            if not started:  # so responde se o handler ainda nao comecou a responder
                await self._reject(send)

    async def _reject(self, send):
        await send({"type": "http.response.start", "status": 413,
                    "headers": [(b"content-type", b"text/plain; charset=utf-8")]})
        await send({"type": "http.response.body", "body": b"request body too large"})


@asynccontextmanager
async def _lifespan(app: FastAPI):
    # Uma vez na subida, nunca por request. O Starlette roda cada rota `def` (sao 65 aqui) num
    # anyio.to_thread, cujo limiter default e de 40 tokens — e cada conexao de chat ainda segura
    # DOIS deles PERMANENTEMENTE, num awatch parado (transcript.py:408 e pqueue.py:366). Com ~20
    # abas os 40 acabam e a API inteira congela, sem erro e sem log. Watcher parado nao gasta CPU,
    # so o slot, entao subir o teto e barato.
    anyio.to_thread.current_default_thread_limiter().total_tokens = 200
    _state_dirs =list({Path(c.path) for c in list_config_dirs()} | {_backend_config_base().resolve()})
    hook_state.on_awaiting = _on_awaiting  # transicao -> awaiting_input dispara web push
    hook_state.on_transition = _on_hook_transition  # drain server-side + confirmacao de entrega
    task = asyncio.create_task(hook_state.watch(_state_dirs))

    def _watch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("hook_state.watch crashed", exc_info=exc)

    task.add_done_callback(_watch_done)

    stall_task = asyncio.create_task(stall_watch.watch())

    def _stall_watch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("stall_watch.watch crashed", exc_info=exc)

    stall_task.add_done_callback(_stall_watch_done)

    # Poda periodica dos sidecars de sessao morta (Task G3): varre na subida e depois a cada
    # 24h — ver app/prune.py para o criterio conservador (chave de sessao nao viva + idade
    # minima de 7 dias) e o porquê de periodica em vez de so no startup.
    # Renovação de token das contas PARADAS (Task de 18/08). Sem ela, conta que você não abre há
    # dias fica com o accessToken vencido: a cota dela some da faixa do rodapé e, no limite do prazo
    # do refresh (~26 dias), a conta pede login de novo. Abrir a sessão é o que renova — medido.
    renova_task = asyncio.create_task(_renova_token_loop())

    def _renova_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("renova_token.laco crashed", exc_info=exc)

    renova_task.add_done_callback(_renova_done)

    fetch_task = asyncio.create_task(_fetch_loop())

    def _fetch_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("_fetch_loop crashed", exc_info=exc)

    fetch_task.add_done_callback(_fetch_done)

    auto_update_task = asyncio.create_task(_auto_update_loop())

    def _auto_update_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("_auto_update_loop crashed", exc_info=exc)

    auto_update_task.add_done_callback(_auto_update_done)

    prune_task = asyncio.create_task(_prune_loop())

    def _prune_done(t: asyncio.Task) -> None:
        if not t.cancelled():
            exc = t.exception()
            if exc is not None:
                _log.exception("prune.prune_loop crashed", exc_info=exc)

    prune_task.add_done_callback(_prune_done)

    # Boot-resume dos loops: flags em memoria (tick em voo) morrem no restart; o sidecar e a verdade.
    # Loop ACTIVE cuja sessao existe e esta idle -> reagenda o tick; sessao sumida -> failed.
    def _boot_resume_loops() -> None:
        try:
            live = {loop_mod._sanitize(i.name): i for i in registry.list()}
            for p in loop_mod._loop_dir().glob("*.json"):
                stem = p.stem
                link = loop_mod.LoopLink(stem)
                d = link.get()
                if not d or d["status"] not in loop_mod.ACTIVE:
                    continue
                info = live.get(stem)
                if info is None:
                    loop_mod._end(link, stem, "failed", "sessão morta no boot", push.notify_loop)
                    continue
                m = hook_state.get_state(session_key(info.jsonl)) if info.jsonl else None
                if m and m[0] == "idle":
                    loop_mod.schedule_tick(info.name, lambda n=info.name: _loop_ctx(n))
        except Exception:
            _log.warning("boot-resume de loops falhou", exc_info=True)

    await asyncio.to_thread(_boot_resume_loops)
    pricing.atualizar_em_background()  # NUNCA num request: o cliente aborta em 4s
    # Mesmo motivo, outra rede: usd_brl() tem cache de 1h e timeout de 3s, e é chamado DENTRO do
    # montar(). Sem aquecer aqui, o primeiro /api/costs depois de todo restart paga a coleta fria
    # (657ms medidos) MAIS até 3s de câmbio, contra o AbortSignal.timeout(4000) do cliente.
    threading.Thread(target=_usd_brl, name="usd-brl-warm", daemon=True).start()
    # A linha vive no loop do servidor, mas o send_prompt roda em thread — ver pi_inbox.entregar_sync.
    INBOX.ligar_loop(asyncio.get_running_loop())
    # Mesmo motivo, outro caminho: o drain do Codex e assincrono (app-server) e quem o chama sao
    # threads (Timer da confirmacao, gatilho de hook). Ver `_drenar`.
    global _loop_servidor
    _loop_servidor = asyncio.get_running_loop()
    try:
        yield
    finally:
        task.cancel()
        stall_task.cancel()
        prune_task.cancel()
        renova_task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        try:
            await stall_task
        except asyncio.CancelledError:
            pass
        try:
            await prune_task
        except asyncio.CancelledError:
            pass
        # Esperar, e não só cancelar: a rodada de renovação roda em to_thread e abre uma janela
        # tmux que só morre no `finally` dela. Sair sem esperar deixaria a janela órfã justo no
        # restart do backend, que aqui é rotina.
        try:
            await renova_task
        except asyncio.CancelledError:
            pass


app = FastAPI(title="hangar", lifespan=_lifespan)


@app.exception_handler(tmux.MuxIndisponivel)
async def _mux_indisponivel(request: Request, exc: tmux.MuxIndisponivel):
    """503 em QUALQUER rota que esbarre num multiplexador que não responde.

    `registry.list()` levanta isto, e ele é chamado por umas quinze rotas (shell, loop, uploads,
    plano, arquivos...). Tratar rota por rota deixaria as não-tocadas devolvendo 500 com traceback
    justamente no cenário que esta mudança existe pra consertar — e a próxima rota a chamar `list()`
    nasceria com o mesmo furo. Um handler só fecha todas de uma vez.

    503 e não a lista vazia de antes: "não sei quais sessões existem" não pode continuar sendo
    entregue como "você não tem nenhuma".
    """
    diag.registrar("mux.indisponivel", "erro", detalhe=f"{request.method} {request.url.path}")
    return JSONResponse(
        status_code=503,
        content={"detail": erro("erro_mux_indisponivel",
                                "o tmux não respondeu — a lista de sessões está indisponível",
                                detalhe=str(exc))})


@app.middleware("http")
async def _correlaciona_diag(request: Request, call_next):
    """Põe o `X-Hangar-Req` do front no contexto, pra o diário poder LIGAR as duas pontas.

    Sem isto, a linha da tela ("POST /select devolveu 409") e a do servidor ("o cursor do picker não
    convergiu") ficam soltas no arquivo, e amarrar uma na outra depende de comparar horário — que
    empata assim que há duas telas abertas. Com o id, quem analisa segue a cadeia inteira de um
    toque só.
    """
    token = diag.req_atual.set(request.headers.get("x-hangar-req", "")[:32])
    try:
        return await call_next(request)
    finally:
        diag.req_atual.reset(token)


# Body-size ANTES do CORS no codigo -> CORS fica por FORA (envolve ate o 413, adicionando headers CORS
# na rejeicao). Ver _BodySizeLimitMiddleware.
app.add_middleware(_BodySizeLimitMiddleware, max_bytes=MAX_BYTES)
# CORS liberado (token-gated): deixa o app servido por UMA origem (ex: tunnel de casa) falar com o
# backend de OUTRA maquina (ex: trabalho) cross-origin — API via header Bearer, SSE via ?token. Sem
# cookies cross-site (allow_credentials=False), entao "*" e seguro: continua exigindo o token.
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
    allow_credentials=False,
)
if settings.sync:
    app.include_router(sync_router)
app.include_router(deploy_router)
# Roteadores por assunto (Task 1 do plano descoberta-e-configuracao): cada Task do lote escreve
# só no módulo dela. Última edição de api.py deste plano.
app.include_router(alcance.alcance_router)
app.include_router(conta_estado.conta_estado_router)
app.include_router(cotas.cotas_router)
app.include_router(credenciais.credenciais_router)
app.include_router(peers_api.peers_router)
registry = SessionRegistry()
# Peer avisado pela varredura de morte já pode estar ocioso: sem este drain a fila só esvazia no
# próximo hook dele, que pode nunca vir.
registry_mod.apos_saida_por_morte = lambda p: threading.Thread(
    target=_drain_session, args=(p,), daemon=True).start()
terminal = TerminalInput()

# Teto de mensagem: o _BodySizeLimitMiddleware ignora scope != http de propósito (api.py:83), então
# a rota WebSocket nasceria sem limite nenhum. 256 KiB cobre com folga qualquer texto de chat.
_WS_MAX = 256 * 1024
# Heartbeat DO SERVIDOR, não eco do cliente: socket aberto não prova que tem alguém lendo (extensão
# com laço travado, notebook suspenso). Sem isto, uma linha zumbi faria toda mensagem pagar o
# PRAZO_ACK inteiro antes de cair pro fallback — a lentidão que o caminho de tecla não tinha.
_WS_PING = 20.0
# Teto de heartbeats SEGUIDOS sem resposta antes de fechar. `send_json` sozinho não pega o zumbi que
# o comentário acima descreve: com o laço de eventos travado (ou notebook suspenso), o buffer TCP do
# SO absorve o ping sem erro nenhum — é exatamente esse caso que o heartbeat existe pra cobrir. Dois
# pings perdidos (~2×_WS_PING) é rápido o bastante pra não atrasar a decisão "linha ou tecla" da
# Task 4, e generoso o bastante pra não fechar por uma rajada de latência isolada.
_WS_PINGS_SEM_RESPOSTA_MAX = 2


# Aviso-uma-vez-ate-mudar da recusa de conexao (achado ALTA da revisao 02/08/2026): sem isto, um
# token girado / bind mudado / firewall no meio faz TODA tentativa de retry da extensao (laco com
# recuo, hangar-state.ts) virar linha de log — e a mesma enxurrada que o retry em si tenta evitar do
# lado dela. Mesma politica de terminal_input._avisa_deferred/_limpa_deferred: WARNING na 1a recusa
# por host, calado ate uma conexao daquele host DAR CERTO (o que também reabre o aviso se a falha
# voltar depois — nao e "avisa uma vez na vida do processo").
_ws_origem_avisada: set[str] = set()
_ws_token_avisado: set[str] = set()


# Única rota WebSocket do backend. Quem LIGA é a extensão do Pi; o backend nunca procura ninguém —
# é isso que faz o custo ser zero pra quem não tem Pi.
# Auth pela query e não por header: é o mesmo caminho que o SSE já usa (auth.py:86-94), e cliente
# WebSocket não manda Authorization de forma portável.
@app.websocket("/api/pi/inbox")
async def pi_inbox_ws(ws: WebSocket):
    host = ws.client.host if ws.client else ""
    # bind_host: o endereco em que o PROPRIO uvicorn subiu (resolve_bind_ip == main.py). Com
    # CP_LAN_BIND_IP=auto ou IP fixo de LAN (modo celular documentado), o processo nao escuta em
    # loopback -- so aceitar 127.0.0.1 fechava a linha do Pi em silencio pra sempre (achado da
    # revisao final). Aceitar TAMBEM o bind continua seguro: uma conexao TCP com origem igual ao
    # endereco que o proprio host bindou so acontece self-connect (a mesma maquina falando com uma
    # interface dela mesma) -- um host remoto na LAN nunca aparece aqui com ESSE endereco de
    # origem, porque a origem eh o IP DELE, nao o do servidor (TCP nao deixa forjar isso).
    if host not in ("127.0.0.1", "::1", "localhost", resolve_bind_ip(settings)):
        # A defesa real é esta. Em loopback o token não protege de quem já está logado na máquina
        # (o próprio auth.py:42-46 registra isso), mas conexão de FORA não tem o que fazer aqui.
        # Achado ALTA da revisao 02/08/2026: ate aqui a recusa era MUDA — nem no terminal do Pi nem
        # no log do backend sobrava rastro de por que a linha rapida nunca ligava.
        if host not in _ws_origem_avisada:
            _ws_origem_avisada.add(host)
            _log.warning("pi_inbox: origem recusada host=%s (fora do bind aceito) — linha do Pi "
                         "vai continuar caindo pra tecla (aviso unico ate mudar)", host)
        await ws.close(code=1008)
        return
    if not settings.auth_token or not secrets.compare_digest(
            ws.query_params.get("token", ""), settings.auth_token):
        # Mesmo achado: inconsistente com auth.py:75, que loga toda virada de bloqueio de token —
        # aqui nao logava NADA. Token girado / sidecar desatualizado ficava indistinguivel de
        # "extensao nao instalada".
        if host not in _ws_token_avisado:
            _ws_token_avisado.add(host)
            _log.warning("pi_inbox: token recusado host=%s — linha do Pi vai continuar caindo pra "
                         "tecla (aviso unico ate acertar)", host)
        await ws.close(code=1008)
        return
    # Conectou: qualquer recusa ANTERIOR deste host era um estado velho — se voltar a falhar depois,
    # merece aviso de novo (nao e "avisou uma vez na vida do processo, calado pra sempre").
    _ws_origem_avisada.discard(host)
    _ws_token_avisado.discard(host)
    await ws.accept()
    pane = ""
    linha = None
    try:
        # Texto cru primeiro, igual ao loop abaixo: receive_json() direto pularia o teto de
        # tamanho pra ESTA mensagem (achado da revisão — só as do loop passavam pelo len(bruto)).
        bruto = await asyncio.wait_for(ws.receive_text(), _WS_PING)
        if len(bruto) > _WS_MAX:
            _log.warning("pi_inbox: primeira mensagem de %d bytes — fechando", len(bruto))
            await ws.close(code=1009)
            return
        primeira = json.loads(bruto)
        # `chave` e o que a extensao declara como identidade da sessao: o nome do psmux quando
        # existe, o pane quando nao (tmux). Extensao ANTIGA nao manda o campo e cai no pane, que e
        # exatamente o comportamento de antes — ninguem precisa dar /reload pra continuar
        # funcionando no Linux. Ver pi_inbox: no psmux o pane e `%1` em TODA sessao, e por isso a
        # linha da segunda sessao Pi tomava o lugar da primeira.
        pane = str(primeira.get("chave") or primeira.get("pane") or "")
        if not pane:
            await ws.close(code=1008)
            return
        linha = INBOX.registrar(pane, ws.send_json)
        _log.info("pi_inbox: linha aberta chave=%s", pane)
        pings_sem_resposta = 0
        while True:
            try:
                bruto = await asyncio.wait_for(ws.receive_text(), _WS_PING)
            except asyncio.TimeoutError:
                # Silêncio: cobra sinal de vida. Se o socket estiver morto, o send levanta e a
                # linha cai aqui mesmo, em vez de ficar registrada como viva pra sempre. Mas o send
                # sozinho não pega o zumbi (buffer do SO absorvendo o ping) — daí o contador: sem
                # NENHUMA resposta por _WS_PINGS_SEM_RESPOSTA_MAX rodadas, fecha por conta própria.
                pings_sem_resposta += 1
                if pings_sem_resposta > _WS_PINGS_SEM_RESPOSTA_MAX:
                    _log.warning("pi_inbox: %d heartbeats sem resposta pane=%s — linha zumbi, "
                                 "fechando", pings_sem_resposta, pane)
                    await ws.close(code=1000)
                    return
                await ws.send_json({"ping": True})
                continue
            # Qualquer mensagem é sinal de vida, não só o pong: zera antes de olhar o conteúdo.
            pings_sem_resposta = 0
            if len(bruto) > _WS_MAX:
                _log.warning("pi_inbox: mensagem de %d bytes pane=%s — fechando", len(bruto), pane)
                await ws.close(code=1009)
                return
            msg = json.loads(bruto)
            if msg.get("pong") or msg.get("ping"):
                continue
            msg_id = str(msg.get("id") or "")
            if not msg_id:
                continue
            if "resposta" in msg:
                # Resposta de PERGUNTA (leitura), nao confirmacao de entrega — chaves diferentes
                # de proposito: a extensao responde `{id, resposta}` e nunca `ok`, entao uma
                # mensagem jamais cai nos dois caminhos. `None` explicito (a extensao dizendo "nao
                # sei") chega como None e o backend cai no plano B; string vazia e resposta.
                valor = msg.get("resposta")
                INBOX.responder(pane, msg_id, valor if isinstance(valor, str) else None)
                continue
            INBOX.confirmar(pane, msg_id, bool(msg.get("ok")), msg.get("erro"))
    except WebSocketDisconnect:
        pass
    except Exception as e:
        _log.warning("pi_inbox: linha caiu pane=%s: %r", pane, e)
    finally:
        if linha is not None:
            INBOX.remover(pane, linha)


@app.websocket("/api/sessions/{name}/term")
async def term_ws_route(ws: WebSocket, name: str):
    # Sem a trava de loopback do /api/pi/inbox: aquela existe porque quem liga la e uma extensao
    # LOCAL. Aqui o celular vai precisar entrar de fora na fase 2.
    from app import termsock
    await termsock.term_ws(ws, name)


@app.post("/api/sessions/{name}/shell", dependencies=[Depends(require_auth)])
def abrir_shell(name: str):
    # Sessao de shell SEPARADA e ESCONDIDA do app (Task 6) -- ver tmux.new_hidden_shell. Sync (nao
    # async): mesmo padrao das rotas POST vizinhas (select/answer acima), que resolvem a sessao via
    # `registry.list()` direto e deixam o FastAPI rodar o handler bloqueante no threadpool.
    from app import tmux
    info = _cached_info_sync(name)
    if info is None:
        raise HTTPException(status_code=404, detail=erro("erro_sessao_inexistente", "sessao nao existe"))
    alvo = f"term-{name}"
    # Achado da revisao (I1, e de novo na rodada 2): `sanitize_session_name` aceita hifen, entao
    # "term-<nome>" pode ja existir como sessao de TERCEIRO (ex: usuario criou "foo" e depois
    # "term-foo" na mao). A 1a versao inferia isso de `registry.list()` -- mas a lista TAMBEM
    # filtra sessao com sidecar Codex de mesmo nome, entao um "term-<nome>" que fosse Codex de
    # verdade sumia da lista por ESSE motivo, nao por ser nosso, e a inferencia concluia (errado)
    # que o nome estava livre. Pergunta DIRETA ao tmux (`is_hidden`), nao mais inferida: cobre
    # Codex tambem, ao custo de 1-2 forks a mais nesta rota de clique unico (nao e o caminho de
    # poll onde fork por sessao e proibido).
    if tmux.has_session(alvo) and not tmux.is_hidden(alvo):
        # L68 da revisao final: o texto NAO afirma mais que a sessao e de terceiro. Ela pode ser o
        # shell DESTE painel que ficou sem a marca (um `set-option` que falhou por tmux ocupado/
        # timeout, ver tmux.new_hidden_shell) -- e como este gate recusa ANTES de chamar aquela
        # funcao, nada se autocorrige sozinho: quem desempata e o usuario.
        raise HTTPException(status_code=409,
                            detail=erro("erro_sessao_tmux_em_uso",
                                        f"ja existe uma sessao tmux chamada {alvo!r} sem a marca do "
                                        "painel -- pode ser uma sessao sua de mesmo nome, ou o shell "
                                        "deste painel que perdeu a marca. Encerre ou renomeie essa "
                                        "sessao antes de abrir o shell", nome=alvo))
    # O cwd vem do REGISTRY, nunca da query: um `?cwd=/` viraria shell em qualquer lugar do disco.
    novo = tmux.new_hidden_shell(name, info.cwd or str(Path.home()))
    if novo is None:
        raise HTTPException(status_code=500, detail=erro("erro_shell_criacao_falhou", "tmux recusou criar o shell"))
    return {"ok": True, "shell": novo}


# Emuladores de terminal conhecidos, na ordem de preferencia da sonda quando CP_TERMINAL nao esta
# setado. Cada valor monta o ARGV completo de attach dado o alvo tmux exato ("=nome:" -- NUNCA sem
# o `=`, senao o tmux cai em prefix-match e abre a sessao errada; o `:` final e a mesma grafia do
# `attach` do termsock, alinhada na revisao final -- medido nesta maquina que as duas formas
# anexam igual, e uma operacao so nao pode ter duas grafias numa branch inteira sobre esse
# detalhe). `wezterm` nao tem `-e`: e
# `start -- comando`. `gnome-terminal -e` esta deprecado e so aceita UM string; `--` e o substituto.
_EMULADORES = {
    "wezterm": lambda alvo: ["wezterm", "start", "--", "tmux", "attach", "-t", alvo],
    "kitty": lambda alvo: ["kitty", "tmux", "attach", "-t", alvo],
    "alacritty": lambda alvo: ["alacritty", "-e", "tmux", "attach", "-t", alvo],
    "konsole": lambda alvo: ["konsole", "-e", "tmux", "attach", "-t", alvo],
    "gnome-terminal": lambda alvo: ["gnome-terminal", "--", "tmux", "attach", "-t", alvo],
    "xterm": lambda alvo: ["xterm", "-e", "tmux", "attach", "-t", alvo],
}
_ORDEM_PROBE = ["wezterm", "kitty", "alacritty", "konsole", "gnome-terminal", "xterm"]


@app.post("/api/sessions/{name}/open-terminal", dependencies=[Depends(require_auth)])
def abrir_terminal_nativo(name: str):
    """Abre um emulador de terminal NATIVO (janela propria do SO) anexado a sessao tmux `name` --
    tanto a do agente quanto a do shell escondido, o alvo e so um nome de sessao tmux. Diferente do
    painel embutido (termsock/xterm.js): esta janela nao depende do backend pra existir, entao
    fechar o painel ou reiniciar o servico NAO a desanexa.

    Checa via `tmux.has_session` (nao `registry.list()`): a sessao de shell escondida (Task 6) NAO
    aparece no registry por design, mas continua um alvo valido pra este botao.
    """
    from app import tmux
    if not tmux.has_session(name):
        raise HTTPException(status_code=404, detail=erro("erro_sessao_inexistente", "sessao nao existe"))
    nome_bin = os.environ.get("CP_TERMINAL")
    if nome_bin:
        # env checada ANTES do PATH: se o usuario apontou um emulador, e ele que vale -- so falha
        # se esse binario especifico nao existir ou nao for suportado (dicionario fechado; NAO
        # inventa um `-e` generico pra emulador desconhecido).
        if nome_bin not in _EMULADORES or shutil.which(nome_bin) is None:
            raise HTTPException(status_code=503,
                                detail=erro("erro_terminal_invalido",
                                            f"CP_TERMINAL={nome_bin!r} nao encontrado no PATH ou nao "
                                            "suportado", nome=nome_bin))
    else:
        nome_bin = next((n for n in _ORDEM_PROBE if shutil.which(n)), None)
        if nome_bin is None:
            raise HTTPException(status_code=503,
                                detail=erro("erro_terminal_ausente", "nenhum emulador de terminal encontrado no PATH"))
    args = tmux._scope_prefix() + _EMULADORES[nome_bin](f"={name}:")
    env = os.environ.copy()
    wl = tmux._wayland_display()
    if wl:
        # sem isto o emulador GUI nao acha o compositor quando o backend roda como servico systemd
        # (env de boot, sem WAYLAND_DISPLAY) -- mesmo problema que o wl-paste do new_session.
        env["WAYLAND_DISPLAY"] = wl
    disp = os.environ.get("DISPLAY")
    if disp:
        # Achado da revisao (I5): X11/XWayland precisa de DISPLAY, nao so WAYLAND_DISPLAY -- sem
        # repassar, um host X11 (ou o servico systemd sem env de sessao grafica) faz o emulador
        # executar e morrer com "cannot open display" LOGO apos o exec, onde o Popen nao pega nada.
        env["DISPLAY"] = disp
    # Arquivo temporario, nao `PIPE` (achado da revisao, rodada 2): a janela que ABRE fica viva
    # bem alem deste request, e um `PIPE` sem leitor enche os 64KB do buffer do kernel e a
    # escrita do emulador TRAVA (medido no wezterm, primeiro da sonda, que loga bastante em
    # stderr) -- mais um fd vazado por clique. Arquivo comum nao tem esse teto.
    err_file = tempfile.TemporaryFile()
    try:
        p = subprocess.Popen(args, env=env, start_new_session=True, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=err_file)
    except OSError as e:
        err_file.close()
        raise HTTPException(status_code=503, detail=erro("erro_terminal_abertura_falhou", f"falha ao abrir o emulador de terminal: {e}", erro=str(e)))
    # Falha aparece, nao some (achado da revisao): o Popen so levanta se o BINARIO nao existe --
    # sem DISPLAY, com o compositor errado, ou qualquer erro pos-exec, o processo sai sozinho em
    # poucos ms e o `except OSError` acima nunca ve nada, devolvendo "ok" pra uma janela que nunca
    # abriu. Espera uma fracao de segundo e confere se ja morreu.
    time.sleep(0.3)
    morreu = p.poll()
    # `morreu != 0`, nao so `morreu is not None` (achado da revisao, rodada 2): sair com rc=0 em
    # poucos ms e COMPORTAMENTO NORMAL de cliente D-Bus/instancia unica -- `gnome-terminal` (na
    # sonda) abre no `gnome-terminal-server` e sai 0 na hora; `wezterm start` com GUI ja de pe e
    # `konsole` reusando instancia fazem o mesmo. Tratar qualquer saida como erro devolvia 503 pra
    # janela que abriu certo.
    if morreu is not None and morreu != 0:
        err_file.seek(0)
        saida = err_file.read().decode(errors="replace").strip()
        err_file.close()
        raise HTTPException(status_code=503,
                            detail=erro("erro_terminal_saiu_cedo",
                                        f"emulador de terminal saiu logo apos abrir: "
                                        f"{saida or f'codigo {morreu}'}",
                                        saida=saida or f"codigo {morreu}"))
    err_file.close()   # nosso handle; o filho, se ainda vivo, segue escrevendo no fd dele
    # Este `Popen` nunca e colhido explicitamente (sem wait(), sem thread de reaper): a janela vive
    # muito alem deste request e esperar por ela seria travar a rota. Quem colhe e o proprio
    # `subprocess`, que varre os filhos ja mortos a cada Popen novo — e o backend chama `tmux` o
    # tempo todo, entao o zumbi some sozinho em segundos. E uma DEPENDENCIA de detalhe interno do
    # modulo, nao um contrato: se um dia o backend parar de disparar subprocessos com frequencia,
    # cada clique aqui deixa um zumbi ate o processo reiniciar.
    return {"ok": True}


# Snapshot com TTL de registry.list() pros endpoints request/response QUENTES (history/workflows):
# o mount do board dispara dezenas de /history de uma vez e cada list() fresco e um scan completo
# de /proc + fork de tmux list-panes. Mesmo padrao do sse._list_snap (la pros loops de SSE; caches
# separados porque as instancias de SessionRegistry sao separadas). Miss por nome (sessao criada ha
# <1s) -> fallback pro list() fresco, entao o TTL nunca causa 404 falso.
_LIST_TTL = 1.0
# UMA chave, guardando o par (quando, lista). Guardar `t` e `infos` em chaves separadas deixava as
# duas escritas se entrelacarem entre threads — e desde que o `_cached_info` async passou a entrar
# por aqui via to_thread, sao threads de verdade, nao mais so o laco de eventos. O pior caso era
# pequeno (a lista de uma thread carimbada com o relogio da outra, dezenas de ms a mais de atraso),
# mas o par num STORE_SUBSCR so custa o mesmo e nao deixa a pergunta em aberto.
_list_snap: dict = {"snap": None}


def _guardar_snap() -> list[SessionInfo]:
    infos = registry.list()
    _list_snap["snap"] = (time.monotonic(), infos)
    return infos


def _cached_info_sync(name: str) -> SessionInfo | None:
    """Gemeo SINCRONO do _cached_info, pro handler `def` (que o FastAPI ja roda na threadpool e
    portanto pode chamar registry.list() direto). Mesmo dicionario dos dois lados: um hit vindo de
    qualquer caminho serve o outro. Duas threads podem recarregar ao mesmo tempo — inofensivo, a
    recarga e idempotente e a ultima vence."""
    snap = _list_snap["snap"]
    infos = (snap[1] if snap is not None and time.monotonic() - snap[0] < _LIST_TTL
             else _guardar_snap())
    info = next((s for s in infos if s.name == name), None)
    if info is None:
        info = next((s for s in _guardar_snap() if s.name == name), None)
    return info


async def _cached_info(name: str) -> SessionInfo | None:
    return await asyncio.to_thread(_cached_info_sync, name)


def _notify_async(session_id: str, send_fn) -> None:
    """Resolve uuid->nome e manda o push escolhido, TUDO numa thread: registry.list() mexe no tmux
    e o webpush e rede — nada disso pode bloquear o loop do watch."""
    def _work() -> None:
        try:
            name = next(
                (s.name for s in registry.list() if s.jsonl and session_key(s.jsonl) == session_id),
                None,
            )
            if name:
                send_fn(name)
        except Exception:
            _log.warning("push falhou (%s)", session_id, exc_info=True)
    threading.Thread(target=_work, daemon=True).start()


def _awaiting_body(info) -> str:
    """Corpo rico da notif de awaiting (feature #5): 1) a pergunta do AskUserQuestion nativo (sidecar
    gravado pelo hook PreToolUse); 2) senao a pergunta lida do PANE (classify — cobre pickers/permissao
    da TUI, que nao passam pelo AskUserQuestion); 3) None se nenhuma deu certo — o push (app.push)
    resolve o fallback no idioma da inscricao, em vez de mandar texto fixo em pt."""
    askq = read_pending_askq(info.jsonl) if info.jsonl else None
    if askq and askq.questions:
        return askq.questions[0].question
    if info.name:
        from app import tmux
        from app.state import classify
        try:
            _, _, question, _ = classify(tmux.capture_pane(info.name))
        except Exception:
            question = None
        if question:
            return question
    return None  # fallback resolvido pelo push, no idioma da inscricao


_AWAITING_PUSH_RETRY_S = 1.5  # Notification chega junto do pedido; o menu pode atrasar um frame


def _pane_wants_input(name: str) -> bool:
    """Pane mostra menu (classify awaiting) ou overlay de teclas — algo REAL esperando resposta.
    Falha de captura -> True (erro de leitura nao pode segurar um push legitimo)."""
    from app import tmux
    from app.state import classify, is_overlay
    try:
        pane = tmux.capture_pane(name)
    except Exception:
        _log.warning("_pane_wants_input falhou name=%s", name, exc_info=True)
        return True
    return classify(pane)[0] == "awaiting_input" or is_overlay(pane)


def _do_notify_awaiting(session_id: str) -> None:
    """Logica sincrona de _on_awaiting: resolve nome+corpo rico e manda pro push (que decide
    mute/quiet-hours/coalescing). Extraida da thread pra ficar testavel direto, sem mockar Thread.

    Gate anti-fantasma: o state_hook mapeia QUALQUER Notification pra awaiting — inclusive a de
    "idle ha 60s" do Claude Code, que chega DEPOIS do Stop com a sessao apenas parada. Sem o gate,
    toda sessao parada >60s empurrava push falso "Aguardando sua resposta". Push so sai com awaiting
    REAL: askq pendente no sidecar OU menu/overlay no pane (retry curto cobre o frame de render)."""
    info = next((s for s in registry.list() if s.jsonl and session_key(s.jsonl) == session_id), None)
    if info is None:
        return
    def _real() -> bool:
        askq = read_pending_askq(info.jsonl) if info.jsonl else None
        return bool(askq and askq.questions) or _pane_wants_input(info.name)

    real = _real()
    if not real:
        time.sleep(_AWAITING_PUSH_RETRY_S)
        real = _real()  # re-le askq TAMBEM: o sidecar pode ser o que atrasou, nao so o pane
    if real:
        push.notify_awaiting(info.name, _awaiting_body(info))


def _on_awaiting(session_id: str) -> None:
    """hook_state -> transicao awaiting_input. Roda numa thread (registry.list mexe no tmux; resolver
    o corpo toca pane/disco) — nada disso pode bloquear o loop do watch."""
    def _work() -> None:
        try:
            _do_notify_awaiting(session_id)
        except Exception:
            _log.warning("push awaiting falhou (%s)", session_id, exc_info=True)
    threading.Thread(target=_work, daemon=True).start()


_CONFIRM_GRACE = 8.0  # s entre o send e a checagem "o transcript gravou o prompt?"
# Kimi: o mesmo prazo, esticado. Ali nao existe segunda tentativa (redigitar duplicaria a msg), e
# sem segunda chance o prazo tem que ser generoso — senao ruido de timing carimba `desistiu` numa
# msg que so estava esperando a vez na fila da propria TUI.
_CONFIRM_GRACE_KIMI = 30.0
# Kimi: de quanto em quanto tempo reavaliar um "idle" que o transcript desmentiu. Nao ha evento pra
# esperar (o fim de turno real grava idle sobre idle e nao gera transicao), entao a saida e reolhar.
# 5s: a sessao demora isso pra aparecer parada, e enquanto o turno anda o custo e um getmtime.
_RECHECA_KIMI = 5.0


# Loop do servidor, pra pontes sync->async (mesmo papel do `INBOX.ligar_loop`). Setado no lifespan.
_loop_servidor: asyncio.AbstractEventLoop | None = None


def _drenar(name: str, jsonl: str, provider: str) -> int:
    """Entrega a fila pendente pelo caminho DAQUELE provider.

    O `terminal_input.drain` digita no pane, e no Codex isso poria a mensagem do usuario duas vezes
    na conversa (a entrega de verdade e o `turn/start` do app-server). Como o adapter do Codex e
    assincrono e quem chama isto e sempre uma thread (Timer, hook, request fora do loop), a ponte e
    a mesma do `pi_inbox.entregar_sync`: agendar no loop do servidor e esperar o resultado."""
    if provider != "codex":
        return drain(name, jsonl, provider)
    loop = _loop_servidor
    if loop is None:
        _log.warning("drain codex name=%s: sem loop do servidor (fila fica pendente)", name)
        return 0
    fut = None
    try:
        fut = asyncio.run_coroutine_threadsafe(get_adapter("codex").drain(name, jsonl), loop)
        # Teto so pra nao pendurar a thread se o loop morrer no meio (restart): quem manda no
        # relogio e o proprio adapter, que ja tem os timeouts do app-server.
        return fut.result(120)
    except Exception:
        # `cancel()` pelo mesmo motivo do `pi_inbox.entregar_sync`: sem ele a corrotina segue viva
        # no loop e pode ENTREGAR depois de este chamador ja ter decidido "ficou pendente" — a
        # proxima rodada entrega de novo e a mesma mensagem chega duas vezes ao agente. So tem
        # efeito se ela ainda nao passou do proximo await; passou disso, a fila ja esta marcada.
        # `fut` continua None se o proprio run_coroutine_threadsafe levantar (loop fechando).
        if fut is not None:
            fut.cancel()
        # Falha VISIVEL, nunca mensagem duplicada: a entrada segue pendente e o proximo fim de
        # turno tenta de novo; a bolha "na fila" continua na tela enquanto isso.
        _log.warning("drain codex name=%s falhou (fila segue pendente)", name, exc_info=True)
        return 0


def _drain_session(name: str) -> None:
    """Entrega enfileiradas pendentes desta sessao (best-effort, roda fora do request)."""
    try:
        info = _cached_info_sync(name)
        if info and info.jsonl:
            _drenar(name, info.jsonl, info.provider)
    except Exception:
        pass


def _confirm_and_drain(name: str) -> None:
    """Confirmacao de entrega: delivered=True so diz 'send_keys chamado' — a TUI pode ter engolido
    as teclas e a msg sumia com cara de entregue. Confere contra o transcript; engolida ->
    re-enfileira (reconcile) e re-drena. Best-effort, roda em Timer/thread."""
    try:
        q = PromptQueue(name)
        if not any(r.get("delivered") is True and not r.get("confirmed") for r in q.load()):
            return  # nada a confirmar: nao paga registry nem o scan do transcript
        info = _cached_info_sync(name)
        if not info or not info.jsonl:
            return
        # MID-TURN o prompt entregue ainda pode nao ter virado entrada no transcript (vive na fila
        # interna do Claude Code) — decidir requeue agora arriscaria redigitar mensagem ja recebida.
        # Adia pro proximo ciclo (o turno acabando dispara transicao -> novo timer).
        m = hook_state.get_state(session_key(info.jsonl))
        # No Kimi o marcador mente pra baixo (idle congelado do turno anterior, ver
        # state.corrige_ocioso_kimi). Sem corrigir AQUI, o guard de mid-turn abaixo nunca segurava
        # nada nesse provider — e ai toda entrega virava `desistiu` 8s depois do envio, mesmo a que
        # so estava esperando a vez na fila da propria TUI.
        if m and info.provider == "kimi":
            m = corrige_ocioso_kimi(m, info.jsonl)
        # Kimi espera MAIS antes de declarar perdida (30s contra 8s): ver o comentario no else.
        grace = _CONFIRM_GRACE_KIMI if info.provider == "kimi" else _CONFIRM_GRACE
        # UMA leitura do oraculo pros dois ramos. None = nao deu pra ler o transcript (ver
        # committed_user_lines): sai SEM decidir e SEM reagendar. Sem reagendar de proposito — um
        # Timer a cada `grace` contra um arquivo que nao abre e tempestade sem fim; o proximo fim
        # de turno (`_on_hook_transition`) ou a proxima mensagem chamam isto de novo, e ate la a
        # entrada fica visivel como bolha "na fila". Falha VISIVEL, nunca mensagem duplicada.
        # As DUAS leituras do transcript ficam juntas e falham juntas. `_transcript_start_ts` abre
        # o mesmo arquivo uma segunda vez, e o 0.0 dele desliga a poda por idade — sem ela, entrada
        # de sessao ANTERIOR nao e mais dispensada e vai parar no caminho que REDIGITA. Ou seja: o
        # mesmo defeito, pela porta do lado. Aqui "nao sei" nunca decide nada.
        committed = committed_user_lines(info.jsonl, info.provider)
        inicio_ts = _transcript_start_ts(info.jsonl)
        if committed is None or inicio_ts is None:
            _log.warning("confirmacao adiada name=%s: transcript ilegivel agora (nada foi "
                         "reenfileirado nem dado por perdido)", name)
            return
        if m and m[0] == "working":
            # Turno vivo: REDIGITAR e DESISTIR no meio do turno sao perigosos (o texto pode ainda
            # estar na fila interna da TUI — desistiu viraria aviso falso de "nao chegou" sobre
            # msg que chega depois). CONFIRMAR nao: o transcript e a fonte de verdade, e texto
            # comprovadamente la = a bolha real ja cobre, o eco da fila so duplica. Sem isto, uma
            # sessao que trabalha HORAS sem ficar ociosa nunca confirmava e o follow reemitia a
            # fila inteira como bolha fantasma a cada reconexao do SSE. `confirm_only` carimba so
            # o provado e deixa o resto pra proxima checagem (reagendada la embaixo).
            q.reconcile_delivered(
                committed, inicio_ts,
                time.time(),
                grace=grace,
                confirm_only=True,
            )
        else:
            # Estado DESCONHECIDO (marcador ausente): nao da pra provar que a sessao nao esta no meio de
            # um turno — e redigitar e a acao destrutiva daqui (mete texto num prompt em uso). Entao
            # confirma sem NUNCA redigitar (max_attempts=0). O caso real e sessao RESSUSCITADA: o
            # kill-server de 2026-08-11 13:55 matou o tmux, a sessao voltou por `claude --resume` e a
            # fila duravel (arquivo por NOME) sobreviveu ao pane — o guard acima caiu pra frente com
            # get_state()=None e o backend redigitou dentro do turno vivo (log REQUEUE 14:01:48).
            # Pior caso agora = comportamento antigo: envio engolido fica visivel como bolha da fila,
            # que e falha VISIVEL. Duplicar a msg do usuario nao e.
            # Kimi NUNCA redigita. Prompt digitado durante um turno fica na fila da TUI e so entra no
            # wire.jsonl quando o turno chega nele — nao ha o equivalente do `queue-operation` do Claude
            # Code, que e o registro feito NO MOMENTO da digitacao. Entao, no Kimi, "ausente do
            # transcript" nao prova engolido, e redigitar e a acao destrutiva. Some a isso o marcador de
            # estado dizer "ociosa" no meio do turno (o Stop do SUBAGENTE grava na chave do pai) e o
            # guard de working acima nao segura nada: medido em 13/08/2026, a mesma mensagem entrou 3x na
            # fila de uma sessao Kimi (REQUEUE n=3 no log das 08:29). Pior caso agora e o mesmo aceito
            # logo acima pro estado desconhecido: envio de verdade engolido fica VISIVEL como bolha da
            # fila (`desistiu`), que e falha visivel — duplicar a msg do usuario nao e.
            max_attempts = 0 if (m is None or info.provider == "kimi") else 2
            # Kimi espera MAIS antes de declarar perdida: com max_attempts=0 nao ha segunda chance — a
            # primeira checagem depois do prazo ja carimba `desistiu`. Subir pra 1 nao serve (no
            # reconcile, attempts < max REDIGITA, a duplicacao que este provider nao pode ter). Entao
            # o que se estica e o PRAZO (grace=30s): cobre o tempo entre a TUI aceitar o texto e ele
            # aparecer no wire.jsonl, sem nunca digitar duas vezes.
            requeued = q.reconcile_delivered(
                committed, inicio_ts,
                time.time(),
                grace=grace,
                max_attempts=max_attempts,
            )
            if requeued:
                # Log com o TEXTO e com a linha mais parecida do transcript. `REQUEUE name=X n=1`
                # sozinho nao diz nada: o oraculo e comparacao de string, entao o que resolve o
                # caso e o DIFF (um espaco a mais, uma barra invertida comida pelo multiplexador,
                # um prefixo prependado pelo harness). E redigitar e a acao destrutiva daqui —
                # quando ela sai errada, o usuario ve a propria mensagem entrar 3x na conversa,
                # e sem estas duas linhas so restava reler o codigo.
                for r in requeued:
                    txt = str(r.get("text") or "").strip()
                    _log.info("REQUEUE name=%s id=%s tentativa=%s texto=%r | mais parecida no "
                              "transcript=%r", name, r.get("id"), r.get("attempts"), txt[:200],
                              (linha_mais_parecida(txt, committed) or "")[:200])
                _log.info("REQUEUE name=%s n=%d (TUI engoliu o send; re-drenando)", name, len(requeued))
                _drenar(name, info.jsonl, info.provider)
        # Sobrou entrada AINDA DENTRO do prazo (o reconcile a pulou por "recente demais")? Volta a
        # olhar. Os agendamentos usam _CONFIRM_GRACE (8,5s) e o prazo do Kimi e 30s, entao num turno
        # curto a unica checagem caia cedo demais e a entrada ficava sem confirmar E sem desistir —
        # presa ate a proxima mensagem do usuario, ou pra sempre se nao houvesse proxima. O laco
        # termina sozinho: passado o prazo, toda linha vira `confirmed` ou `desistiu`.
        if any(r.get("delivered") is True and not r.get("confirmed") and not r.get("desistiu")
               for r in q.load()):
            threading.Timer(grace + 0.5, _confirm_and_drain, args=(name,)).start()
    except Exception:
        # LOGA, nao `pass` mudo: isto roda num Timer, entao ninguem ve a excecao — e o que mora
        # aqui e a confirmacao de entrega. Falhando calado, a msg do usuario fica sem confirmar pra
        # sempre e nao ha onde olhar. Best-effort segue (o proximo idle tenta de novo).
        _log.warning("confirmacao de entrega falhou name=%s", name, exc_info=True)


def _maybe_chain(name: str) -> None:
    """Encadeamento de sessao (feature #12): `name` acabou de confirmar idle no MESMO ponto do push de
    'terminou' (state == 'idle' em _on_hook_transition — so vira idle no hook Stop, entao ja e turno
    REALMENTE terminado, nao redraw). Se ha um vinculo 'then' armado, enfileira o prompt na sessao ALVO
    e dispara o drain dela (mesmo mecanismo do /input), depois consome o vinculo (one-shot: nao e DAG,
    so 1 hop -- sem isto o alvo levaria o MESMO prompt de novo no proximo turno da fonte).
    Kill-switch mestre no topo (app.config.automations_enabled) -- desliga esta e a auto-resume junto."""
    if not automations_enabled():
        return
    link = ThenLink(name)
    data = link.get()
    if not data:
        return
    target, text = data.get("target"), data.get("text")
    try:
        if target and text:
            PromptQueue(target).append(text, delivered=False)
            _drain_session(target)
    finally:
        link.clear()  # one-shot sempre, mesmo se target/text vier malformado -> nao fica repetindo lixo


_working_started: dict[str, float] = {}  # session_id -> ts de quando entrou em "working" (mede duracao do turno pro push de "terminou")
# Lock PROPRIO do `_working_started`, separado do da cadeia de recheck: quem escreve ali roda no laco
# do hook_state.watch e quem le/consome roda numa thread de `_work`. Um compare-and-delete protegido
# so de um lado nao protege nada — o produtor concorrente passaria por cima entre o get e o del, e o
# turno seguinte terminaria com `started is None`, sem aviso e sem rastro. Nunca aninhar com
# `_recheca_lock`: sao independentes de proposito.
_turno_lock = threading.Lock()
# Sessoes com uma reavaliacao de "idle mentiroso" ja agendada. UMA cadeia por sessao: sem isto, cada
# transicao espuria abria a sua propria corrente de Timers, e duas correntes em paralelo dobram o
# `registry.list()` (que toca tmux) a cada 5s, sem limite e sem ninguem notar.
_recheca_armada: set[str] = set()
_recheca_lock = threading.Lock()


# Tentativas seguidas com FALHA antes de abandonar a cadeia de reavaliacao de uma sessao.
_RETRY_FALHA = 5
_falhas_seguidas: dict[str, int] = {}


def _armar_recheca(session_id: str) -> bool:
    """True se ESTA chamada ficou dona da cadeia de reavaliacao da sessao."""
    with _recheca_lock:
        if session_id in _recheca_armada:
            return False
        _recheca_armada.add(session_id)
        return True


def _recheca_kimi(session_id: str, state: str) -> None:
    """Elo da cadeia: solta a posse ANTES de reavaliar, pra a proxima passada poder rearmar. Soltar
    depois prenderia a cadeia numa unica corrente que morre junto com uma excecao."""
    with _recheca_lock:
        _recheca_armada.discard(session_id)
    _on_hook_transition(session_id, state)


def _push_terminou(session_id: str, started: Optional[float]) -> None:
    """Push de 'terminou': avisa se o turno que comecou em `started` passou do minimo configurado.

    Mora numa funcao propria porque o disparo saiu do caminho sincrono — no Kimi so o `_work` sabe
    se o 'idle' que chegou e de verdade, e avisar 'terminou' no meio do trabalho e tao errado quanto
    re-promptar a sessao.

    `started` vem de FORA, lido no inicio do `_work`, e o consumo aqui e CONDICIONAL. Popar direto
    seria uma corrida real: entre o inicio do `_work` e este ponto rodam `registry.list()` e
    `drain()`, e o proprio drain pode largar um prompt novo — a sessao volta a "working" e o
    caminho sincrono grava o inicio do turno NOVO. Um `pop` cego levaria embora esse valor: o push
    deste turno sairia com duracao errada e o turno seguinte, ao acabar de verdade, acharia
    `started is None` e nunca avisaria."""
    if started is None:
        return
    with _turno_lock:
        if _working_started.get(session_id) != started:
            return                       # outro turno ja tomou o lugar: nao e nosso pra consumir
        del _working_started[session_id]
    if not runtime_config.get("notify_finished"):
        return
    m = hook_state.get_state(session_id)
    elapsed = (m[1] if m else time.time()) - started
    if elapsed >= runtime_config.get("finish_min_seconds"):
        _notify_async(session_id, push.notify_finished)


def _on_hook_transition(session_id: str, state: str) -> None:
    """hook_state -> mudanca de estado. Drain SERVER-SIDE: o gatilho antigo morava na conexao SSE de
    cada chat — sem celular conectado, entrada deferred ficava parada indefinidamente. idle/working =
    o pane aceita texto (Claude Code enfileira internamente); o drain re-checa deliverable sozinho.
    Tambem agenda a confirmacao de entrega das drenadas.

    Alem do drain, e o choke-point dos pushes de 'terminou' (working -> idle apos turno longo, com
    debounce) e 'caiu' (-> dead, sempre) — ver push.py. Tambem o choke-point do encadeamento de sessao
    (feature #12, _maybe_chain): idle == turno realmente terminado (so o hook Stop escreve idle), entao
    e o ponto certo pra disparar o vinculo 'then' sem correr risco de pegar um redraw no meio do turno."""
    if state == "working":
        m = hook_state.get_state(session_id)
        if m:
            with _turno_lock:
                _working_started[session_id] = m[1]
    elif state == "idle":
        # O push de "terminou" NAO sai daqui: ele espera o `_work` decidir se este idle e de verdade
        # (no Kimi ele pode ser o marcador congelado do turno anterior — ver corrige_ocioso_kimi).
        # Antes disso, avisar "terminou" no meio do trabalho era o mesmo erro das outras automacoes,
        # com o agravante de o `pop` abaixo consumir o inicio do turno: o fim REAL viria sem saber
        # ha quanto tempo a sessao trabalhava, e o debounce de turno longo nunca mais dispararia.
        pass
    elif state == "dead":
        with _turno_lock:
            _working_started.pop(session_id, None)
        if runtime_config.get("notify_dead"):
            _notify_async(session_id, push.notify_dead)

    if state == "awaiting_input":
        # Loop runner: awaiting cobre pedido de permissao tb -> pausa o loop (retoma no idle seguinte).
        # Thread propria (registry.list toca tmux); sem push proprio (o _on_awaiting ja empurra).
        def _pause_loop() -> None:
            try:
                info = next((s for s in registry.list()
                             if s.jsonl and session_key(s.jsonl) == session_id), None)
                if info:
                    with loop_mod._lock:
                        link = loop_mod.LoopLink(info.name)
                        d = link.get()
                        if d and d["status"] == "running":
                            link.update(status="paused_awaiting")
            except Exception:
                pass
        threading.Thread(target=_pause_loop, daemon=True).start()
        return
    # Inicio do turno lido AQUI, antes de qualquer subprocess: o `drain` la embaixo pode largar um
    # prompt novo e a sessao voltar pra "working", e ai o valor no dict ja seria de OUTRO turno.
    # Quem consome (`_push_terminou`) confere que ainda e este antes de tirar.
    with _turno_lock:
        inicio_do_turno = _working_started.get(session_id) if state == "idle" else None

    def _work() -> None:
        try:
            info = next((s for s in registry.list()
                         if s.jsonl and session_key(s.jsonl) == session_id), None)
            # Sessao nao encontrada (morreu, ou nao esta no tmux): o push de "terminou" continua
            # saindo pelo caminho de sempre. Nao ha como desconfiar do idle sem o transcript.
            if state == "idle" and not (info and info.jsonl):
                _push_terminou(session_id, inicio_do_turno)
            if info and info.jsonl:
                # Kimi: este "idle" pode ser MENTIRA. Um turno que comeca a partir de um prompt
                # enfileirado na TUI nao dispara evento nenhum, entao o marcador fica congelado no
                # idle do turno ANTERIOR enquanto o novo roda (ver state.corrige_ocioso_kimi). Sem
                # esta checagem, tres automacoes disparam com a sessao trabalhando: o loop
                # re-prompta, o `then` e CONSUMIDO (one-shot: o fim de turno real nao o dispara de
                # novo) e o push diz "terminou". O drain segue — enfileirar texto e sempre seguro,
                # e e o que o Claude/Kimi ja fazem sozinhos.
                real = state
                if state == "idle" and getattr(info, "provider", "claude") == "kimi":
                    m = hook_state.get_state(session_id)
                    if m and corrige_ocioso_kimi(m, info.jsonl)[0] == "working":
                        real = "working"
                sent = _drenar(info.name, info.jsonl, info.provider)
                # Confirmacao em TODO idle (nao so pos-drain): Timers pendentes morrem no restart
                # do backend — sem isto, entrada entregue ficava sem confirmar indefinidamente.
                if sent or real == "idle":
                    threading.Timer(_CONFIRM_GRACE + 0.5, _confirm_and_drain,
                                    args=(info.name,)).start()
                # Loop runner: no idle, se ha loop ativo e o drain NAO acabou de digitar algo
                # (sent == 0 -> este idle e fim de turno de trabalho, nao o eco do goal/re-prompt),
                # tica o loop. Loop ativo SUPRIME o chain (senao cada idle entre iteracoes dispararia).
                loop_d = loop_mod.LoopLink(info.name).get()
                loop_active = loop_d is not None and loop_d["status"] in loop_mod.ACTIVE
                if real == "idle" and loop_active and sent == 0:
                    loop_mod.schedule_tick(info.name, lambda: _loop_ctx(info.name))
                # Encadeamento (feature #12): so quando NAO ha loop ativo — turno REALMENTE terminado,
                # reusando o info.name ja resolvido nesta thread — ver _maybe_chain.
                if real == "idle" and not loop_active:
                    _maybe_chain(info.name)
                if real == "idle":
                    # so no fim de turno PROVADO (ver o elif la em cima)
                    _push_terminou(session_id, inicio_do_turno)
                # Idle desmentido pelo transcript: o fim de turno REAL nao vai gerar transicao
                # nenhuma (o Stop grava idle sobre idle, e hook_state._apply so avisa quando o
                # estado MUDA). Sem reagendar, o turno terminaria sem drenar a fila, sem ticar o
                # loop e sem disparar o `then`. O reagendamento converge sozinho: quando o turno
                # acaba, o wire.jsonl para de crescer e a proxima passada ve idle de verdade.
                # UMA cadeia por sessao (_recheca_armada): cada transicao espuria abria a sua, e
                # duas cadeias em paralelo dobram `registry.list()` (tmux) a cada 5s pra sempre.
                _falhas_seguidas.pop(session_id, None)   # esta volta foi ate o fim: zera o teto
                if real != state and _armar_recheca(session_id):
                    threading.Timer(_RECHECA_KIMI, _recheca_kimi,
                                    args=(session_id, state)).start()
        except Exception:
            # LOGA, nao `pass` mudo: e daqui que saem o drain da fila, o tick do loop, o vinculo
            # `then` e o push de "terminou". Falha calada aqui devolve exatamente o sintoma que este
            # bloco existe pra matar — sessao que nunca drena — sem uma linha pra investigar. E o
            # texto diz a CONSEQUENCIA, nao so "falhou": no Kimi o fim de turno real nao gera
            # transicao nova (idle sobre idle), entao sem reavaliacao a sessao pode ficar parada
            # sem drenar ate a proxima msg do usuario.
            _log.warning("transicao de estado falhou sid=%s state=%s — sem reavaliacao automatica "
                         "ate a proxima transicao", session_id, state, exc_info=True)
            # Reagenda MESMO ASSIM quando o idle era suspeito: a falha pode ter sido pontual
            # (registry/tmux piscando), e desistir aqui e o que deixa a sessao presa. Mas com TETO:
            # falha PERMANENTE (jsonl corrompido, erro reproduzivel no registry) reergueria a mesma
            # excecao a cada 5s pra sempre, pagando `registry.list()` (tmux) toda volta. Depois de
            # _RETRY_FALHA tentativas a cadeia para e diz isso no log — sessao presa e ruim, laco
            # eterno tocando tmux e pior.
            if state == "idle":
                n = _falhas_seguidas.get(session_id, 0) + 1
                if n <= _RETRY_FALHA:
                    _falhas_seguidas[session_id] = n
                    if _armar_recheca(session_id):
                        threading.Timer(_RECHECA_KIMI, _recheca_kimi,
                                        args=(session_id, state)).start()
                elif n == _RETRY_FALHA + 1:
                    # NAO zera o contador aqui: zerando, o proximo evento recomecava a contagem e o
                    # laco voltava a girar de 5 em 5s — teto que reinicia nao e teto. Quem zera e a
                    # volta que COMPLETA (no fim do `_work`), que e a prova de que voltou a
                    # funcionar. Loga uma vez so, na virada.
                    _falhas_seguidas[session_id] = n
                    _log.error("reavaliacao de %s abandonada apos %d falhas seguidas — a sessao so "
                               "volta a drenar sozinha na proxima transicao de estado",
                               session_id, _RETRY_FALHA)
                else:
                    _falhas_seguidas[session_id] = n
    threading.Thread(target=_work, daemon=True).start()


class _StrictBody(BaseModel):
    # rejeita campos desconhecidos no corpo (contrato estrito; pega typo de campo do cliente -> 422).
    model_config = ConfigDict(extra="forbid")


class CreateBody(_StrictBody):
    name: str = Field(min_length=1)
    cwd: str = Field(min_length=1)
    config_dir: str | None = None
    # Qual Adapter cria a sessao (app.adapters.get_adapter). Default "claude" preserva o
    # comportamento de hoje pros clientes que ainda nao mandam o campo.
    provider: str = "claude"
    # Wrapper interativo do Codex pode iniciar a TUI ja com um prompt. Nao e argv arbitrario:
    # evita que um cliente remoto injete flags que afrouxem sandbox/aprovacoes do backend.
    initial_prompt: str | None = None
    # Motor de modelo (nome no engines.json). None = conta Anthropic, comportamento de hoje.
    engine: str | None = None
    # Escolhidos na tela de abertura. None = padrão do binário (comportamento de hoje). Validado
    # aqui, nunca no front: o valor entra num comando de shell.
    model: str | None = None
    effort: str | None = None
    # Modo de permissão do Claude Code. None = padrão da conta (comportamento de hoje).
    permission_mode: str | None = None


class TtsBody(_StrictBody):
    # max_length: sem teto, um corpo de 100 MB era parseado INTEIRO antes do 413 do preparo/teto
    # de caracteres poder recusar. 200_000 e bem acima do teto real (_TTS_TETO) — so evita o corpo
    # gigante, quem recusa por caractere de verdade continua sendo a rota.
    text: str = Field(min_length=1, max_length=200_000)
    voice: str = ""
    provider: str = "elevenlabs"
    # Confirmacao explicita do usuario pra passar do limite de aviso. Ver _TTS_TETO abaixo: o teto
    # duro nao e confirmavel, so o limite de aviso.
    confirm: bool = False
    # Fase 2 (narracao guiada): instrucao que ja tratou este `text` via POST /api/tts/narrar ("" =
    # leitura direta, o caminho de hoje). So entra na chave do cache (tts.hash_de) — chega aqui
    # depois que o texto ja esta pronto pra virar audio, nunca dispara a Groq.
    instruction: str = ""


class NarrarBody(_StrictBody):
    text: str = Field(min_length=1, max_length=200_000)
    code_blocks: list[str] = Field(default_factory=list)
    instruction: str = Field(min_length=1, max_length=2000)


class PushSubscribeBody(_StrictBody):
    subscription: dict  # PushSubscription do browser: {endpoint, keys:{p256dh, auth}}
    label: str = Field(min_length=1)    # nome do servidor escolhido no celular (Casa/my-org)
    serverId: str = Field(min_length=1)  # id local do servidor no celular (pro deep-link da notif)
    # Idioma da inscricao: o front manda o escolhido na tela Geral; ausente (front velho) cai em
    # "en", o baseLocale do app — a leitura do registro antigo e que trata o campo ausente como pt.
    locale: str = "en"


@app.get("/api/push/vapid", dependencies=[Depends(require_auth)])
def push_vapid():
    # Chave publica VAPID (applicationServerKey) pro browser assinar. Vazia = push desligado no backend.
    return {"key": settings.vapid_public}


@app.post("/api/push/subscribe", dependencies=[Depends(require_auth)])
def push_subscribe(body: PushSubscribeBody):
    try:
        push.add_subscription(body.subscription, body.label, body.serverId, body.locale)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


@app.get("/api/push/settings", dependencies=[Depends(require_auth)])
def push_settings():
    # Estado atual (mute por sessao + quiet hours global) pro app refletir na UI.
    return push.get_push_prefs()


class PushMuteBody(_StrictBody):
    session: str = Field(min_length=1)
    muted: bool


@app.post("/api/push/mute", dependencies=[Depends(require_auth)])
def push_mute(body: PushMuteBody):
    push.set_muted(body.session, body.muted)
    return {"ok": True}


class PushQuietHoursBody(_StrictBody):
    # HH:MM. Ambos None desliga a janela; so ha janela com os dois presentes.
    start: str | None = None
    end: str | None = None


@app.post("/api/push/quiet-hours", dependencies=[Depends(require_auth)])
def push_quiet_hours(body: PushQuietHoursBody):
    try:
        push.set_quiet_hours(body.start, body.end)
    except ValueError as e:
        raise HTTPException(422, str(e))
    return {"ok": True}


class InputBody(_StrictBody):
    text: str
    # ACEITO E IGNORADO. Existiu por ~40min em 14/08/2026 (o steer ia junto do envio; virou uma tecla
    # avulsa, POST /steer). O corpo e estrito, entao tirar o campo fez a PAGINA ABERTA — que e um PWA
    # com service worker e pode ficar versoes atras — receber 422 em TODO envio: "Extra inputs are
    # not permitted". Cliente velho nao pode quebrar por causa de campo que o servidor deixou de
    # usar; fica aqui como tolerancia, sem efeito nenhum.
    steer: bool = False


class BroadcastBody(_StrictBody):
    names: list[str] = Field(min_length=1)
    text: str


class SelectBody(_StrictBody):
    option: int = Field(ge=1, le=50)  # picker 1-based; teto evita loop de fork tmux (DoS)


class KeyBody(_StrictBody):
    key: str  # nome da tecla de navegacao (allowlist em TerminalInput._NAV_KEYS)


class TermInputBody(_StrictBody):
    # Terminal interativo (so desktop): texto livre (literal) e/ou uma tecla nomeada (allowlist
    # em TerminalInput._TERM_KEYS). Os dois opcionais -> um POST pode mandar so texto OU so tecla.
    text: str | None = None
    key: str | None = None


class ModelEffortBody(_StrictBody):
    # ambos opcionais: so esforco (sem modelo) ainda dirige o picker do /model, deixando o
    # modelo na linha atual. scope: 'session' (aperta `s`) ou 'default' (aperta Enter).
    model: str | None = None
    effort: str | None = None
    scope: Literal["session", "default"] = "session"


@app.get("/api/sessions", dependencies=[Depends(require_auth)], response_model=list[SessionInfo])
async def list_sessions():
    # list_with_state: resolucao otimizada (1 scan /proc + 1 chamada tmux em lote) + estado vivo por
    # sessao (working/idle/awaiting_input) classificado do pane. async pq captura os panes concorrente.
    # MuxIndisponivel nao e tratada aqui: o handler de `_mux_indisponivel` cobre esta rota e as
    # outras quinze que chamam registry.list(). Um try/except so nesta seria a mesma resposta
    # escrita duas vezes, e a que envelhece primeiro.
    return await registry.list_with_state()


@app.post("/api/diag", dependencies=[Depends(require_auth)])
async def diag_anotar(request: Request):
    """Lote de eventos da TELA pro diário de uso (backend/app/diag.py).

    Aceita o que reconhece e descarta o resto em silêncio — de propósito. Este endpoint não pode ser
    um caminho que falha: ele descreve o uso, e um 400 aqui viraria um erro na tela causado pelo
    próprio mecanismo de registrar erros.
    """
    try:
        corpo = await request.json()
    except Exception:                                # noqa: BLE001 — ver docstring
        return {"gravadas": 0}
    lote = corpo.get("eventos") if isinstance(corpo, dict) else corpo
    return {"gravadas": await asyncio.to_thread(diag.anotar_da_tela, lote)}


@app.get("/api/diag", dependencies=[Depends(require_auth)])
async def diag_resumo(ultimas: int = 60):
    resumo = await asyncio.to_thread(diag.resumo)
    # As últimas linhas junto: a tela precisa PROVAR que está gravando, e um segundo pedido só pra
    # isso seria mais latência pra mostrar a mesma coisa.
    resumo["ultimas"] = await asyncio.to_thread(diag.ultimas, max(1, min(ultimas, 200)))
    return resumo


@app.get("/api/diag/arquivo", dependencies=[Depends(require_auth)])
async def diag_arquivo():
    # Baixa como anexo pra pessoa mandar no chat — é o único caminho do diário até quem analisa.
    texto = await asyncio.to_thread(diag.ler_tudo)
    return Response(
        content=texto, media_type="application/x-ndjson",
        headers={"Content-Disposition": 'attachment; filename="hangar-uso.jsonl"'})


@app.get("/api/claude-configs", dependencies=[Depends(require_auth)], response_model=list[ConfigDirInfo])
def claude_configs():
    return list_config_dirs()


class ContaBody(_StrictBody):
    # pattern com \z (fim absoluto da crate regex do pydantic — o \Z do Python não é aceito lá,
    # e o $ casaria antes de uma quebra de linha final, deixando 'conta2\n' passar: pasta com
    # controle de linha no nome). O mesmo padrão do contas._NOME_OK, que é fullmatch; aqui no
    # schema o pedido inválido nem chega no módulo.
    nome: str = Field(min_length=1, max_length=32,
                      pattern=r"^[a-z0-9][a-z0-9_-]{0,31}\z")


@app.post("/api/claude-configs", dependencies=[Depends(require_auth)])
async def post_claude_config(body: ContaBody):
    """Cria a pasta da conta. NÃO loga: o OAuth abre navegador e é interativo — quem roda o
    /login é o usuário, dentro da primeira sessão aberta nessa conta."""
    if os.environ.get("CP_CLAUDE_CONFIG_DIRS", "").strip():
        # Com a lista fixa por env, list_config_dirs ignora o auto-scan: a conta seria criada e
        # nunca apareceria no seletor. Recusar com o motivo é melhor que um 200 inútil.
        raise HTTPException(409, detail=erro("erro_config_dirs_fixo",
                                 "CP_CLAUDE_CONFIG_DIRS está setado: a lista de contas é fixa por "
                                 "ambiente. Remova a variável ou acrescente a conta nela."))
    try:
        p = await asyncio.to_thread(contas.criar, body.nome)
    except contas.ContaError as e:
        raise HTTPException(e.status, e.detail) from None
    return {"path": str(p), "label": body.nome, "active": False}


@app.delete("/api/claude-configs/{nome}", dependencies=[Depends(require_auth)])
async def delete_claude_config(nome: str):
    """Apaga a conta e os transcripts dela. Recusa se alguma sessão viva estiver usando, se a
    conta for a configuração ativa do backend, se estiver na lista fixa do ambiente ou se algum
    processo vivo tiver o config dir dela — apagar debaixo de um deles deixa o CLI escrevendo
    num caminho que sumiu."""
    try:
        alvo = contas.caminho(nome)
    except contas.ContaError as e:
        # Nome fora do alfabeto da conta (ex: pasta de backup com ponto no nome): envelope pra
        # o front traduzir no idioma do app, em vez de mostrar a string crua do módulo.
        raise HTTPException(e.status, detail=erro("erro_conta_nome_invalido", e.detail)) from None
    if alvo.resolve() == _backend_config_base().resolve():
        # A config ativa do backend é o ~/.claude (ou o CLAUDE_CONFIG_DIR dele): settings,
        # custos e transcripts do próprio app moram lá — apagar derrubaria o app em si.
        raise HTTPException(409, detail=erro("erro_conta_ativa_backend",
                                 "esta conta é a configuração ativa do backend — não dá pra "
                                 "apagar por aqui"))
    if os.environ.get("CP_CLAUDE_CONFIG_DIRS", "").strip():
        # Com a lista fixa por env, o GET continua devolvendo esta conta MESMO apagada: sobraria
        # um fantasma no seletor, e a próxima sessão recriaria a pasta sem marcador nem atalhos.
        if alvo.resolve() in {Path(c.path).resolve() for c in list_config_dirs()}:
            raise HTTPException(409, detail=erro("erro_conta_lista_fixa",
                                     "CP_CLAUDE_CONFIG_DIRS está setado: esta conta está na "
                                     "lista fixa por ambiente. Remova-a da variável antes de "
                                     "apagar."))
    # O ciclo segura a trava da conta (a mesma do create_session) ao redor da checagem e do
    # rmtree: sem ele, o DELETE passaria na janela entre a reconciliação e o registry.create de
    # uma sessão que está subindo, e apagaria a pasta embaixo dela.
    def _checar_e_apagar():
        # TUDO numa thread só: o `ciclo_conta` pega `flock` no __enter__, que BLOQUEIA. Chamado
        # direto da rota async, uma segunda operação de conta concorrente congelava o event loop
        # inteiro — todas as rotas do app, não só esta — até a primeira soltar. E a janela é longa:
        # o laço abaixo roda um `subprocess` do tmux por sessão viva, também síncrono.
        with contas.ciclo_conta(nome) as ciclo:
            for s in registry.list():
                cfg, confiavel = _session_config_dir_strict(s.name)
                if not confiavel:
                    raise HTTPException(409, detail=erro("erro_config_dir_sessao",
                                             f"não consegui confirmar o config dir da sessão "
                                             f"'{s.name}' — apagar recusado", nome=s.name))
                if cfg is not None and cfg.resolve() == alvo.resolve():
                    raise HTTPException(409, detail=erro("erro_sessao_usa_conta",
                                             f"a sessão '{s.name}' está usando esta conta", nome=s.name))
            # CLI aberto FORA do tmux não aparece no registry: a varredura por CLAUDE_CONFIG_DIR
            # no /proc é quem segura o apagar debaixo dele.
            pids, varredura_ok = procinfo._pids_com_config_dir(alvo)
            if not varredura_ok:
                # "Não consegui olhar" não é "olhei e não achei": seguir aqui apagaria a pasta
                # debaixo de um `claude` vivo que a varredura não chegou a enxergar.
                raise HTTPException(409, detail=erro("erro_varredura_processos",
                                         "não consegui varrer os processos da máquina — apagar "
                                         "recusado (pode haver um claude aberto nesta conta)"))
            if pids:
                raise HTTPException(409, detail=erro("erro_processos_usam_conta",
                                         f"processo(s) {pids} estão usando esta conta", pids=pids))
            ciclo.apagar()

    try:
        await asyncio.to_thread(_checar_e_apagar)
    except contas.ContaError as e:
        # Pasta não carimbada (ou conta que sumiu): mesmo 404 do apagar() antigo, agora como
        # envelope — a mesma chave do login (erro_conta_inexistente) traduz nos dois fluxos.
        raise HTTPException(e.status, detail=erro("erro_conta_inexistente", e.detail,
                                                  nome=nome)) from None
    return {"ok": True}


@app.get("/api/desktop/palette", dependencies=[Depends(require_auth), Depends(require_loopback)])
def desktop_palette_get():
    # 404 e resposta de negocio, nao erro: e como o front sabe que nao ha rice nesta maquina e
    # esconde a opcao.
    p = desktop_palette.ler()
    if p is None:
        raise HTTPException(status_code=404, detail=erro("erro_sem_paleta", "sem paleta"))
    return p


@app.get("/api/desktop/wallpaper", dependencies=[Depends(require_auth), Depends(require_loopback)])
def desktop_wallpaper_get():
    # A imagem que o rice esta usando agora, pro modo "Vidro" do fundo Desktop desenhar ela DENTRO da
    # pagina (backdrop-filter so enxerga o que a propria pagina pintou; atras de janela transparente
    # nao ha pixel nenhum pra virar vidro). 404 = sem rice/sem foto, e o front esconde a opcao.
    p = desktop_palette.wallpaper()
    if p is None:
        raise HTTPException(status_code=404, detail=erro("erro_sem_papel_de_parede", "sem papel de parede"))
    # Sem cache do navegador: trocar o papel de parede mantem a URL e so muda o conteudo, entao um
    # 304 deixaria a foto velha na tela ate alguem limpar o cache.
    return FileResponse(p, headers={"Cache-Control": "no-store"})


@app.get("/api/costs", dependencies=[Depends(require_auth)], response_model=CostReport)
def costs_endpoint(period: str = "all"):
    # Período inválido cai em "all" em vez de 422: um cliente antigo da malha mandando qualquer
    # coisa não pode derrubar o custo daquela máquina inteira da soma.
    # Lista vem de costs.PERIODOS (fonte única com o montar()); "all" fica de fora do dict porque
    # não tem número de dias, então entra à parte aqui.
    if period not in _COST_PERIODOS and period != "all":
        period = "all"
    return costs_report(period=period)


@app.post("/api/sessions", dependencies=[Depends(require_auth)], response_model=SessionInfo)
async def create_session(body: CreateBody):
    # Handler async por causa da trava de conta mais abaixo. Todo provider passa pelo MESMO
    # registry.create — o Codex tambem, desde que o lancador unico virou o comando do pane dele.
    # registry.create e SINCRONO e spawna um
    # subprocess tmux (bloqueante) -> rodar direto aqui travaria o event loop / o SSE de outras
    # sessoes; vai pro threadpool via asyncio.to_thread, igual aos outros handlers async deste
    # arquivo que chamam registry.list()/save_upload (menor risco de regressao: comportamento e
    # exceções do create() Claude ficam IDENTICOS, so a chamada muda de sync p/ thread).
    # Pi entra pelo MESMO registry.create do Claude (pane tmux + spawn_command do PiAdapter); o que
    # muda la dentro e so o transcript, que nao e pre-semeado (layout proprio, arquivo so no 1o turno).
    # Validar provider, config_dir e engine ANTES de qualquer efeito no disco: um pedido que vai
    # ser rejeitado aqui não pode ter reconciliado a conta (deriva movida, memória criada) à toa.
    if body.provider not in ("claude", "codex", "pi", "kimi"):
        raise HTTPException(400, detail=erro("erro_provider_sessao_invalido", "provider invalido"))
    if body.config_dir is not None and body.config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    # Mesma guarda do config_dir. Codex nao usa spawn_command/tmux desse jeito, entao motor + codex e
    # pedido incoerente — 400, nao "ignora e segue".
    if body.engine is not None:
        if body.provider != "claude":
            raise HTTPException(400, detail=erro("erro_motor_sem_claude", "motor so vale para provider claude"))
        if body.engine not in await asyncio.to_thread(engines.listar):
            raise HTTPException(400, detail=erro("erro_motor_invalido", "motor invalido"))
    # permission_mode só vale para claude
    if body.permission_mode is not None and body.provider != "claude":
        raise HTTPException(409, detail=erro("erro_permissao_so_claude", "modo de permissao so vale para claude"))
    # Mesma regra das linhas acima, pro model/effort: recusa ANTES de qualquer efeito no disco,
    # inclusive pro provedor fora de escopo (codex/kimi) quando alguem pedir escolha — o valor
    # entraria num comando de shell montado por concatenacao.
    try:
        model_args.validar(body.provider, body.model, body.effort, body.permission_mode)
    except ValueError as e:
        # permission_mode fora da lista deve ser 409 com código específico, não 400 genérico
        msg = str(e)
        if "permission_mode" in msg:
            raise HTTPException(409, detail=erro("erro_permissao_invalida", msg)) from None
        raise HTTPException(400, str(e)) from None
    # O nível do Codex não tem lista fechada em model_args (varia POR MODELO), então quem cruza
    # modelo×nível é o catálogo. Sem isto, `--effort ultra` num `gpt-5.5` sobe a sessão e o binário
    # descarta o nível calado — sucesso reportado sobre escolha que não valeu.
    if body.provider == "codex" and (body.model or body.effort):
        try:
            await asyncio.to_thread(codex_models.checar_escolha, body.model, body.effort)
        except ValueError as e:
            raise HTTPException(422, detail=erro("erro_codex_escolha_invalida", str(e), erro=str(e))) from None
        except (RuntimeError, OSError) as e:
            # Catálogo fora do ar (ou `codex` ausente — o CodexAusente é um RuntimeError) não pode
            # IMPEDIR de abrir sessão: mesma decisão da janela do motor, logo abaixo. A escolha
            # segue pro comando e o CLI decide. A falha não some — fica no log.
            _log.warning("codex: catalogo indisponivel, escolha nao conferida: %s", e)

    # Janela do modelo escolhido, pra entrar no env do motor (Task 3). O número já está no cache do
    # catálogo do provedor (_engine_models); vir do navegador seria deixar um terceiro escolher uma
    # variável de ambiente — e ainda ficaria None justamente nos provedores que não reportam
    # context_length. Com motor mas sem modelo (ou vice-versa), nada a resolver: o env segue o motor.
    janela = None
    if body.engine and body.model:
        try:
            for m in await _engine_models(body.engine):
                if m["id"] == body.model:
                    janela = m.get("context_length")
                    break
        except HTTPException:
            # _engine_models devolve 502 quando o cache expirou e o /v1/models não responde, e 409
            # quando o motor sumiu do arquivo entre a validação e aqui. A janela é enfeite: deixar
            # essa chamada derrubar a criação faria o provedor fora do ar IMPEDIR de abrir sessão —
            # coisa que hoje não acontece, e que contradiz o Step 5 da Task 5 ("provedor parado: a
            # sessão ainda cria"). A sessão sobe sem a var e o CLI usa o default dele.
            janela = None

    # Reconciliar e criar a sessão sob a MESMA trava (ciclo_conta), só no caminho que consome o
    # config dir (Claude/Pi — o Codex tem conta propria e nao le config dir do Claude). Sem o ciclo, um DELETE da
    # conta no meio via a lista de sessões ainda vazia e apagaria a pasta embaixo da sessão que
    # está subindo (a criação roda em thread).
    if body.config_dir is not None and body.provider in ("claude", "pi"):
        alvo = Path(body.config_dir)
        if contas.e_conta(alvo):
            nome_conta = alvo.name.removeprefix(".claude-")
            try:
                # `ciclo_conta` numa thread pelo mesmo motivo do DELETE: o `flock` do __enter__
                # bloqueia, e no event loop isso congelava o app inteiro quando duas operações de
                # conta se cruzavam. flock pertence ao descritor aberto, não à thread — tomar e
                # soltar de threads diferentes é válido.
                cm = contas.ciclo_conta(nome_conta)
                ciclo = await asyncio.to_thread(cm.__enter__)
                try:
                    try:
                        avisos = await asyncio.to_thread(ciclo.reconciliar,
                                                         sanitize_cwd(body.cwd))
                    except contas.ContaError as e:
                        # ContaError já carrega status HTTP (o hangar-conta imprime o detail). Deixar
                        # escapar viraria 500 com traceback — o usuário não saberia por que a
                        # abertura falhou (ex: Windows sem Modo Desenvolvedor recusando symlink).
                        raise HTTPException(e.status, e.detail) from None
                    except OSError as e:
                        raise HTTPException(500, detail=erro("erro_conta_reconciliacao_falhou",
                                             f"não consegui reconciliar a conta "
                                             f"{nome_conta}: {e}", nome_conta=nome_conta,
                                             erro=str(e))) from None
                    for aviso in avisos:
                        _log.warning("conta %s: %s", alvo.name, aviso)
                    try:
                        _kw = dict(provider=body.provider, engine=body.engine, model=body.model,
                                   effort=body.effort, context_window=janela)
                        if body.permission_mode is not None:
                            _kw["permission_mode"] = body.permission_mode
                        info = await asyncio.to_thread(registry.create, body.name, body.cwd, body.config_dir, **_kw)
                        return info.model_copy(update={"avisos": list(avisos)})
                    except ValueError as e:
                        raise HTTPException(409, str(e))
                finally:
                    # Solta a trava sempre — inclusive quando o corpo levanta HTTPException.
                    await asyncio.to_thread(cm.__exit__, None, None, None)
            except contas.ContaError as e:
                # Conta sumiu entre a validação e a trava (ex: DELETE concorrente).
                raise HTTPException(e.status, e.detail) from None
    try:
        _kw2 = dict(provider=body.provider, engine=body.engine, model=body.model,
                     effort=body.effort, context_window=janela)
        if body.permission_mode is not None:
            _kw2["permission_mode"] = body.permission_mode
        if body.initial_prompt is not None:
            _kw2["initial_prompt"] = body.initial_prompt
        return await asyncio.to_thread(registry.create, body.name, body.cwd, body.config_dir, **_kw2)
    except ValueError as e:
        raise HTTPException(409, str(e))


@app.delete("/api/sessions/{name}", dependencies=[Depends(require_auth)])
async def kill_session(name: str):
    # 500 quando a sessao SOBREVIVE ao kill — mesmo padrao do /rename logo abaixo, que ja confere e
    # responde 404/500. Antes era {"ok": true} incondicional: o card sumia da UI e a sessao reaparecia
    # na varredura seguinte, sem fila e sem pareamento (ver SessionRegistry.kill).
    # Os peers são lidos ANTES do kill: registry.kill -> _clear_pair já limpa o sidecar, e depois
    # dele ninguém sabe quem ficou.
    link = await asyncio.to_thread(lambda: PairLink(name).get())
    try:
        await asyncio.to_thread(registry.kill, name)
    except KillFailed as e:
        raise HTTPException(500, str(e))
    warn = None
    if link:
        errs = await _avisar_saida(name, link["peers"], "encerrou a sessão e saiu do grupo de trabalho")
        if errs:
            warn = erro("erro_pareamento_saida_falhou",
                        "aviso de saída falhou: " + "; ".join(
                            f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs),
                        avisos=errs)
    return {"ok": True, "warning": warn}


class RenameBody(_StrictBody):
    new: str


@app.post("/api/sessions/{name}/rename", dependencies=[Depends(require_auth)])
def rename_session(name: str, body: RenameBody):
    from app import tmux
    # tmux nao aceita espaco/./: no nome -> sanitiza. O transcript NAO depende do nome (resolve por
    # /proc), entao renomear nao quebra o historico. Migra so o sidecar da fila (keyed por nome).
    new = sanitize_session_name(body.new)
    if not new:
        raise HTTPException(400, detail=erro("erro_nome_invalido", "nome invalido"))
    if not tmux.has_session(name):
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if new == name:
        return {"ok": True, "name": name}
    if tmux.has_session(new):
        raise HTTPException(409, detail=erro("erro_nome_em_uso", "ja existe uma sessao com esse nome"))
    if not tmux.rename_session(name, new):
        raise HTTPException(500, detail=erro("sessao_falha_renomear", "falha ao renomear"))
    registry.rename(name, new)  # migra o cache name->jsonl (senao serve transcript errado pos-rename)
    from app.pqueue import PromptQueue
    try:
        oq, nq = PromptQueue(name).path, PromptQueue(new).path
        if oq.exists():
            atomico.substituir(oq, nq)
        # O dossiê da passagem de bastão é keyed por nome do MESMO jeito que a fila: sem migrar
        # junto, a sucessora renomeada fica com um kick-off apontando pro caminho antigo e o
        # `prune` apaga o arquivo em 7 dias por não achar sessão viva com aquele nome.
        od, nd = bastao_mod.caminho(name), bastao_mod.caminho(new)
        if od.exists():
            atomico.substituir(od, nd)
    except OSError as e:
        # Não derruba o rename (que já aconteceu no tmux), mas APARECE: sidecar que não migrou é
        # fila perdida ou dossiê órfão, e nenhum dos dois pode sumir calado.
        _log.warning("rename %s -> %s: sidecar nao migrou: %s", name, new, e)
    return {"ok": True, "name": new}


class ThenLinkBody(_StrictBody):
    target: str = Field(min_length=1)
    text: str = Field(min_length=1)


@app.put("/api/sessions/{name}/then", dependencies=[Depends(require_auth)])
def set_then_link(name: str, body: ThenLinkBody):
    """Arma o vinculo 'then' (feature #12): quando `name` confirmar idle (turno terminado), `body.text`
    e enviado pra `body.target` -- ver app.chain.ThenLink e app.api._maybe_chain. Um hop so (nao DAG):
    setar de novo so troca alvo/texto, nao encadeia mais niveis."""
    from app import tmux
    if body.target == name:
        raise HTTPException(400, detail=erro("erro_encadeamento_proprio", "sessao nao pode encadear pra si mesma"))
    if not tmux.has_session(body.target):
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao alvo nao encontrada"))
    ThenLink(name).set(body.target, body.text)
    return {"ok": True}


@app.delete("/api/sessions/{name}/then", dependencies=[Depends(require_auth)])
def clear_then_link(name: str):
    ThenLink(name).clear()
    return {"ok": True}


# --- Loop runner (harness bloco A) -------------------------------------------

class LoopCreate(_StrictBody):
    goal: str = Field(min_length=1)
    check_cmd: str | None = None
    max_iters: int = Field(default=10, ge=1, le=100)  # teto: loop nao vira gerador infinito de prompts
    require_branch: bool = True


class LoopResolve(_StrictBody):
    accept: bool


class LoopRefine(_StrictBody):
    goal: str = Field(min_length=1, max_length=2000)
    check_cmd: str | None = None


def _loop_ctx(name: str) -> "loop_mod.TickCtx | None":
    """Monta o TickCtx real da sessao CORRENTE (nome -> jsonl/cwd via registry, sobrevive /clear).
    deliver = enfileira delivered=False + drain (caminho unico de entrega; a entrada e duravel, entao
    o drain server-side reentrega depois) -> retorna True sempre; enqueue nunca dispara o fallback do
    run_tick (senao duplicaria a entrada). Sessao sumida -> loop failed + notify, return None."""
    info = next((i for i in registry.list() if i.name == name), None)
    if info is None or not info.jsonl:
        loop_mod._end(loop_mod.LoopLink(name), name, "failed", "sessão morta", push.notify_loop)
        return None
    jsonl = info.jsonl
    provider = info.provider

    def deliver(prompt: str) -> bool:
        PromptQueue(name).append(prompt, delivered=False)
        drain(name, jsonl, provider)
        return True

    return loop_mod.TickCtx(
        cwd=info.cwd or "",
        jsonl=jsonl,
        deliver=deliver,
        enqueue=lambda p: PromptQueue(name).append(p, delivered=False),
        notify=push.notify_loop,
        automations=automations_enabled,
        branch=branch_of,
        last_assistant=last_assistant_text,
        run_check=loop_mod._run_check,
        entry_delivered=lambda eid: PromptQueue(name).entry_delivered(eid),
    )


@app.post("/api/sessions/{name}/loop", dependencies=[Depends(require_auth)])
def loop_create(name: str, body: LoopCreate):
    info = next((i for i in registry.list() if i.name == name), None)
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if getattr(info, "provider", "claude") != "claude":
        # Codex e outros nao sao tmux: sem hook de transicao, o tick nunca dispara -> loop ficaria
        # running mudo pra sempre. Recusa cedo em vez de criar um loop-zumbi.
        raise HTTPException(409, detail=erro("erro_loop_provider_invalido", "loop runner só suporta sessões claude"))
    if not automations_enabled():
        raise HTTPException(409, detail=erro("erro_automacoes_desligadas", "automações desligadas (kill-switch)"))
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        cur = link.get()
        if cur and cur["status"] in loop_mod.ACTIVE:
            raise HTTPException(409, detail=erro("erro_loop_ja_ativo", "já existe um loop ativo nesta sessão"))
        br = branch_of(info.cwd) if info.cwd else None
        if body.require_branch and br in ("main", "master"):
            raise HTTPException(409, detail=erro("erro_loop_branch_invalida", f"sessão está na branch {br} — crie uma branch ou desligue 'exigir branch'", br=br))
        d = loop_mod.new_loop(body.goal, body.check_cmd, body.max_iters, body.require_branch)
        entry = PromptQueue(name).append(body.goal, delivered=False)
        d["goal_entry_id"] = entry["id"]
        link.set(d)
    if info.jsonl:
        drain(name, info.jsonl, info.provider)   # entrega ja se a sessao estiver entregavel; senao o drain server-side entrega depois
    return {"loop": link.get()}


@app.get("/api/sessions/{name}/loop", dependencies=[Depends(require_auth)])
def loop_get(name: str):
    info = next((i for i in registry.list() if i.name == name), None)
    suggestions = loop_mod.suggest_checks(info.cwd) if info and info.cwd else []
    return {"loop": loop_mod.LoopLink(name).get(), "suggestions": suggestions}


@app.delete("/api/sessions/{name}/loop", dependencies=[Depends(require_auth)])
def loop_stop(name: str):
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        if link.get() is None:
            raise HTTPException(404, detail=erro("erro_loop_inexistente", "nenhum loop nesta sessão"))
        d = loop_mod._end(link, name, "stopped", "parado pelo usuário", push.notify_loop)
    return {"loop": d}


@app.post("/api/sessions/{name}/loop/refine", dependencies=[Depends(require_auth)])
def loop_refine(name: str, body: LoopRefine):
    """Refina o objetivo do loop via claude -p efemero (sonnet). Stateless — nao toca a sessao nem o
    sidecar; o {name} da rota so mantem a familia de URLs consistente. Falha do CLI -> 502.
    Sob o kill-switch mestre: refine dispara um agente autonomo, entao respeita automations_enabled."""
    if not automations_enabled():
        raise HTTPException(409, detail=erro("erro_automacoes_desligadas", "automações desligadas (kill-switch)"))
    try:
        return {"goal": loop_mod.refine_goal(body.goal, body.check_cmd)}
    except loop_mod.ClaudePError as e:
        _log.warning("loop/refine falhou (%s): %s", name, e)
        raise HTTPException(502, str(e))


@app.post("/api/sessions/{name}/loop/resolve", dependencies=[Depends(require_auth)])
def loop_resolve(name: str, body: LoopResolve):
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        cur = link.get()
        if cur is None or cur["status"] != "done_claimed":
            raise HTTPException(409, detail=erro("erro_loop_estado_errado", "loop não está aguardando confirmação"))
        if body.accept:
            d = loop_mod._end(link, name, "done", "confirmado pronto", push.notify_loop)
            return {"loop": d}
        # reject: conta iteracao e re-prompta (reusa o MESMO helper do run_tick)
        cur["status"] = "running"
        link.set(cur)
        if cur["iter"] + 1 > cur["max_iters"]:
            d = loop_mod._end(link, name, "exhausted", f"esgotou {cur['max_iters']} iterações",
                              push.notify_loop)
            return {"loop": d}
        ctx = _loop_ctx(name)
        if ctx is None:
            return {"loop": link.get()}
        loop_mod._reprompt(link, cur, "conclusão rejeitada pelo usuário", None,
                           ctx.deliver, ctx.enqueue)
    return {"loop": link.get()}


class ResumeBody(_StrictBody):
    # None = "escolha por mim" (caso seguro) ou pede confirmacao (caso ambiguo). uuid = o candidato que o
    # usuario escolheu no sheet de confirmacao.
    session_id: str | None = None


@app.post("/api/sessions/{name}/resume", dependencies=[Depends(require_auth)])
def resume_session(name: str, body: ResumeBody):
    # Relança uma sessao "sem id" com `claude --resume <uuid>` pra passar a rastrea-la (chat volta a abrir,
    # continuando a conversa). Sem session_id: se so ha esta sessao no cwd, retoma o transcript mais
    # recente direto; se ha outras (ambiguo), devolve os candidatos pro app confirmar antes.
    sid = body.session_id
    if sid is None:
        try:
            _, ambiguous, candidates = registry.resume_candidates(name)
        except ValueError as e:
            raise HTTPException(404, str(e))
        if not candidates:
            raise HTTPException(404, detail=erro("erro_transcript_ausente", "nenhum transcript pra retomar neste diretorio"))
        if ambiguous and len(candidates) > 1:
            return {"ambiguous": True, "candidates": candidates}
        sid = candidates[0]["session_id"]
    try:
        return registry.resume(name, sid)
    except ValueError as e:
        raise HTTPException(409, str(e))


@app.get("/api/sessions/{name}/history", dependencies=[Depends(require_auth)], response_model=list[ChatEvent])
async def history(name: str, limit: int | None = None):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.pqueue import merged_history
    # provider: o rollout do Codex tem um shape DIFERENTE do jsonl do Claude (ver
    # app.adapters.codex.rollout) -- sem isto merged_history tentava o parser do Claude em toda
    # linha do rollout, nunca casava e devolvia [] (chat do Codex abria vazio ate o SSE encher via
    # backfill do tail; reabrir apos ficar horas em segundo plano perdia o que passou do tail-200).
    # Com limit, merged_history faz tail-read (parseia so o fim do arquivo); to_thread porque o
    # parse (mesmo da cauda) e CPU/IO sincrono.
    evs = await asyncio.to_thread(merged_history, name, info.jsonl, info.provider, limit)
    # Cauda CRUA de proposito: o corte no 1o
    # user_msg (pra nao desenhar resposta orfa) e preferencia de RENDERIZACAO do card do quadro e vive
    # no BoardCard.svelte. Aplicado AQUI, valia pra todo consumidor e matava a espiada do hover da
    # Sidebar (HP_TAIL=8), que so quer o ultimo assistant_msg: com o proximo prompt ja mandado, o corte
    # jogava fora a resposta anterior -> latestAssistantEvent = None -> popover vazio, cacheado por 30s.
    if limit is not None and limit > 0:
        return evs[-limit:]
    return evs


@app.get("/api/sessions/{name}/bastao", dependencies=[Depends(require_auth)])
async def bastao(name: str):
    """Dossiê de continuidade da sessão, em markdown. SÓ leitura — não cria nada e não grava nada.

    to_thread não é detalhe: `montar` roda `git status`/`git diff` (subprocess) e parseia a cauda de
    um transcript que pode ter MB. Trabalho desses dentro da corrotina trava o loop e leva junto o
    SSE de TODAS as sessões — é o incidente de 2026-07-23, quando um `git status` no tick da lista
    derrubou a conexão inteira.
    """
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    texto = await asyncio.to_thread(bastao_montar, info.jsonl, info.cwd, info.provider, name)
    return Response(content=texto, media_type="text/markdown; charset=utf-8")


@app.get("/api/sessions/{name}/bastao/dossie", dependencies=[Depends(require_auth)])
async def bastao_dossie(name: str):
    """O dossiê que ESTA sessão RECEBEU, lido do disco — não um novo.

    Irmão do GET acima e diferente dele de propósito: aquele MONTA o dossiê da sessão pedida agora,
    e serve pra prévia de quem vai passar o bastão. Este devolve o arquivo gravado na hora da
    passagem, que é o que a sucessora leu. Mostrar um dossiê remontado no lugar dele seria exibir
    um texto que ninguém leu como se fosse a instrução recebida.
    """
    alvo = bastao_mod.caminho(name)
    if not alvo.exists():
        raise HTTPException(404, detail=erro("erro_bastao_sem_dossie", "no handover dossier for this session"))
    texto = await asyncio.to_thread(alvo.read_text, encoding="utf-8")
    return Response(content=texto, media_type="text/markdown; charset=utf-8")


class BastaoBody(_StrictBody):
    """Sessão NOVA que vai continuar o trabalho de `{name}`.

    Corpo próprio, e não `CreateBody`: aquele é `extra="forbid"` e tem `cwd` obrigatório — aqui o
    padrão é o cwd da ORIGEM (a passagem continua o mesmo trabalho, na mesma árvore). Os campos
    que sobrepõem o `CreateBody` são repassados pra ele tal e qual, então a validação de
    provider/conta/motor/modelo/esforço/permissão continua num lugar só (`model_args` + o handler
    de criação).
    """
    name: str = Field(min_length=1)          # nome da sessão nova (o destino)
    cwd: str | None = None                   # None = o cwd da origem
    config_dir: str | None = None
    provider: str = "claude"
    engine: str | None = None
    model: str | None = None
    effort: str | None = None
    permission_mode: str | None = None


def _nome_ocupado(nome: str) -> bool:
    """Já existe sessão com esse nome? Mesmas duas fontes que `registry.create` consulta antes de
    levantar `ValueError` — tmux (Claude/Pi/Kimi) e o sidecar do Codex."""
    from app.adapters.codex import sessions as codex_sessions
    return tmux.has_session(nome) or codex_sessions.exists(nome)


def _bastao_preparar(info: SessionInfo, origem: str, destino: str) -> tuple[str, Path, str]:
    """Monta o dossiê, GRAVA e devolve (texto, caminho, kick-off). Tudo sync, numa thread só.

    Gravar antes de criar a sessão é o que fecha o caso "sessão nova viva apontando pra um arquivo
    que não existe": se o disco recusar, a exceção sobe daqui e nada foi criado ainda.
    """
    texto = bastao_montar(info.jsonl, info.cwd, info.provider, origem)
    alvo = bastao_mod.gravar(destino, texto)
    conta, modelo = bastao_mod.origem_resumida(info.jsonl)
    return texto, alvo, bastao_mod.kickoff(origem, alvo, conta, modelo)


@app.post("/api/sessions/{name}/bastao", dependencies=[Depends(require_auth)])
async def bastao_passar(name: str, body: BastaoBody):
    """Passa o bastão de `{name}` pra uma sessão nova: dossiê no disco + sessão criada + kick-off
    na fila durável dela.

    A ordem é a feature: **monta → grava → cria → enfileira**. Invertida, um erro de disco deixaria
    uma sessão viva com um kick-off mandando ler um arquivo inexistente.

    A entrega NÃO passa pelo caminho do `/input` (que digita PRIMEIRO e só depois grava na fila):
    numa sessão criada há milissegundos a TUI ainda está subindo e as teclas se perdem. Entra como
    `append(delivered=False)` — durável — e o drain entrega quando ela aceitar texto.

    `to_thread` no preparo pelo mesmo motivo do GET: `montar` roda `git status` (subprocess) e
    parseia a cauda do transcript; no loop isso derruba o SSE de todas as sessões (2026-07-23).
    """
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    # UM nome só, sanitizado pelo MESMO lugar que a criação usa (`registry.create` chama isto), e
    # daqui pra frente é ele quem nomeia o arquivo e a sessão. Sanitizar duas vezes por dois
    # caminhos diferentes era o bug: `api.v2` gravava `api.v2.md` mas nascia como `api-v2`, e aí o
    # `prune` não achava chave viva pro dossiê e APAGAVA o sidecar de uma sessão viva. De quebra,
    # a guarda do "bastão pra si mesma" passa a pegar `"cc "` contra `"cc"`.
    destino = sanitize_session_name(body.name)
    if not destino:
        raise HTTPException(400, detail=erro("erro_nome_invalido", "nome invalido"))
    if destino == name:
        raise HTTPException(400, detail=erro("erro_bastao_para_si_mesma",
                                             "a sessão não passa o bastão pra si mesma"))
    # Nome JÁ OCUPADO recusa aqui, ANTES de gravar. O `create_session` lá embaixo também recusa
    # (registry.create: `ja existe uma sessao com esse nome` -> 409), só que tarde demais: o dossiê
    # é `<destino>.md`, keyed por nome, então digitar o nome de uma sessão viva que já recebeu um
    # bastão SOBRESCREVIA o dossiê dela — e o kick-off dela aponta pra aquele caminho. Mesma dupla
    # de fontes do registry (tmux + sidecar do Codex), pra recusar o mesmo conjunto de nomes.
    # Sobra uma janela estreita: se ALGUÉM MAIS criar esse nome entre esta checagem e o create, o
    # dossiê já terá sido gravado por cima. O 409 do `create_session` impede duas sessões com o
    # mesmo nome — ele NÃO desfaz essa escrita. Fechar de vez exigiria criar a sessão antes de
    # gravar, que é a ordem que a feature proíbe (sessão viva apontando pra arquivo inexistente).
    if await asyncio.to_thread(_nome_ocupado, destino):
        raise HTTPException(409, detail=erro("erro_nome_em_uso", "ja existe uma sessao com esse nome"))
    cwd = body.cwd or info.cwd
    if not cwd:
        # A origem sem cwd conhecido é o caso do transcript resolvido sem pane utilizável: sem
        # diretório não há onde criar a sessão nova, e chutar um seria pior que recusar.
        raise HTTPException(400, detail=erro("erro_bastao_sem_cwd",
                                             "a sessão de origem não tem diretório conhecido; "
                                             "escolha o cwd da sessão nova"))
    try:
        texto, alvo, kick = await asyncio.to_thread(_bastao_preparar, info, name, destino)
    except OSError as e:
        # Falha APARECE, e a sessão não nasce órfã: nada foi criado até aqui.
        _log.warning("bastao: não deu pra gravar o dossiê de %s -> %s: %s", name, destino, e)
        # `motivo` como PARAM, e não só embutido no `msg`: sem ele o front não tem como traduzir a
        # frase sem jogar fora o erro do sistema de arquivos, que é a única parte acionável dela.
        raise HTTPException(500, detail=erro("erro_bastao_gravar",
                                             f"não consegui gravar o dossiê: {e}",
                                             motivo=str(e))) from None
    # Reusa o handler de criação inteiro (validação de provider/conta/motor/model_args, ciclo da
    # conta, Codex): duplicar aquilo aqui seria uma segunda porta de criação pra manter em dia.
    # HTTPException dele sobe tal e qual — o dossiê já gravado vira sidecar órfão, que o `prune`
    # recolhe pelo nome (ver bastao_mod.caminho).
    novo = await create_session(CreateBody(
        name=destino, cwd=cwd, config_dir=body.config_dir, provider=body.provider,
        engine=body.engine, model=body.model, effort=body.effort,
        permission_mode=body.permission_mode))
    try:
        await asyncio.to_thread(lambda: PromptQueue(novo.name).append(
            kick, delivered=False, pre_transcript=True))
    except OSError as e:
        # A sessão JÁ existe: o erro tem de dizer isso, senão o 500 nomeia a coisa errada e quem
        # lê acha que nada aconteceu — e vai criar outra. Aqui o conserto é humano (mandar o
        # kick-off na mão), então o caminho do dossiê vai junto.
        _log.error("bastao: sessão %s criada, mas o kick-off não entrou na fila: %s", novo.name, e)
        raise HTTPException(500, detail=erro(
            "erro_bastao_fila", f"a sessão {novo.name} nasceu, mas o kick-off não entrou na fila "
            f"dela ({e}) — mande você mesmo o pedido apontando pra {alvo}",
            nome=novo.name, dossie=str(alvo))) from None
    # Drena numa thread: `send_prompt` espera a TUI ficar interativa (`_wait_input_ready`), o que
    # pode levar segundos numa sessão recém-criada — segurar o request nisso não ajuda ninguém.
    # Vale só pro Claude na prática (Pi/Kimi nascem com `jsonl=None` e a thread sai calada); não há
    # perda, a fila é durável e o drain do próximo idle/SSE entrega.
    threading.Thread(target=_drain_session, args=(novo.name,), daemon=True).start()
    return {"name": novo.name, "dossie": str(alvo), "texto": texto, "kickoff": kick}


@app.get("/api/sessions/{name}/workflows", dependencies=[Depends(require_auth)])
async def workflows_list(name: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.workflows import list_workflows
    return await asyncio.to_thread(list_workflows, info.jsonl)


@app.get("/api/sessions/{name}/workflows/{run_id}", dependencies=[Depends(require_auth)])
async def workflow_detail(name: str, run_id: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.workflows import get_workflow
    wf = await asyncio.to_thread(get_workflow, info.jsonl, run_id)
    if wf is None:
        raise HTTPException(404, detail=erro("erro_workflow_inexistente", "workflow run not found"))
    return wf


@app.get("/api/sessions/{name}/workflows/{run_id}/agents/{agent_id}", dependencies=[Depends(require_auth)])
async def workflow_agent_detail(name: str, run_id: str, agent_id: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.workflows import get_agent
    a = await asyncio.to_thread(get_agent, info.jsonl, run_id, agent_id)
    if a is None:
        raise HTTPException(404, detail=erro("erro_agente_inexistente", "agent not found"))
    return a


@app.get("/api/sessions/{name}/peer-address", dependencies=[Depends(require_auth)])
async def peer_address(name: str):
    """Endereço do inbox nativo desta sessão (cross-session messaging), ou `null`.

    Existe pro `hangar-send` decidir com FATO se o caminho nativo alcança este alvo, em vez de supor
    pelo tipo da sessão: quem não tem socket (sessão aberta antes da liberação da Anthropic, Codex,
    Pi) não aparece no `ListAgents` de ninguém, e mandar o modelo usar `SendMessage` ali seria
    mandá-lo bater numa porta que não existe. `null` nunca é erro — é a resposta "aqui não tem".
    """
    # `registry` aqui é a INSTÂNCIA (SessionRegistry); inbox_socket_of é função de MÓDULO.
    from app.registry import inbox_socket_of
    return {"uds": await asyncio.to_thread(inbox_socket_of, name)}


@app.get("/api/sessions/{name}/subagents", dependencies=[Depends(require_auth)])
async def subagents_list(name: str):
    # Subagentes soltos (tool Agent). O transcript de cada um mora em <session-dir>/subagents/ —
    # e é a ÚNICA fonte do que ele está chamando enquanto roda; o jsonl do pai só tem o pedido.
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.subagents import list_subagents
    return await asyncio.to_thread(list_subagents, info.jsonl)


@app.get("/api/sessions/{name}/subagents/{agent_id}", dependencies=[Depends(require_auth)])
async def subagent_detail(name: str, agent_id: str, events: int = 0):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.subagents import get_subagent
    # events=N -> devolve tambem o transcript do subagente nos MESMOS ChatEvent do chat, pra a UI
    # reusar a lista de mensagens em vez de desenhar um formato proprio.
    a = await asyncio.to_thread(get_subagent, info.jsonl, agent_id, 40, events)
    if a is None:
        raise HTTPException(404, detail=erro("erro_subagente_inexistente", "subagent not found"))
    return a


@app.get("/api/sessions/events", dependencies=[Depends(require_auth)])
async def sessions_events():
    from app.sse import list_events
    return EventSourceResponse(list_events())


@app.get("/api/sessions/{name}/events", dependencies=[Depends(require_auth)])
async def events(name: str, request: Request):
    # handler async -> registry.list() (subprocess tmux) vai pro threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    # Retomada exata: o id que emitimos no transcript e "<stem-do-jsonl>:<offset-em-bytes>". Chega
    # por header (Last-Event-ID, que o browser reenvia sozinho quando o MESMO EventSource reconecta)
    # ou por query param (o app fecha e recria o EventSource no proprio retry, e objeto novo nunca
    # manda o header -> sem o param a retomada nunca dispararia no uso real).
    #
    # O STEM e obrigatorio e tem que bater com o transcript ATUAL: apos um /clear o jsonl e outro
    # arquivo, e honrar um offset do arquivo antigo daria seek no meio do novo, pulando calado todo
    # o inicio da conversa. Nao bateu (ou lixo) -> None, e o tail cai no backfill normal.
    raw = request.query_params.get("last_event_id") or request.headers.get("last-event-id")
    start_offset = None
    if raw:
        stem, _, off = raw.rpartition(":")
        if stem and stem == session_key(info.jsonl):
            try:
                start_offset = int(off)
            except ValueError:
                start_offset = None
    # provider da sessao (SessionInfo.provider, ja marcado por registry.list() -- tmux -> "claude",
    # sidecar Codex -> "codex") -> merged_events escolhe o Adapter certo (tail do jsonl, monitor de
    # estado, fonte do preview). Sem isto TODA sessao caia no default "claude" do merged_events e o
    # SSE do Codex nunca ligava (chat vazio, sem estado ao vivo).
    return EventSourceResponse(
        merged_events(name, info.jsonl, provider=info.provider, start_offset=start_offset))


# Pool DEDICADO ao caminho de ENVIO (nucleo sagrado). Separado do executor default do asyncio, que a
# decoracao da lista (git_summary/capture_pane via asyncio.to_thread) pode ocupar em rajada -> sem isto,
# um burst de decoracao lenta atrasaria o POST /input. Poucos workers bastam (single-user; envios a uma
# mesma sessao ja serializam no _send_lock do terminal_input).
_send_executor = ThreadPoolExecutor(max_workers=4, thread_name_prefix="hangar-send")


def _send_thread(fn, *args):
    """Roda `fn(*args)` no pool DEDICADO de envio (nao no executor default, saturavel pela decoracao)."""
    return asyncio.get_running_loop().run_in_executor(_send_executor, fn, *args)


def _erro_texto(e) -> str:
    """Texto de um erro de envio: string crua (endpoint antigo) ou o `msg` do envelope {code, params, msg}.

    Os avisos compostos (pareamento, group-message) interpolam o TEXTO, nunca o dict — o dict seria
    "[object Object]" no front antigo. O front novo recebe a estrutura via params e traduz por ela;
    o msg montado aqui e a rede para quem nao tem o codigo no mapa.
    """
    return e if isinstance(e, str) else (e.get("msg") if isinstance(e, dict) else str(e))


def _send_one(name: str, text: str) -> dict:
    """Sequencia UNICA de envio de prompt: send_prompt + registro na fila duravel + confirmacao/drain.
    Usada pelo /input (uma sessao) e pelo /broadcast (loop por N sessoes) — o broadcast NAO reimplementa
    entrega, so repete esta mesma sequencia por nome. Nunca levanta (devolve ok/error) pra o broadcast
    reportar falha de uma sessao sem abortar as demais."""
    # ts da entrada carimbado ANTES do send: o send_prompt digita + Enter e o Claude Code grava o
    # prompt no transcript NA HORA, entao o append la embaixo roda DEPOIS do commit. Carimbar no
    # append punha a entrada ~ms apos o proprio commit e o dedup ts-aware do merged_history a
    # mantinha pendente (msg em dobro no historico ate o reconcile). A ordem send->append->drain
    # NAO muda — so o valor gravado, que e o unico dado que o dedup le.
    t0 = time.time()
    provider, pane_id = _pane_info(name)
    stripped = text.lstrip()
    # Pi COM LINHA: cria a entrada da fila ANTES do 1o envio, pra ter um id ESTAVEL pra oferecer
    # como msg_id (achado ALTA da revisao 02/08/2026 — "Porta A"). A extensao chama sendUserMessage
    # ANTES de confirmar, entao a PRIMEIRA tentativa (esta aqui) e a que mais importa: sem id nela,
    # um retry do drain() (apos "deferred" por ACK perdido) nao tem como a extensao reconhecer como
    # a MESMA mensagem.
    #
    # SO com linha (achado da re-revisao 02/08/2026): pre-criar a entrada TAMBEM pra Pi sem linha
    # (fallback de teclado) abre uma janela de duplo envio que nao existia antes deste commit. Entre
    # este append() e o send_prompt() abaixo nao ha trava nenhuma — o _send_lock so e adquirido
    # DENTRO do send_prompt (terminal_input.py) — e o claim_undelivered do drain() usa so o
    # _append_lock da fila, que nao tem relacao com aquele. Um drain() concorrente (hook, /input
    # duplo, _maybe_chain) podia reivindicar essa entrada na janela e digitar o MESMO texto de novo
    # assim que o send_lock liberasse. E o msg_id nao ajuda em nada nesse caminho: quem digita no
    # tty nunca le esse valor (ver comentario em terminal_input.send_prompt). Claude/Codex-via-tty
    # e Pi-sem-linha ficam todos no fluxo de sempre (append DEPOIS do send, delivered ja resolvido)
    # — e ali NAO ha janela, porque a entrada so nasce depois que o unico send_prompt desta chamada
    # ja terminou.
    entry = None
    # ponytail: JANELA RESIDUAL CONHECIDA (nao fechada agora, registrada por decisao do usuario). A
    # decisao "vai por linha ou por tecla" e tomada DUAS vezes — aqui, FORA de qualquer trava, e de
    # novo dentro do _send_lock (terminal_input.py, perto de "provider == pi and pane_id and
    # INBOX.tem_linha"). Entre as duas nao ha trava compartilhada: claim_undelivered (pqueue.py) usa
    # so o _append_lock da fila, sem relacao com o _send_lock. Se a linha cair ENTRE esta leitura de
    # tem_linha() e a segunda checagem dentro do lock, quem perde a corrida pelo _send_lock ve
    # tem_linha=False, cai pro teclado — que NUNCA le msg_id (ver terminal_input.send_prompt). Sai
    # pela linha de um lado, e redigitado do outro. Nao e regressao deste commit: e o buraco
    # original encolhido de "qualquer sessao Pi" pra "sessao com linha viva no instante do append, e
    # a linha caiu bem nessa janela". Medido: entregar_sync segura o _send_lock por ate PRAZO_ACK+2.0
    # = 5s (pi_inbox.py) — janela de segundos, nao de microssegundos, tempo de sobra pra um drain de
    # reconexao de SSE ou de transicao de hook entrar.
    # CUIDADO no upgrade: so mover o append() pra dentro do _send_lock fecha a corrida entre as DUAS
    # LEITURAS de tem_linha() (o TOCTOU vira leitura unica) mas NAO fecha a duplicata. A entrada
    # nasce delivered=False aqui e so vira True quando o set_delivered(...) do fim desta funcao roda
    # DEPOIS que send_prompt() retorna — tambem fora de qualquer trava. Nesse intervalo (que inclui
    # a espera inteira pelo _send_lock MAIS os ate 5s do entregar_sync) a entrada continua
    # reivindicavel por claim_undelivered. Upgrade completo precisa das DUAS coisas juntas: o
    # append() E o set_delivered() final dentro da MESMA trava — ou claim_undelivered passar a
    # respeitar/disputar o _send_lock. Mover so o append() e necessario, mas sozinho e insuficiente.
    # `pi_inbox.linha_de` (nome primeiro, pane depois), nunca o pane cru: no psmux o pane e `%1`
    # em toda sessao Pi e a busca por pane achava a linha da OUTRA — ver pi_inbox.
    is_pi = (provider == "pi" and pi_inbox.linha_de(name, pane_id) is not None
             and not stripped.startswith("/"))
    if is_pi:
        try:
            entry = PromptQueue(name).append(text, delivered=False, ts=t0)
        except OSError:
            # Fail-soft, mas NAO calado: sem log aqui, um disco ruim degrada pro uuid4-por-tentativa
            # de sempre (vulneravel a duplicata) exatamente na hora em que este conserto deveria
            # entrar em acao — achado da re-revisao 02/08/2026.
            _log.exception("fila indisponivel antes do envio (Pi com linha) name=%s", name)
            entry = None
    # Limpa a flag ANTES de chamar send_prompt (nao so depois de ler, mais abaixo): assim a AUSENCIA
    # dela depois so pode significar "esta chamada nao passou pelo _partial", em vez de herdar o
    # valor de uma chamada anterior na MESMA thread do pool. `_ULTIMA_LIMPEZA` e threading.local
    # (ver o comentario ao lado da declaracao em terminal_input.py) e o pool REUSA thread — sem isto,
    # um "partial" que um dia devolvesse sem passar por `_partial()` leria a sobra de outro envio.
    if hasattr(terminal_input._ULTIMA_LIMPEZA, "limpou"):
        del terminal_input._ULTIMA_LIMPEZA.limpou
    try:
        result = terminal.send_prompt(
            name, text, provider, pane_id=pane_id,
            **({"msg_id": entry["id"]} if entry is not None else {}))
        # DIAG: correlaciona o send com o jsonl pra onde ESTE nome resolve AGORA -> pega o cross-wire
        # (msg indo pro transcript/terminal errado). Best-effort, nunca quebra o envio.
        try:
            from app import tmux as _tmux
            _cwd = next((p["cwd"] for p in _tmux.list_panes_active() if p["name"] == name), "")
            _j, _t = registry.resolve_tracked(name, _cwd)
            _log.info("SEND name=%s -> jsonl=%s tracked=%s result=%s text=%r",
                      name, (_j or "").rsplit("/", 1)[-1], _t, result, text[:80])
        except Exception:
            pass
    except ValueError as e:
        # send_prompt rejeita control chars (ex: '\n'). Sem isto virava 500 -> a msg sumia sem
        # feedback. Agora vira 400 com envelope (o frontend traduz o prefixo e mostra a causa em
        # params.erro). (Multi-linha de verdade: backlog.)
        return {"ok": False, "error": erro("erro_envio_falhou",
                                           f"falha ao enviar: {e}", erro=str(e))}
    if result == "partial":
        # Entrega PARCIAL no fatiamento do Windows: parte do texto ficou no composer e o Enter NAO foi
        # enviado (ver terminal_input.send_prompt). Reporta erro em vez de seguir pro caminho de
        # sucesso, que gravaria a entrada na fila como delivered e afirmaria entrega de uma mensagem
        # cortada. Sem entrada na fila, o drain nao reentra digitando em cima do residuo.
        #
        # A mensagem pro usuario depende do que _partial() conseguiu fazer no composer (mesma thread,
        # lida logo apos o send_prompt acima que a escreveu): o conserto de 07/08/2026 LIMPA o
        # composer antes de devolver "partial", entao a mensagem antiga ("confira o terminal, o
        # residuo esta a vista") ficou FALSA no caso comum — quem abre o terminal depois de uma
        # limpeza confirmada nao acha nada. Le e ja APAGA a flag: e o mesmo apagar que fecha dois
        # achados menores — o valor nao pode ficar escrito pra sempre, e sem apagar aqui o
        # threading.local reusado pelo pool vazaria esta leitura pro proximo envio desta thread que
        # tambem cair em "partial".
        limpou = getattr(terminal_input._ULTIMA_LIMPEZA, "limpou", False)
        if hasattr(terminal_input._ULTIMA_LIMPEZA, "limpou"):
            del terminal_input._ULTIMA_LIMPEZA.limpou
        if entry is not None:
            # A entrada do Pi ja existe (criada acima, ANTES de saber o resultado) — sem isto ficaria
            # delivered=False pra sempre e o proximo drain reentraria digitando em cima do residuo.
            try:
                PromptQueue(name).set_delivered(entry["id"], True)
            except OSError:
                # Achado da re-revisao 02/08/2026: falhar calado aqui e o MESMO bug que o comentario
                # acima descreve (residuo redigitado por cima) voltando sem deixar rastro nenhum.
                _log.exception("fechar entrada apos 'partial' falhou name=%s", name)
        if limpou:
            return {"ok": False, "error": erro("erro_envio_incompleto_limpo",
                                               "envio incompleto: o composer foi limpo e a mensagem NAO foi enviada — pode "
                                               "reenviar sem risco de duplicar.")}
        return {"ok": False, "error": erro("erro_envio_incompleto_composer",
                                           "envio incompleto: parte do texto ficou no composer da sessao e nada foi "
                                           "submetido. Confira o terminal antes de reenviar.")}
    if stripped.startswith("/"):
        # Slash-commands NAO entram na fila — sao meta, nao viram bubble. Excecao /clear: ele reinicia
        # a sessao do Claude Code (novo session-id/transcript), mas a fila e keyed pelo NOME da sessao
        # e sobreviveria -> entradas velhas nunca casariam com o transcript novo e virariam fantasma.
        # Zera a fila junto do /clear pra ela seguir o ciclo da sessao.
        if stripped[1:].split(maxsplit=1)[:1] == ["clear"]:
            try:
                PromptQueue(name).clear()
            except OSError:
                pass
            # ponytail: o sidecar do AskUserQuestion NAO e limpo aqui — /clear abre um transcript com
            # session_id novo, entao o sidecar antigo vira lixo inofensivo (nao reabre nada).
    elif entry is not None:
        # Pi com id estavel: a entrada JA existe (criada antes do send) — so atualiza o delivered,
        # nunca um segundo append (duplicaria a bubble na fila).
        try:
            PromptQueue(name).set_delivered(entry["id"], result == "sent")
        except OSError:
            _log.exception("atualizar fila apos envio falhou name=%s", name)
        if result == "sent":
            threading.Timer(_CONFIRM_GRACE + 0.5, _confirm_and_drain, args=(name,)).start()
        else:
            threading.Thread(target=_drain_session, args=(name,), daemon=True).start()
    else:
        # Registra na fila duravel (sidecar) sempre — aparece como user_msg em ordem e persiste no
        # reload; o merge dedup-a contra o transcript quando o Claude Code grava o prompt. delivered =
        # o send_prompt REALMENTE digitou ("sent"); pane em overlay -> "deferred" (nao tocou a TUI) e a
        # entrada fica pendente pro drain entregar quando o overlay fechar. Falha ao gravar a fila nao
        # quebra o envio.
        try:
            PromptQueue(name).append(text, delivered=(result == "sent"), ts=t0)
        except OSError as e:
            if result != "sent":
                # NAO digitado na TUI (overlay/picker aberto) + sidecar nao gravou = a msg nao esta em
                # lugar NENHUM. Era aqui que o "ok, na fila" mentia: 200 + delivered=False pra uma msg
                # que sumiu. Vira erro (o front mostra), nunca sucesso.
                _log.exception("fila indisponivel e prompt NAO digitado name=%s", name)
                return {"ok": False, "error": erro("erro_fila_nao_digitada",
                                                           f"fila indisponivel e prompt nao foi digitado: {e}",
                                                           erro=str(e))}
            # Digitado na TUI: a msg CHEGOU, o envio nao falhou. Perder o registro so desliga a rede de
            # seguranca (o _confirm_and_drain abaixo nao vai achar o que reconferir) — nao e motivo pra
            # falhar o envio, mas nao pode passar calado.
            _log.exception("append na fila falhou (prompt ja digitado) name=%s", name)
        if result == "sent":
            # Confirmacao de entrega: em ~8s confere se o transcript gravou; engolida -> re-drena.
            threading.Timer(_CONFIRM_GRACE + 0.5, _confirm_and_drain, args=(name,)).start()
        else:
            # Kick: fecha a corrida append-depois-da-transicao — se o estado virou entregavel entre
            # o "deferred" do send_prompt e o append acima, o gatilho daquele ciclo nao viu esta
            # entrada (e sem SSE aberto nao havia gatilho nenhum). O drain re-checa deliverable.
            threading.Thread(target=_drain_session, args=(name,), daemon=True).start()
    # delivered: digitou AGORA na TUI ("sent"); False = ficou na fila durável (sessão ocupada/overlay).
    return {"ok": True, "error": None, "delivered": result == "sent"}


def _provider_of(name: str) -> str:
    """Resolve o provider de uma sessao PELO NOME, barato e sem tmux: sidecar Codex existe -> "codex",
    senao "claude". Default "claude" preserva 100% o caminho tmux de hoje pra qualquer nome que nao seja
    de uma sessao Codex conhecida (regra de ouro: Claude identico, tudo Codex e ramo condicional)."""
    return "codex" if codex_sessions.exists(name) else "claude"


def _pane_info(name: str) -> tuple[str, str | None]:
    """(provider, pane_id) numa leitura só — era `_pane_provider`, que pagava seu próprio
    `tmux list-panes -t <name>` (via `tmux.pane_pid`); agora usa `list_panes_all()` (MESMA chamada
    `list-panes -a` que o antigo `list_panes_active` já fazia — um fork só), e o `pane_id` sai de
    carona, sem tmux novo no caminho quente. Provider do pane (claude/pi) continua lido do /proc
    como antes: o gate de "TUI pronta" do terminal_input casa marcas do rodape do Claude, que o Pi
    nao imprime, e sem saber o provider todo envio a uma sessao Pi queimava os 12s de timeout
    antes de digitar.

    Task 6: resolve pelo pane do AGENTE (`SessionRegistry._agent_pane`, Task 5.5), nao mais pelo
    pane ATIVO — reusa a MESMA resolucao que `registry.list()` ja usa, nao uma terceira. Com um
    split (o shell escondido, ou qualquer split manual), o pane ativo podia ser o do shell: uma
    sessao Pi virava ("claude", pane_id do shell) neste caminho de ENVIO — o gate esperava as
    marcas de rodape do Claude e queimava os 12s por mensagem, e `INBOX.tem_linha(pane_id)`
    falhava (derrubava a linha rapida do Pi), porque o pane_id devolvido era do pane errado.

    Erro/pane sumido -> ("claude", None) — comportamento de hoje, marcas do Claude, sem pane_id
    (cai pra tecla, igual a antes desta task).

    Revisao final (I1): o corpo mudou de casa pro `agentpane.pane_info` — o drain da fila duravel e
    o adapter do Pi precisavam da MESMA resolucao e estavam no pane ativo. Esta funcao fica como o
    nome que as rotas (e os testes) ja conhecem."""
    from app import agentpane
    return agentpane.pane_info(name)


async def _send_one_codex(name: str, text: str) -> dict:
    """Envio de prompt pra sessao Codex pela TUI no tmux. Registra na fila duravel
    (aparece como user_msg em ordem e persiste no reload; o
    merge dedup-a contra o rollout do Codex) e entrega pela TUI no tmux SE a sessao esta idle;
    senao deixa pendente pro drain-on-complete entregar quando o turno terminar. Codex nao tem
    slash-commands do Claude -> envia o texto como esta. Nunca levanta (mesmo contrato do _send_one pro
    broadcast: devolve ok/error por sessao).

    IMPORTANT 2: PromptQueue.append/set_delivered fazem I/O de arquivo sincrono com lock -- chamados
    direto aqui (corrotina) bloqueariam o event loop. Mesmo padrao de to_thread do drain do Codex."""
    adapter = get_adapter("codex")
    try:
        deliverable = await adapter.deliverable(name)
    except Exception:
        # Adapter quebrado/fora do ar nao pode passar calado: sem o log, um erro aqui virava um
        # "delivered: false" indistinguivel de turno em andamento. Segue como NAO-entregavel -> a
        # fila abaixo segura o prompt e o drain-on-complete tenta de novo no proximo idle.
        _log.exception("codex deliverable falhou name=%s", name)
        deliverable = False
    # Enfileira sempre como pendente; so marca entregue apos a TUI REALMENTE receber o prompt.
    try:
        entry = await asyncio.to_thread(PromptQueue(name).append, text, delivered=False)
    except OSError as e:
        # Mesma regra do _send_one: sidecar nao gravou + NAO entregavel = a msg nao esta em lugar
        # NENHUM, e responder "ok, na fila" era a mentira que o eeba30a tirou do caminho Claude.
        # Vira erro (o front mostra), nunca sucesso.
        if not deliverable:
            _log.exception("fila indisponivel e prompt NAO entregue name=%s", name)
            return {"ok": False, "error": erro("erro_fila_nao_entregue",
                                                       f"fila indisponivel e prompt nao foi entregue: {e}",
                                                       erro=str(e))}
        # Entregavel: a TUI abaixo ainda leva o texto, entao a msg CHEGA. Perder o registro so
        # desliga a rede de seguranca (o drain-on-complete nao acha o que reconferir) — nao e motivo
        # pra falhar o envio, mas nao pode passar calado.
        _log.exception("append na fila falhou (prompt sera entregue) name=%s", name)
        entry = None
    if not deliverable:
        # turno em andamento -> fica pendente na fila; o drain-on-complete entrega no proximo idle.
        return {"ok": True, "error": None, "delivered": False}
    try:
        result = await adapter.send_prompt(name, text)
    except Exception as e:
        _log.exception("codex send_prompt falhou name=%s", name)
        return {"ok": False, "error": erro("erro_envio_falhou",
                                           f"falha ao enviar: {e}", erro=str(e))}
    if result == "sent":
        if entry is not None:
            # turno iniciou -> marca entregue pra o drain-on-complete nao reenviar a mesma entrada.
            try:
                await asyncio.to_thread(PromptQueue(name).set_delivered, entry["id"], True)
            except OSError:
                pass
    elif entry is None:
        # "deferred" (corrida idle->working entre o deliverable e o send) + sidecar morto: o texto NAO
        # foi digitado E nao ha entrada pendente pro drain-on-complete drenar -- a msg nao esta em lugar
        # NENHUM. Aqui morre a suposicao do append la em cima ("entregavel -> a TUI leva o texto"):
        # o deferred e exatamente o caso em que nao levou. Ultimo ponto onde o 200 "na fila"
        # ainda seria a mentira do eeba30a.
        _log.error("prompt deferido sem entrada na fila — NAO foi entregue name=%s", name)
        return {"ok": False, "error": erro("erro_fila_nao_entregue",
                                                   "fila indisponivel e o turno nao aceitou o prompt: nao foi entregue")}
    # "deferred" COM entrada na fila: fica pendente (delivered ja e False) -> drain-on-complete entrega.
    return {"ok": True, "error": None, "delivered": result == "sent"}


def _session_exists(name: str) -> bool:
    """Sessão existe DE VERDADE (pane tmux vivo ou sidecar Codex)? Sem esta guarda o /input aceitava
    qualquer nome e enfileirava no VOID: 'ok' pra sessão morta = recado órfão que só seria entregue
    se um dia nascesse outra sessão com o mesmo nome (foi exatamente como um recado 'se perdeu')."""
    from app import tmux
    return codex_sessions.exists(name) or tmux.has_session(name)


@app.post("/api/sessions/{name}/input", dependencies=[Depends(require_auth)])
async def input_prompt(name: str, body: InputBody):
    # Ramifica por provider: Claude via _send_thread (_send_one e SYNC/bloqueante — tmux), Codex via
    # _send_one_codex (async, app-server). Default "claude" pra qualquer nome nao-Codex.
    # NUCLEO SAGRADO: o envio NUNCA disputa executor com feature. _send_thread e um pool DEDICADO
    # (nao o default do asyncio, que a decoracao — git_summary/capture_pane — pode ocupar). Assim um
    # git status pendurado + refine (60s, pool do anyio) + check (600s, thread propria) nao seguram
    # o POST /input. Ver _send_thread.
    if not await _send_thread(_session_exists, name):
        raise HTTPException(404, detail=erro("erro_sessao_recado_nao_enfileirado", "sessão não encontrada — recado NÃO enfileirado"))
    if _provider_of(name) == "codex":
        res = await _send_one_codex(name, body.text)
    else:
        res = await _send_thread(_send_one, name, body.text)
    if not res["ok"]:
        raise HTTPException(400, res["error"])
    # delivered: True = digitou agora na TUI; False = na fila durável (entrega no próximo idle).
    return {"ok": True, "delivered": res.get("delivered", False)}


@app.post("/api/sessions/{name}/steer", dependencies=[Depends(require_auth)])
async def steer_session(name: str):
    """`ctrl-s` avulso numa sessao Kimi: a msg que ja esta na fila da TUI entra no turno em curso.

    Rota propria e nao um /input sem texto: aqui NAO se digita nada — e uma tecla so, pra uma msg
    que o usuario ja mandou. Passa pelo mesmo pool dedicado do envio (é tmux, bloqueante).

    409 (e nao 400) fora do Kimi: a sessao existe e o pedido e valido, so nao ha "steer" naquela TUI
    — mesmo contrato das rotas que recusam com o painel do terminal aberto. O front nem mostra o
    botao fora do Kimi; isto e a defesa de quem chama a API na mao."""
    if not await _send_thread(_session_exists, name):
        raise HTTPException(404, "sessão não encontrada")
    provider, _ = await _send_thread(_pane_info, name)
    if provider != "kimi":
        raise HTTPException(409, "só sessão Kimi tem steer (ctrl-s)")
    # `is False` e nao `not ...`: o unico produtor de False e o tmux recusando a tecla; um dublê de
    # teste que devolve None nao pode virar erro. Sem esta checagem a rota afirmava entrega de um
    # ctrl-s que nunca saiu (pane morto) — o chip sumia da tela e a msg ficava parada na fila.
    r = await _send_thread(terminal_input.steer_now, name)
    if r is False:
        raise HTTPException(502, "o terminal recusou a tecla — a mensagem continua na fila")
    if r == "sem-fila":
        # A bolha "na fila" existia mas a TUI nao tinha o marcador (a msg ja entrou no turno por
        # outra via, ou o wire ainda nao flushou): NAO confirma nada — quem decide e o reconcile
        # do transcript, nunca um carimbo sobre promocao que nao aconteceu.
        return {"ok": True, "promoted": False}
    # Promovido de verdade: baixa a fila duravel AGORA. O Kimi so grava o append_message da msg
    # steerada no FIM do turno (medido: 34s depois do ctrl-s), entao esperar o transcript confirmar
    # deixava o chip "N na fila" aceso o turno inteiro — e clicavel, sobre um no-op.
    n = await _send_thread(PromptQueue(name).confirm_delivered)
    return {"ok": True, "promoted": True, "confirmed": n}


@app.post("/api/broadcast", dependencies=[Depends(require_auth)])
async def broadcast(body: BroadcastBody):
    """Fan-out de UM prompt pra N sessoes (feature #9): mesma sequencia do /input, em loop — sessao
    ocupada enfileira na fila duravel dela (crash-safe), sessao ociosa recebe na hora, sem mecanismo
    de entrega novo. Ramifica por provider por nome (Claude via to_thread, Codex via _send_one_codex),
    reportando por-sessao sem abortar as demais. Slash-commands ficam FORA (rota por sessao so):
    "/clear" pra N sessoes de uma vez e ambiguo/perigoso (o front ja desabilita o envio; isto e defesa
    em profundidade)."""
    if body.text.lstrip().startswith("/"):
        raise HTTPException(400, detail=erro("erro_broadcast_slash", "broadcast nao suporta slash-commands: envie por sessao"))
    results: dict[str, dict] = {}
    for name in body.names:
        # Mesma guarda do /input: nome sem sessão viva -> erro POR SESSÃO (não enfileira no void).
        if not await _send_thread(_session_exists, name):
            results[name] = {"ok": False, "error": erro("erro_sessao_inexistente", "sessão não encontrada"), "delivered": False}
            continue
        if _provider_of(name) == "codex":
            results[name] = await _send_one_codex(name, body.text)
        else:
            results[name] = await _send_thread(_send_one, name, body.text)
    return {"results": results}


class PairBody(_StrictBody):
    # peer (1) OU peers (N) — peers vence; peer fica por compat (hangar-send --pair manda um só).
    peer: str = ""
    peers: list[str] = []
    task: str = ""
    replace_task: bool = False


def _group_text(me: str, others: list[str], task: str) -> str:
    # Par remoto (srv::sessao): o contrato não sincroniza cross-server, então a linha dele some.
    # contract_path_for devolve None em sidecar legado sem gid — str(None) viraria "None" no prompt.
    cross = any(peers.is_remote(o) for o in others)
    caminho = None if cross else contract_path_for(me)
    return pair_texto.texto_grupo(me, others, task, str(caminho) if caminho else None)


async def _deliver(name: str, text: str) -> dict | None:
    # Mesma esteira do /input (fila durável se ocupada), ramificada por provider.
    # Devolve o envelope {code, params, msg} (ou string crua de erro tecnico ainda nao migrado)
    # ou None — _send_one/_send_one_codex NUNCA levantam, reportam no dict; engolir isso fazia o
    # pareamento dizer "ok" com o aviso jamais entregue.
    if _provider_of(name) == "codex":
        res = await _send_one_codex(name, text)
    else:
        res = await _send_thread(_send_one, name, text)
    return None if res.get("ok") else (res.get("error")
                                           or erro("erro_envio_falhou_desconhecida", "falha desconhecida no envio"))


@app.post("/api/sessions/{name}/pair", dependencies=[Depends(require_auth)])
async def pair_session(name: str, body: PairBody):
    """Junta `name` e peer(s) num GRUPO de trabalho (une os grupos existentes de todos) e injeta
    em CADA membro o prompt do grupo atualizado — a partir daí trocam recados via hangar-send por
    iniciativa própria, dentro do escopo da tarefa. Badge `pair_peers` aparece na lista."""
    others = [p for p in dict.fromkeys(body.peers or ([body.peer] if body.peer else [])) if p]
    if not others:
        raise HTTPException(400, detail=erro("erro_peer_nao_informado", "informe peer ou peers"))
    if name in others:
        raise HTTPException(400, detail=erro("erro_autopareamento", "não dá pra parear uma sessão com ela mesma"))
    if any(peers.is_remote(o) for o in others):
        # Cross-server é 1:1 puro (um peer remoto, sem misturar grupo local) — grupo cross-server de
        # N fica pra fase 2. ponytail: 1:1 cobre "trabalhar junto entre máquinas"; N quando doer.
        if len(others) != 1:
            raise HTTPException(400, detail=erro("erro_pareamento_cross_1_1",
                                             "pareamento cross-server é 1:1 por enquanto: um peer remoto, "
                                             "sem misturar com grupo local"))
        if not settings.server_id:
            raise HTTPException(400, detail=erro("erro_pareamento_server_id_ausente",
                                             "CP_SERVER_ID ausente no backend/.env — obrigatório pra "
                                             "pareamento cross-server (é o endereço de resposta srv::sessao)"))
        return await _pair_cross_server(name, others[0], body.task, body.replace_task)
    names = {s.name for s in await asyncio.to_thread(registry.list)}
    missing = [p for p in [name, *others] if p not in names]
    if missing:
        raise HTTPException(404, detail=erro("erro_sessao_nao_encontrada_detalhe", f"sessão não encontrada: {', '.join(missing)}", detalhe=", ".join(missing)))
    # join_group: snapshot + join na MESMA seção crítica (em seções separadas, um join concorrente
    # na janela entre elas entrava no grupo fora do snapshot e um rollback posterior não o
    # reverteria). O snapshot volta pra cá pra desfazer se o aviso não chegar em ninguém.
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, others, body.task, substituir_task=body.replace_task)
    except pair.PairMixError as e:
        # Uma das sessões locais já está pareada cross-server (1:1) — não dá pra fundir em grupo local.
        raise HTTPException(400, str(e))
    except pair.TaskConflito as e:
        raise HTTPException(409, detail=erro("erro_pareamento_tarefa_existente",
                                             f"o grupo já tem tarefa: {e.existente!r} — repita com "
                                             f"--substituir-tarefa pra trocar", existente=e.existente))
    link = await asyncio.to_thread(lambda: PairLink(name).get() or {})
    task = link.get("task", body.task)
    # Protocolo completo só pra quem estava SOLTO; veterano ganha uma linha com quem entrou. Quem
    # não teve mudança de peers nem de tarefa não recebe nada — o protocolo pós-/clear é do hook.
    avisos: list[tuple[str, str]] = []
    for m in members:
        antes = snap.get(m)
        outros = [x for x in members if x != m]
        if antes is None:
            avisos.append((m, _group_text(m, outros, task)))
            continue
        entraram = [x for x in outros if x not in antes["peers"]]
        if entraram:
            avisos.append((m, pair_texto.texto_entrada(entraram, members, task)))
        elif antes.get("task", "") != task:
            avisos.append((m, pair_texto.texto_tarefa_atualizada(task)))
    errs = []
    for m, texto in avisos:
        e = await _deliver(m, texto)
        if e:
            errs.append({"sessao": m, "erro": e})
    if avisos and len(errs) == len(avisos):
        # NINGUÉM foi avisado -> grupo fantasma; restaura o estado anterior e reporta.
        await asyncio.to_thread(pair.restore, snap)
        raise HTTPException(502, detail=erro("erro_pareamento_desfeito",
                            f"pareamento desfeito: falha ao avisar as sessões "
                            f"({'; '.join(f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs)})",
                            avisos=errs))
    return {"ok": True, "members": members,
            "warning": erro("erro_pareamento_aviso_parcial",
                            "aviso falhou em: " + "; ".join(
                                f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs),
                            avisos=errs)
            if errs else None}


async def _pair_cross_server(name: str, peer: str, task: str, replace_task: bool) -> dict:
    """Pareamento 1:1 entre máquinas. Registra o vínculo LOCAL (name.json peers=[srv::sessao];
    sidecar do remoto vive na máquina dele) e chama o /pair-remote do backend peer pra registrar o
    reverso + injetar o protocolo lá. Falha na chamada remota desfaz o vínculo local (mesmo racional
    do 'grupo fantasma' do pair local). Transporte já provado pelo hangar-send cross-server (peers.json)."""
    local_names = {s.name for s in await asyncio.to_thread(registry.list)}
    if name not in local_names:
        raise HTTPException(404, detail=erro("erro_sessao_nao_encontrada_detalhe", f"sessão não encontrada: {name}", detalhe=name))
    srv, sess = peers.split_addr(peer)
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, [peer], task, substituir_task=replace_task)
    except pair.PairMixError as e:
        # `name` já está num grupo local (ou já pareada cross-server): não dá pra cross-parear.
        raise HTTPException(400, str(e))
    except pair.TaskConflito as e:
        raise HTTPException(409, detail=erro("erro_pareamento_tarefa_existente",
                                             f"o grupo já tem tarefa: {e.existente!r} — repita com "
                                             f"--substituir-tarefa pra trocar", existente=e.existente))
    link = await asyncio.to_thread(lambda: PairLink(name).get() or {})
    task = link.get("task", task)
    initiator = f"{settings.server_id}::{name}"
    try:
        await asyncio.to_thread(
            peers.call, srv, "POST", f"/api/sessions/{sess}/pair-remote",
            {"initiator": initiator, "task": task})
    except peers.PeerError as e:
        await asyncio.to_thread(pair.restore, snap)
        if e.transport:
            # Rede caiu / resposta perdida: o /pair-remote PODE ter comitado no peer antes de a
            # resposta se perder. Desfiz este lado; tento limpar o outro por garantia (best-effort —
            # se o peer está mesmo inacessível isto também falha, e aí o usuário desapareia lá na mão).
            try:
                await asyncio.to_thread(peers.call, srv, "POST",
                                        f"/api/sessions/{sess}/unpair-remote", {"peer": initiator})
            except peers.PeerError:
                pass
            raise HTTPException(502, detail=erro("erro_pareamento_nao_confirmado",
                                             f"pareamento NÃO confirmado (falha de rede com '{srv}'): desfeito "
                                             f"deste lado; se o peer tiver ficado pareado, rode unpair lá. ({e})",
                                             srv=srv, erro=str(e)))
        raise HTTPException(502, detail=erro("erro_pareamento_rejeitado", f"pareamento desfeito (peer rejeitou): {e}", erro=str(e)))
    # Reverso registrado. Injeta o protocolo NESTE lado; se este falhar (sessão morreu na janela), o
    # vínculo já vale dos dois lados — só avisa, não desfaz (o par remoto já sabe).
    warn = None
    e = await _deliver(name, _group_text(name, [peer], task))
    if e:
        warn = erro("erro_pareamento_aviso_local",
                    f"vínculo criado, mas o aviso local falhou ({name}: {_erro_texto(e)}) — refaça o pair se precisar",
                    sessao=name, erro=e)
    return {"ok": True, "members": members, "warning": warn}


class PairRemoteBody(_StrictBody):
    initiator: str        # 'srv::nome' — quem iniciou o pareamento, na máquina remota
    task: str = ""


@app.post("/api/sessions/{name}/pair-remote", dependencies=[Depends(require_auth)])
async def pair_remote(name: str, body: PairRemoteBody):
    """Lado RECEPTOR do pareamento cross-server: registra `name` (sessão LOCAL) pareada ao iniciador
    remoto `body.initiator` (srv::nome) e injeta o protocolo. NÃO chama de volta (o iniciador já
    registrou o próprio lado — chamar de volta recursaria). Chamado só pelo backend do outro server
    via peers.call, autenticado pelo token do peers.json."""
    if not peers.is_remote(body.initiator):
        raise HTTPException(400, detail=erro("erro_initiator_invalido", "initiator precisa ser qualificado (srv::nome)"))
    local_names = {s.name for s in await asyncio.to_thread(registry.list)}
    if name not in local_names:
        raise HTTPException(404, detail=erro("erro_sessao_nao_encontrada_detalhe", f"sessão não encontrada: {name}", detalhe=name))
    try:
        # substituir_task=True: a task que chega aqui é a combinada do iniciador, sempre vence.
        members, snap = await asyncio.to_thread(pair.join_group, name, [body.initiator], body.task, substituir_task=True)
    except pair.PairMixError as e:
        # `name` já está num grupo local aqui — não pode virar par cross-server de outra máquina.
        raise HTTPException(409, str(e))
    e = await _deliver(name, _group_text(name, [body.initiator], body.task))
    if e:
        await asyncio.to_thread(pair.restore, snap)
        raise HTTPException(502, detail=erro("erro_pareamento_aviso_falhou",
                                    f"pareamento desfeito: falha ao avisar '{name}': {_erro_texto(e)}",
                                    nome=name, erro=e))
    return {"ok": True, "members": members}


class UnpairRemoteBody(_StrictBody):
    peer: str             # 'srv::nome' que saiu do pareamento, na máquina remota


@app.post("/api/sessions/{name}/unpair-remote", dependencies=[Depends(require_auth)])
async def unpair_remote(name: str, body: UnpairRemoteBody):
    """`name` (local) tinha um par remoto que saiu — remove o vínculo local e avisa. Idempotente
    (sair de quem não está pareado é no-op). Chamado pelo backend peer no unpair do outro lado."""
    # Defesa: só dissolve se `name` está MESMO pareado com quem diz estar saindo. Sem isto, um
    # /unpair-remote perdido, duplicado ou com peer errado dissolvia um pareamento legítimo de `name`
    # e mandava aviso falso (era o vetor de dano cross-máquina do achado crítico do review).
    link = await asyncio.to_thread(lambda: PairLink(name).get())
    if not link or body.peer not in (link.get("peers") or []):
        return {"ok": True, "warning": None, "noop": f"'{name}' não está pareado com '{body.peer}'"}
    ex = await asyncio.to_thread(pair.leave, name)
    warn = None
    if ex:
        e = await _deliver(name, f"[de: hangar] '{body.peer}' saiu do pareamento. "
                                 "Volte a operar independente; use hangar-send só quando o usuário pedir.")
        if e:
            warn = erro("erro_pareamento_aviso_unpair", f"{name}: {_erro_texto(e)}",
                        sessao=name, erro=e)
    return {"ok": True, "warning": warn}


# Anti-tempestade: o prompt manda nunca responder [grupo:] com --group, mas prompt é disciplina,
# não trava. 5 avisos/min por grupo cobre "terminei" + "contrato atualizado" de N membros; um loop
# N×N passa disso em segundos. ponytail: dict em memória, zera no restart — é o que basta.
_GROUP_MAX_NA_JANELA = 5
_GROUP_JANELA_S = 60
_group_envios: dict[str, list[float]] = {}


def _group_estourou(gid: str, agora: float) -> bool:
    ts = [t for t in _group_envios.get(gid, []) if agora - t < _GROUP_JANELA_S]
    ts.append(agora)
    _group_envios[gid] = ts
    return len(ts) > _GROUP_MAX_NA_JANELA


class GroupMsgBody(_StrictBody):
    text: str
    # O hangar-send diz se o REMETENTE tem socket ($CLAUDE_CODE_MESSAGING_SOCKET); o backend
    # decide o resto: peer local com inbox = caminho nativo alcança os dois lados = não digita
    # nele, devolve em `pulados` pro modelo mandar por SendMessage. Mesmo critério do 1:1.
    remetente_nativo: bool = False
    forcar_tmux: bool = False


@app.post("/api/sessions/{name}/group-message", dependencies=[Depends(require_auth)])
async def group_message(name: str, body: GroupMsgBody):
    """Aviso pro GRUPO todo (hangar-send --group): entrega o texto a CADA companheiro de `name` numa
    tacada, como `[grupo: <name>]`. Unidirecional por contrato (o prompt instrui a NUNCA responder
    um [grupo:] com --group) — é o que impede o loop de N sessões se avisando em cascata.
    Slash-command fora (mesmo racional do /broadcast)."""
    if body.text.lstrip().startswith("/"):
        raise HTTPException(400, detail=erro("erro_group_message_slash", "group-message não suporta slash-commands"))
    txt = body.text.lstrip()
    if txt.startswith("[grupo:") or txt.startswith("[de:"):
        raise HTTPException(400, detail=erro("erro_group_message_resposta",
                                             "aviso de grupo não pode reencaminhar um [grupo:]/[de:] — responda 1:1"))
    link = await asyncio.to_thread(lambda: PairLink(name).get())
    membros = link.get("peers") if link else None
    if not membros:
        raise HTTPException(404, detail=erro("erro_sessao_sem_grupo", "sessão não está num grupo"))
    if _group_estourou(link.get("gid") or name, time.time()):
        raise HTTPException(429, detail=erro("erro_group_message_tempestade",
                                             f"mais de {_GROUP_MAX_NA_JANELA} avisos de grupo em "
                                             f"{_GROUP_JANELA_S}s — parece loop; espere ou responda 1:1",
                                             max=_GROUP_MAX_NA_JANELA, janela=_GROUP_JANELA_S))
    from app.registry import inbox_socket_of
    pulados: list[str] = []
    if body.remetente_nativo and not body.forcar_tmux:
        for p in membros:
            if not peers.is_remote(p) and await asyncio.to_thread(inbox_socket_of, p):
                pulados.append(p)
    text = f"[grupo: {name}] {body.text}"
    results: dict[str, dict] = {}
    for p in [x for x in membros if x not in pulados]:
        if not await _send_thread(_session_exists, p):
            results[p] = {"ok": False, "error": erro("erro_sessao_inexistente", "sessão não encontrada"), "delivered": False}
            continue
        if _provider_of(p) == "codex":
            results[p] = await _send_one_codex(p, text)
        else:
            results[p] = await _send_thread(_send_one, p, text)
    failed = [{"sessao": n, "erro": r.get("error")} for n, r in results.items() if not r.get("ok")]
    return {"ok": True, "peers": membros, "pulados": pulados,
            "warning": erro("erro_pareamento_grupo_falha",
                            "falha em: " + "; ".join(
                                f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in failed),
                            avisos=failed)
            if failed else None}


# ----------------------------------------------------------------- orquestração (política + papéis)

def _catalogo_claude_cache(dir_conta: Path) -> tuple[list[dict], bool]:
    """Leitor do cache do picker pro inventário — o MESMO cache de /api/model-options."""
    cacheado = _models_cache_get(_chave_config(dir_conta))
    if cacheado is not None:
        return list(cacheado.get("models") or []), False
    return orq_politica._modelos_claude_reduzidos(dir_conta)


def _inventario() -> list[orq_politica.ContaInventario]:
    return orq_politica.inventario(_catalogo_claude_cache)


class PoliticaContaBody(_StrictBody):
    provider: str
    apelido: str = ""
    modelos: list[str] = ["*"]
    trocar: bool = True
    ligada: bool = True
    mtime: float


@app.get("/api/orquestracao/politica", dependencies=[Depends(require_auth)])
async def orq_politica_get():
    texto, mtime = orq_md.ler_arquivo(orq_politica.caminho())
    inv = await asyncio.to_thread(_inventario)
    return {"arquivo": str(orq_politica.caminho()), "mtime": mtime,
            "politica": [asdict(c) for c in orq_politica.ler(texto)],
            "inventario": [asdict(i) for i in inv]}


@app.put("/api/orquestracao/politica/{conta}", dependencies=[Depends(require_auth)])
async def orq_politica_put(conta: str, body: PoliticaContaBody):
    inv = await asyncio.to_thread(_inventario)
    item = next((i for i in inv if i.provider == body.provider
                 and orq_md.normalizar(i.conta) == orq_md.normalizar(conta)), None)
    if item is None:
        raise HTTPException(400, detail=erro("erro_orq_conta_desconhecida",
                                             f"conta {conta!r} ({body.provider}) não existe nesta máquina"))
    modelos = tuple(m.strip() for m in body.modelos if m.strip()) or ("*",)
    if "*" not in modelos and not item.reduced and item.modelos:
        conhecidos = {m["id"] for m in item.modelos}
        ruim = [m for m in modelos if m not in conhecidos]
        if ruim:
            raise HTTPException(400, detail=erro("erro_orq_modelo_desconhecido",
                                                 f"modelo(s) fora do catálogo da conta: {', '.join(ruim)}",
                                                 modelos=ruim))
    try:
        for v in (conta, body.apelido, *modelos):
            orq_md.validar_celula(v)
        if body.ligada:
            c = orq_politica.ContaPolitica(item.conta, body.provider, body.apelido, modelos, body.trocar)
            mtime = await asyncio.to_thread(orq_politica.gravar_conta, c, body.mtime)
        else:
            mtime = await asyncio.to_thread(orq_politica.desligar, item.conta, body.mtime)
    except ValueError as e:
        raise HTTPException(400, detail=erro("erro_orq_celula_invalida", str(e)))
    except orq_md.Conflito:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou",
                                             "o arquivo mudou desde a leitura — recarregue"))
    return {"ok": True, "mtime": mtime}


class PapelBody(_StrictBody):
    papel: str
    sessao: str = ""
    provider: str
    conta: str
    modelo: str = ""
    esforco: str = ""
    mtime: float


def _gid_de(name: str) -> str:
    link = PairLink(name).get()
    if link and link.get("gid"):
        return link["gid"]
    # Sem grupo, a tela edita o TIME PADRÃO (regras-padrao.md): é dali que o árbitro parte ao
    # montar o próximo grupo — configurar antes de começar foi pedido do usuário (26/08/2026).
    return orq_papeis.gid_por_sessao(name) or orq_papeis.GID_PADRAO


def _papeis_de(gid: str) -> tuple[str, float, list[orq_papeis.Papel]]:
    texto, mtime = orq_md.ler_arquivo(orq_papeis.regras_path(gid))
    return texto, mtime, orq_papeis.ler(texto)


@app.get("/api/sessions/{name}/orq", dependencies=[Depends(require_auth)])
async def orq_get(name: str):
    gid = await asyncio.to_thread(_gid_de, name)
    _texto, mtime, papeis = await asyncio.to_thread(_papeis_de, gid)
    # A lista fresca do registry (sem git nem pane): `casar_viva` só precisa de nome + last_activity.
    infos = await asyncio.to_thread(registry.list)
    arbitro = next((p for p in papeis if p.e_arbitro()), None)
    return {
        "gid": gid, "arquivo": str(orq_papeis.regras_path(gid)), "mtime": mtime,
        "arbitro": orq_papeis.casar_viva(arbitro, infos) if arbitro else None,
        "papeis": [{**asdict(p), "viva": orq_papeis.casar_viva(p, infos),
                    "id_cota": orq_politica.id_cota(p.provider, p.conta)} for p in papeis],
    }


def _recado_arbitro(novos: list[orq_papeis.Papel], gid: str) -> str:
    # Prefixo `[painel: orquestração]` = mesma família do `[de: <sessão>]` do hangar-send: o front
    # desenha o chip "configuração · orquestração" e a sessão sabe que é recado automático.
    linhas = "; ".join("`" + p.papel + "` agora é provider `" + p.provider + "`, conta `" + p.conta
                       + "`, modelo `" + (p.modelo or "-") + "`, esforço `" + (p.esforco or "-") + "`"
                       for p in novos)
    return ("[painel: orquestração] A configuração de modelos do grupo mudou no painel: " + linhas
            + ". Releia `" + str(orq_papeis.regras_path(gid))
            + "`. Aplicação, papel a papel: sessão desse papel PARADA (idle) → feche-a e abra outra já na "
            "configuração nova (o Claude não troca conta/modelo com a sessão aberta); sessão "
            "TRABALHANDO → deixe terminar a tarefa atual e a próxima sessão desse papel nasce na nova. "
            "A linha já está gravada: não reescreva a tabela. "
            "Se o papel for o seu (árbitro): termine a tarefa em curso, escreva no seu registro "
            "(o diário do grupo, seja grupo-<gid>.md ou o registro.md do diretório durável) a seção "
            "'Passagem para o árbitro seguinte' (até 25 linhas: Task e portão, sessões vivas por "
            "papel, HEAD e git status, pendências, decisões recentes, caminhos do plano/regras/"
            "registro), abra o sucessor na configuração nova com kick-off apontando pra essa seção, "
            "troque a linha `árbitro` da tabela pro nome dele, avise executor e revisor vivos quem é "
            "o árbitro agora, e pare de despachar — rito 'Sucessão do árbitro' da skill.")


class PapelItem(_StrictBody):
    papel: str
    sessao: str = ""
    provider: str
    conta: str
    modelo: str = ""
    esforco: str = ""
    # Vazio = o papel roda numa conta só (formato original). "1", "2", "3"… = rodízio, e a Task N
    # cabe à conta de índice (N-1) % total. "par" = todas ao mesmo tempo.
    vez: str = ""


class PapeisBody(_StrictBody):
    papeis: list[PapelItem]
    mtime: float
    # Falso = grava e NÃO acorda o árbitro. É o "salvar e continuar montando o time": quem monta
    # o grupo mexe em vários papéis em sessões separadas da tela, e um recado por rodada de edição
    # faz o árbitro parar o que está fazendo pra ler meia configuração. O aviso vai no fim, uma vez.
    avisar: bool = True


async def _aplicar_papeis(name: str, itens: list[PapelItem], mtime_lido: float,
                          avisar: bool = True) -> dict:
    """Grava TODAS as linhas numa escrita só e manda UM recado ao árbitro listando as mudanças —
    o usuário edita vários papéis e salva no fim (medido em 26/08/2026: salvar um por vez
    descartava o resto sem aviso). `avisar=False` grava sem acordar o árbitro."""
    if not itens:
        raise HTTPException(400, detail=erro("erro_orq_celula_invalida", "nenhum papel"))
    gid = await asyncio.to_thread(_gid_de, name)
    texto, _mtime, papeis = await asyncio.to_thread(_papeis_de, gid)
    novos: list[orq_papeis.Papel] = []
    try:
        for it in itens:
            # Herda a sessão da linha de MESMO papel e MESMA vez: num papel que reveza, cada conta
            # tem a sua, e casar só pelo papel copiaria a sessão da primeira pras demais.
            #
            # Consequência a saber ao escrever um chamador novo: converter um papel de conta única
            # (`vez` vazia) em rodízio (`vez` = "1") não casa linha nenhuma, então a sessão NÃO é
            # herdada — quem faz essa conversão tem de mandar `sessao` explícito, como o painel faz
            # em `adicionarConta`. Omitir ali perderia a sessão viva do papel, calado.
            vez = it.vez.strip()
            atual = next((p for p in papeis
                          if orq_md.normalizar(p.papel) == orq_md.normalizar(it.papel)
                          and orq_md.normalizar(p.vez) == orq_md.normalizar(vez)), None)
            novo = orq_papeis.Papel(it.papel.strip(), (it.sessao or (atual.sessao if atual else "")).strip(),
                                    it.provider.strip().lower(), it.conta.strip(),
                                    it.modelo.strip(), it.esforco.strip(), vez)
            motivo = await asyncio.to_thread(orq_politica.permitido, novo.provider, novo.conta, novo.modelo, novo.esforco)
            if motivo:
                raise HTTPException(400, detail=erro(motivo, "a política de contas não permite esta escolha: " + novo.papel))
            # ponytail: validar_celula roda dentro de escrever_papel — texto do cliente nunca chega
            # ao arquivo nem ao recado sem passar por ali.
            texto = orq_papeis.escrever_papel(texto, novo)
            novos.append(novo)
        mtime = await asyncio.to_thread(orq_md.gravar, orq_papeis.regras_path(gid), texto, mtime_lido)
    except ValueError as e:
        raise HTTPException(400, detail=erro("erro_orq_celula_invalida", str(e)))
    except orq_md.Conflito:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou",
                                             "o contrato mudou desde a leitura — recarregue"))
    # Gravado. Sem aviso, para aqui: o arquivo é a verdade do grupo, e o árbitro relê o contrato
    # quando for usar — o recado é conveniência, não o canal de entrega da configuração.
    if not avisar:
        return {"papeis": [asdict(p) for p in novos], "papel": asdict(novos[0]), "mtime": mtime,
                "arbitro": None, "aviso": "nao_avisado", "erro": None}
    arb = next((p for p in orq_papeis.ler(texto) if p.e_arbitro()), None)
    infos = await asyncio.to_thread(registry.list)
    # Time padrão não tem árbitro vivo pra avisar: uma sessão que por acaso case o nome não é dele.
    arbitro = orq_papeis.casar_viva(arb, infos) if arb and gid != orq_papeis.GID_PADRAO else None
    aviso, err = "sem_arbitro", None
    if arbitro:
        res = await _send_thread(_send_one, arbitro, _recado_arbitro(novos, gid))
        if res["ok"]:
            aviso = "enviado" if res.get("delivered") else "enfileirado"
        else:
            aviso, err = "falhou", res["error"]
    return {"papeis": [asdict(p) for p in novos], "papel": asdict(novos[0]), "mtime": mtime,
            "arbitro": arbitro, "aviso": aviso, "erro": err}


@app.post("/api/sessions/{name}/orq/papel", dependencies=[Depends(require_auth)])
async def orq_papel_set(name: str, body: PapelBody):
    return await _aplicar_papeis(name, [PapelItem(**body.model_dump(exclude={"mtime"}))], body.mtime)


@app.post("/api/sessions/{name}/orq/papeis", dependencies=[Depends(require_auth)])
async def orq_papeis_set(name: str, body: PapeisBody):
    return await _aplicar_papeis(name, body.papeis, body.mtime, body.avisar)


class ComecarBody(_StrictBody):
    mtime: float = 0.0


@app.post("/api/sessions/{name}/orq/comecar", dependencies=[Depends(require_auth)])
async def orq_comecar(name: str, body: ComecarBody):
    """Põe ESTA sessão pra tocar a orquestração como árbitra. Quem planejou vira árbitro (é o que a
    skill manda: a sessão da fase 1 assume a fase 2), então o alvo é a própria sessão, não uma nova
    — uma sessão nova começaria relendo tudo que esta já sabe.

    Recusa sem plano: o árbitro despacha Tasks e as Tasks vêm do plano. Um botão que acorda alguém
    sem ter o que despachar é pior que botão nenhum."""
    info = next((s for s in await asyncio.to_thread(registry.list) if s.name == name), None)
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    gid = await asyncio.to_thread(_gid_de, name)
    if gid == orq_papeis.GID_PADRAO:
        raise HTTPException(409, detail=erro("erro_orq_sem_grupo",
                                             "esta sessão não está num grupo — pareie as sessões antes"))
    _texto, _mt, papeis = await asyncio.to_thread(_papeis_de, gid)
    if not papeis:
        raise HTTPException(409, detail=erro("erro_orq_sem_papeis",
                                             "defina os papéis do grupo antes de começar"))
    plano = await asyncio.to_thread(plan_progress, info.cwd)
    if plano is None:
        raise HTTPException(409, detail=erro("erro_orq_sem_plano",
                                             "não há plano nesta pasta — a orquestração despacha as "
                                             "Tasks do plano, então escreva o plano primeiro"))
    regras = str(orq_papeis.regras_path(gid))
    texto = (
        "[painel: orquestração] Comece a orquestração deste grupo. Você é o ÁRBITRO.\n"
        f"Invoque a skill `orquestrar` e leia `references/arbitro.md` — só a página do seu papel.\n"
        f"Contrato do grupo: `{regras}` (tabela `## Quem é quem` = quem roda cada papel; "
        "papel com coluna `vez` reveza entre contas, e a Task N cabe à linha (N-1) % total).\n"
        f"Plano: `{plano.path}` — {plano.done} de {plano.total} steps, Task {plano.task_idx} de {plano.task_total}.\n"
        "Comece pelo portão: confira o que já passou, e só então despache a próxima Task."
    )
    res = await _send_thread(_send_one, name, texto)
    if not res["ok"]:
        raise HTTPException(409, detail=erro("erro_orq_comecar_falhou",
                                             f"não deu pra avisar a sessão: {_erro_texto(res['error'])}",
                                             erro=res["error"]))
    return {"ok": True, "entregue": bool(res.get("delivered")), "plano": plano.name}


class RemoverPapelBody(_StrictBody):
    papel: str
    vez: str = ""
    mtime: float


@app.delete("/api/sessions/{name}/orq/papel", dependencies=[Depends(require_auth)])
async def orq_papel_del(name: str, body: RemoverPapelBody):
    """Tira UMA linha da tabela: um papel inteiro (sem `vez`) ou uma conta do rodízio dele. NÃO
    avisa o árbitro — quem mexe na fila normalmente mexe em várias linhas seguidas, e o aviso sai
    uma vez no fim, pelo botão. A sessão viva daquele papel não é tocada: o contrato diz quem
    DEVE rodar, não mata quem está rodando."""
    gid = await asyncio.to_thread(_gid_de, name)
    texto, _mtime, papeis = await asyncio.to_thread(_papeis_de, gid)
    alvo = next((p for p in papeis
                 if orq_md.normalizar(p.papel) == orq_md.normalizar(body.papel)
                 and orq_md.normalizar(p.vez) == orq_md.normalizar(body.vez)), None)
    if alvo is None:
        raise HTTPException(404, detail=erro("erro_orq_papel_inexistente",
                                             f"não há linha para {body.papel!r} nesta configuração"))
    cab = orq_papeis.CABECALHO_VEZ if orq_papeis.tem_coluna_vez(texto) else orq_papeis.CABECALHO
    chave = (alvo.papel, alvo.vez or "-") if cab is orq_papeis.CABECALHO_VEZ else alvo.papel
    texto = orq_md.remover_linha(texto, cab, chave)
    try:
        mtime = await asyncio.to_thread(orq_md.gravar, orq_papeis.regras_path(gid), texto, body.mtime)
    except orq_md.Conflito:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou",
                                             "o contrato mudou desde a leitura — recarregue"))
    return {"papeis": [asdict(p) for p in orq_papeis.ler(texto)], "mtime": mtime}


@app.get("/api/sessions/{name}/pair/contract", dependencies=[Depends(require_auth)])
def pair_contract(name: str):
    """Contrato compartilhado do GRUPO (markdown que os membros editam via fs; keyed pelo gid —
    estável quando membro entra/sai). 404 sem grupo; content vazio se ainda não existe."""
    p = contract_path_for(name)
    if p is None:
        raise HTTPException(404, detail=erro("erro_sessao_nao_pareada", "sessão não está pareada"))
    link = PairLink(name).get() or {}
    try:
        content = p.read_text(encoding="utf-8")
    except OSError:
        content = ""
    return {"peers": link.get("peers", []), "path": str(p), "content": content}


async def _avisar_saida(name: str, expeers: list[str], motivo: str) -> list[dict]:
    """Avisa quem FICOU depois de `name` sair do grupo (o sidecar dele já foi limpo): remoto via
    /unpair-remote do backend dele, local via _deliver. Uma esteira só pra unpair e kill — o kill
    não avisava ninguém e os pares seguiam mandando recado pra um nome morto (ou pra sessão nova
    que reusasse o nome)."""
    errs: list[dict] = []
    for p in expeers:
        if not peers.is_remote(p):
            continue
        if not settings.server_id:
            errs.append({"sessao": p,
                         "erro": erro("erro_pareamento_server_id_ausente",
                                      "CP_SERVER_ID ausente no backend/.env — obrigatório pra "
                                      "pareamento cross-server (é o endereço de resposta srv::sessao)")})
            continue
        srv, sess = peers.split_addr(p)
        try:
            await asyncio.to_thread(peers.call, srv, "POST",
                                    f"/api/sessions/{sess}/unpair-remote",
                                    {"peer": f"{settings.server_id}::{name}"})
        except peers.PeerError as ex:
            # Sidecar remoto fica órfão até alguém desparear lá. ponytail: sem fila de retry — single-user.
            _log.warning("saida do grupo: peer remoto '%s' não avisado (sidecar de lá fica órfão): %s", p, ex)
            errs.append({"sessao": p, "erro": str(ex)})
    resto = [p for p in expeers if not peers.is_remote(p)]
    for p in resto:
        e = await _deliver(p, pair_texto.texto_saida(name, motivo, [x for x in resto if x != p]))
        if e:
            errs.append({"sessao": p, "erro": e})
    return errs


@app.delete("/api/sessions/{name}/pair", dependencies=[Depends(require_auth)])
async def unpair_session(name: str):
    """`name` SAI do grupo (os demais membros continuam entre si; grupo restante de 1 dissolve).
    Avisa quem saiu e quem ficou. Idempotente. Aviso que falhar NÃO refaz o vínculo (fora do grupo
    é o estado desejado) — só reporta no result."""
    expeers = await asyncio.to_thread(pair.leave, name)   # nome próprio: 'peers' é o módulo importado
    if not expeers:
        return {"ok": True, "warning": None}
    errs = await _avisar_saida(name, expeers, "saiu do grupo de trabalho")
    e = await _deliver(name, "[de: hangar] Você saiu do grupo de trabalho "
                             f"({', '.join(expeers)}). Volte a operar independente; use hangar-send só "
                             "quando o usuário pedir.")
    if e:
        errs.append({"sessao": name, "erro": e})
    return {"ok": True, "warning": erro("erro_pareamento_saida_falhou",
            "aviso de saída falhou: " + "; ".join(
                f"{x['sessao']}: {_erro_texto(x['erro'])}" for x in errs),
            avisos=errs)
            if errs else None}


# Quanto esperar o picker sumir da tela depois do Escape, antes de digitar a resposta por texto.
_FECHA_PICKER_TIMEOUT = 3.0


def _espera_picker_fechar(name: str, timeout: float = _FECHA_PICKER_TIMEOUT) -> bool:
    """Espera o overlay (picker/menu) sair do pane depois de um Escape. True se saiu.

    Sem isto, o Escape e a digitacao saiam juntos e o texto era ENGOLIDO pela TUI que ainda estava
    fechando o picker — a resposta do usuario sumia e a bolha ficava presa no fim do chat pra sempre
    (medido em 13/08/2026 numa sessao Kimi: `result=sent` no log e o texto nunca no wire.jsonl).
    O gate normal (`_wait_input_ready`) nao pega este caso no Pi/Kimi: os marcadores de "TUI pronta"
    la sao pedacos de moldura (`─ ╰ │`), e o proprio picker desenha moldura — a primeira leitura ja
    devolve True com o picker ainda em tela.

    Estourou o prazo: devolve False e quem chama envia mesmo assim (nao piora o caso de hoje) —
    mesma politica do _wait_input_ready."""
    from app import tmux                      # import local: mesmo padrao das rotas vizinhas
    from app.state import _FOOTER_RE
    limite = time.monotonic() + timeout
    while time.monotonic() < limite:
        # O PANE INTEIRO, nao o `is_overlay` (que so olha as 8 ultimas linhas): pergunta longa, com
        # muitas opcoes ou tela de Review, empurra o rodape de navegacao pra fora dessa janela e o
        # `is_overlay` responde False com o picker AINDA aberto — furo ja medido noutro consumidor
        # (tests/test_askquestion.py: "is_overlay e falso p/ AskUserQuestion"). Aqui os dois erros
        # custam coisas MUITO diferentes: falso-negativo devolve a corrida que esta funcao existe pra
        # matar; falso-positivo (a frase citada na conversa) so gasta o timeout e envia do mesmo
        # jeito. Entao erra-se pro lado de esperar.
        if not _FOOTER_RE.search(tmux.capture_pane(name)):
            return True
        time.sleep(0.1)
    _log.warning("picker de %s nao fechou em %.1fs apos o Escape; enviando o texto assim mesmo",
                 name, timeout)
    return False


# Prazo pro `tool.result` do picker do Kimi aparecer no wire depois do Submit.
_RESULT_KIMI_TIMEOUT = 5.0


def _espera_resposta_kimi(jsonl: str | None, call_id: str,
                          timeout: float = _RESULT_KIMI_TIMEOUT) -> bool:
    """True quando o `tool.result` daquele toolCallId chega no wire. A escrita nao e instantanea —
    sem a espera, a checagem rodaria antes do Kimi gravar e todo drive bem-sucedido cairia no
    fallback por texto, entregando a resposta DUAS vezes (uma pela ferramenta, outra como msg)."""
    if not jsonl:
        return False
    from app.adapters.kimi.transcript import resposta_chegou
    limite = time.monotonic() + timeout
    while time.monotonic() < limite:
        if resposta_chegou(jsonl, call_id):
            return True
        time.sleep(0.2)
    return False


# Prazo pra escolha no painel de aprovacao do Kimi aterrissar (mesmo criterio do picker).
_APROV_KIMI_TIMEOUT = 5.0


def _espera_escolha_kimi(name: str, jsonl: str, req_id: str, pede_feedback: bool,
                         timeout: float = _APROV_KIMI_TIMEOUT) -> bool:
    """True quando a escolha no painel de aprovacao do Kimi esta comprovadamente entregue.

    Duas provas, porque as escolhas do painel terminam de dois jeitos diferentes: a comum vira
    `interaction.resolved` no wire, e a que pede justificativa (`Revise`, `Reject with feedback`)
    NAO resolve nada na hora — o painel troca o rodape por um campo de texto e espera a pessoa
    escrever. Sem a segunda prova, escolher `Revise` gastaria o prazo inteiro e voltaria erro numa
    tecla que pegou."""
    from app.adapters.kimi.transcript import interacao_resolvida
    limite = time.monotonic() + timeout
    while time.monotonic() < limite:
        if interacao_resolvida(jsonl, req_id):
            return True
        if pede_feedback and terminal_input.feedback_kimi_aberto(name):
            return True
        time.sleep(0.2)
    return False


def _select_aprovacao_kimi(name: str, info, option: int) -> dict:
    """Escolhe no painel de APROVACAO do Kimi (plano/comando/arquivo).

    As opcoes vem do WIRE (`read_pending_interaction`), nao do pane — e a mesma fonte que o estado
    usou pra desenhar os botoes, entao o numero que chega aqui casa com o que a pessoa leu.

    Sem pedido pendente, 409 — NUNCA cair no `terminal.select` generico. Ele conta a linha do cursor
    e, quando nao acha (`_cursor_row` so le `❯`, e o Kimi desenha `▶`), manda Down x(n-1) + Enter as
    CEGAS. Numa sessao Kimi isso nao tem alvo: ou o painel ja fechou e as teclas caem na conversa em
    execucao, ou ele esta aberto e o Enter confirma a linha errada — nos dois casos a rota devolveria
    {"ok": true}, que e o sucesso falso que este projeto proibe. E nao ha o que perder: `_menu_block`
    exige cursor `❯`/`>`, entao o pane do Kimi nunca produziu opcao por raspagem — este endpoint so
    e alcancavel, nesse provider, pelos botoes que o wire desenhou."""
    from app.adapters.kimi.transcript import read_pending_interaction
    jsonl = info.jsonl if info else None
    pend = read_pending_interaction(jsonl) if jsonl else None
    if pend is None:
        # Cobre "ja foi respondida no terminal" e "nao deu pra ler o wire agora" — pro usuario a
        # saida e a mesma (nada foi enviado, olhe a sessao). O caso ilegivel nao some calado: o
        # `_objetos_da_cauda` loga uma vez por arquivo.
        raise HTTPException(409, detail=erro(
            "erro_sem_pergunta_kimi",
            "nenhuma pergunta do Kimi pendente (ja respondida no terminal?)"))
    escolhas = pend["escolhas"]
    if not 1 <= option <= len(escolhas):
        raise HTTPException(409, detail=erro(
            "erro_opcao_fora_da_lista",
            f"opção {option} não existe neste pedido (são {len(escolhas)}) — opção NÃO enviada"))
    try:
        terminal_input.select_kimi(name, option, jsonl, pend["id"])
    except ValueError as e:
        raise HTTPException(409, str(e))
    except terminal_input.DriveError as e:
        # O wire dizia pendente e o painel ja saiu da tela: respondido no terminal entre o toque e
        # aqui. Mesmo caso (e mesma frase) do picker do Kimi no /answer.
        diag.registrar("aprovacao_kimi.painel_fechado", "erro", sessao=name, detalhe=str(e))
        raise HTTPException(409, detail=erro(
            "erro_sem_pergunta_kimi",
            "nenhuma pergunta do Kimi pendente (ja respondida no terminal?)"))
    if not _espera_escolha_kimi(name, jsonl, pend["id"], escolhas[option - 1]["requires_feedback"]):
        # Prazo estourado NAO prova que nada pegou — pode ser o Kimi demorando pra gravar. Igual ao
        # /answer do picker: so se o painel CONTINUA na tela e que a tecla comprovadamente nao pegou.
        if terminal_input.aprovacao_kimi_aberta(name):
            raise HTTPException(409, detail=erro(
                "erro_opcao_nao_convergiu", "não consegui marcar essa opção no terminal — tente de novo",
                detalhe="o painel de aprovação continua aberto e nada foi resolvido no wire"))
        raise HTTPException(409, detail=erro(
            "erro_sem_confirmacao_resposta",
            "resposta enviada, mas nao deu pra confirmar a tempo — "
            "confira na sessao antes de responder de novo"))
    return {"ok": True, "feedback_pendente": escolhas[option - 1]["requires_feedback"]}


def _recusa_se_so_enfileirou(name: str, res: dict) -> None:
    """Plano B do /answer: o texto foi ACEITO pela fila mas NAO digitado (o gate recusou, porque o
    picker segue aberto). Ate 01/09/2026 os tres provedores devolviam ok=true aqui e o app pintava a
    bolha como enviada — a pessoa esperava por uma resposta que nunca sairia da fila, ja que a fila
    so drena quando a sessao deixa de aguardar e quem a segurava era a propria pergunta.

    Levanta 409 e NAO limpa o sidecar do hook: a pergunta continua aberta, e apaga-lo devolveria a
    sessao pra `idle` na lista (ver askquestion.pergunta_aberta)."""
    if res.get("delivered"):
        return
    _log.warning("resposta name=%s: texto do plano B ficou na fila, nao digitado", name)
    raise HTTPException(409, detail=erro(
        "erro_resposta_nao_entregue",
        "nao consegui responder por aqui — a pergunta segue aberta, responda no terminal"))


def _recusa_se_painel_aberto(name: str) -> None:
    # Com o painel anexado, a janela do tmux esta no tamanho DELE (~120x20). Quem conta linha no
    # pane — o seletor de opcao, o stepper do AskUserQuestion (terminal_input.answer_questions /
    # answer_question_pi) e o model_picker (lista e troca de modelo, que dirige o /model contando
    # linhas do pane) — leria um pane truncado e escolheria errado.
    #
    # O termsock NAO importa `pty` no topo justamente pra este import funcionar no Windows.
    from app import termsock
    if name in termsock.clientes_ativos():
        raise HTTPException(status_code=409,
                            detail=erro("erro_terminal_aberto",
                                        "Terminal aberto nesta sessao. Feche o painel pra responder "
                                        "por aqui."))


@app.post("/api/sessions/{name}/select", dependencies=[Depends(require_auth)])
def select(name: str, body: SelectBody):
    # Mesma guarda do /input — e aqui ela é a ÚNICA: a cadeia abaixo não sabe falhar. terminal.select
    # devolve None, send_keys descarta o returncode e tmux._run converte tmux morto/travado
    # (TimeoutExpired/OSError) num CompletedProcess(returncode=1) que ninguém lê. Sem isto, responder
    # uma opção de sessão morta digitava no vazio e a resposta era {"ok": true} — o catch do card
    # nunca disparava. (O fix de raiz em send_keys/_run é outro diff: interrupt/model_picker/
    # TerminalMirror também passam por lá.)
    _recusa_se_painel_aberto(name)
    if not _session_exists(name):
        raise HTTPException(404, detail=erro("erro_sessao_opcao_nao_enviada", "sessão não encontrada — opção NÃO enviada"))
    # Kimi: os botoes de aprovacao (plano/comando/arquivo) sao desenhados a partir do WIRE, entao a
    # escolha volta pelo wire tambem — tecla numerica + `interaction.resolved` como prova. O drive
    # generico abaixo NAO atende este provider em hipotese nenhuma (ver _select_aprovacao_kimi).
    info = _cached_info_sync(name)
    if getattr(info, "provider", "claude") == "kimi":
        return _select_aprovacao_kimi(name, info, body.option)
    try:
        terminal.select(name, body.option)
    except terminal_input.DriveError as e:
        # Cursor do picker nao convergiu pra opcao pedida: nada foi enviado (o Enter fica de fora).
        # 409 com texto na tela em vez de 500 calado — sem isso o toque some sem nenhum sinal.
        diag.registrar("opcao.nao_convergiu", "erro", sessao=name, detalhe=str(e))
        raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu", "não consegui marcar essa opção no terminal — tente de novo", detalhe=str(e)))
    return {"ok": True}


@app.post("/api/sessions/{name}/select/submit", dependencies=[Depends(require_auth)])
def select_submit(name: str):
    """Envia as opções JÁ MARCADAS de um picker de múltipla escolha.

    Existe porque marcar e enviar são coisas diferentes ali: pelo celular dava pra marcar e não
    dava pra enviar — a lista crua só oferecia Cancelar. Ver `terminal_input.submeter_multipla`
    pro caminho na TUI (aba Submit) e pro porquê de o Enter sozinho não servir.
    """
    _recusa_se_painel_aberto(name)
    if not _session_exists(name):
        raise HTTPException(404, detail=erro("erro_sessao_opcao_nao_enviada", "sessão não encontrada — opção NÃO enviada"))
    try:
        terminal.submeter_multipla(name)
    except terminal_input.DriveError as e:
        # Mesma política do /select: 409 com o motivo na tela, nunca 500 calado nem "ok" mentiroso.
        diag.registrar("opcao.envio_falhou", "erro", sessao=name, detalhe=str(e))
        raise HTTPException(409, detail=erro("erro_opcao_nao_convergiu", "não consegui enviar as opções marcadas — tente de novo", detalhe=str(e)))
    return {"ok": True}


@app.post("/api/sessions/{name}/interrupt", dependencies=[Depends(require_auth)])
async def interrupt(name: str, clear: bool = False):
    # Codex: interrompe a propria TUI pelo tmux, mantendo celular e terminal no mesmo controlador.
    if _provider_of(name) == "codex":
        await get_adapter("codex").interrupt(name)
        return {"ok": True}
    # clear=True: alem de interromper, limpa o input (2o Esc). So o front com msg pendente passa isso —
    # garante input nao-vazio, evitando que o Esc-Esc abra o menu de rewind num input ja vazio.
    # terminal.interrupt e SYNC (tmux) -> threadpool pra nao bloquear o event loop (handler async agora).
    await asyncio.to_thread(terminal.interrupt, name, clear=clear)
    return {"ok": True}


def _normalize_rate_window(window: dict | None) -> dict | None:
    # RateLimitWindow (app-server) -> shape neutro do front: usedPercent/windowMins/resetsAt.
    # window None (secondary/credits costumam vir null) -> None, o front so mostra o que existe.
    if window is None:
        return None
    return {
        "usedPercent": window.get("usedPercent"),
        "windowMins": window.get("windowDurationMins"),
        "resetsAt": window.get("resetsAt"),
    }


@app.get("/api/sessions/{name}/limits", dependencies=[Depends(require_auth)])
async def limits(name: str):
    # So Codex tem rate limits expostos pelo app-server (account/rateLimits/read) -- Claude tem o
    # proprio chip de rate-limit (status_line), fora do escopo aqui (regra de ouro: Claude intocado).
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_limits_so_codex", "limits so existe pra sessoes Codex"))
    snapshot = await get_adapter("codex").read_rate_limits(name)
    if snapshot is None:
        # app-server indisponivel/recusou -- resposta neutra (sem erro), o front so nao mostra nada.
        return {"primary": None, "secondary": None, "planType": None}
    return {
        "primary": _normalize_rate_window(snapshot.get("primary")),
        "secondary": _normalize_rate_window(snapshot.get("secondary")),
        "planType": snapshot.get("planType"),
    }


class CodexModelBody(_StrictBody):
    model: str
    effort: str | None = None


@app.get("/api/sessions/{name}/models", dependencies=[Depends(require_auth)])
async def modelos_da_sessao_codex(name: str):
    # Task C: modelo + reasoning effort so pra Codex (via model/list) -- o /model do Claude e o
    # picker interativo dedicado (/model-effort), sem esta rota.
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_models_so_codex", "models so existe pra sessoes Codex"))
    adapter = get_adapter("codex")
    return {"models": await adapter.list_models(name), "current": adapter.current_model(name)}


@app.post("/api/sessions/{name}/model", dependencies=[Depends(require_auth)])
async def set_codex_model(name: str, body: CodexModelBody):
    # Grava a escolha e reabre/configura a TUI; se ha turno em voo, aplica ao terminar.
    if _provider_of(name) != "codex":
        raise HTTPException(400, detail=erro("erro_model_so_codex", "model so existe pra sessoes Codex"))
    await get_adapter("codex").set_model(name, body.model, body.effort)
    return {"ok": True}


@app.get("/api/sessions/{name}/pane", dependencies=[Depends(require_auth)])
def pane(name: str, lines: int = 200):
    # Pane CRU (texto ja composto pelo tmux: sem ANSI/cursor-move). O espelho do pane (TerminalMirror)
    # le isto pra mostrar overlays so-TUI (/status, /config, /help, pickers) que nao caem no .jsonl.
    # `lines` = quanto SCROLLBACK trazer acima da tela visivel (capture-pane -S). O espelho pede mais
    # quando o usuario rola pro topo; clampeado pra uma janela absurda nao virar payload gigante a
    # cada poll de 450ms.
    from app import tmux
    if not tmux.has_session(name):
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    # `scrollback` diz se pedir mais linhas ADIANTA. Num TUI de tela alternada (Claude Code) vale 0:
    # o tmux nao guarda historico ali, e quem quer subir tem que rolar o PROPRIO TUI (PageUp), nao o
    # tmux. Sem esse dado a UI ofereceria "carregar mais historico" que nunca traria nada.
    return {"text": tmux.capture_pane(name, lines=max(50, min(lines, 5000))),
            "scrollback": tmux.pane_scrollback(name)}


@app.post("/api/sessions/{name}/keys", dependencies=[Depends(require_auth)])
def keys(name: str, body: KeyBody):
    # Uma tecla de navegacao (allowlist) pro pane — dirige overlays so-TUI a partir do espelho.
    try:
        terminal.send_key(name, body.key)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


@app.post("/api/sessions/{name}/term-input", dependencies=[Depends(require_auth)])
def term_input(name: str, body: TermInputBody):
    # Terminal interativo (so desktop): manda texto digitado (literal) e/ou uma tecla nomeada pro pane.
    try:
        if body.text:
            terminal.send_text(name, body.text)
        if body.key:
            terminal.send_term_key(name, body.key)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


def _painel_disponivel() -> bool:
    # O termsock NAO importa `pty` no topo justamente pra este import funcionar no Windows.
    from app import termsock
    return termsock.painel_disponivel()


# ─── Atualizar ─────────────────────────────────────────────────────────────────────────────────

def _mudancas_pendentes() -> list[dict]:
    """Os commits que entraram em `origin/main` e ainda não estão aqui — o changelog da tela.

    Título de commit, e não um `CHANGELOG.md` mantido à mão: as mensagens deste repo já são
    descritivas, e um arquivo à parte seria uma segunda cópia pra envelhecer. Passo que merecer
    texto próprio ganha um arquivo em `docs/atualizacoes/`, cujo corpo entra junto.
    """
    p = atualizar._git("log", "--format=%h%x00%s", "HEAD..origin/main", timeout=30)
    if p.returncode != 0:
        return []
    linhas = []
    for linha in p.stdout.splitlines():
        sha, _, titulo = linha.partition("\x00")
        if titulo:
            linhas.append({"sha": sha, "titulo": titulo})
    return linhas


@app.get("/api/atualizacao", dependencies=[Depends(require_auth)])
async def get_atualizacao(procurar: bool = False):
    """Estado da atualização: as três versões, o que há de novo, e o que o motor está fazendo.

    As TRÊS versões porque "a versão instalada" tem três respostas — o disco, o processo vivo e o
    bundle que o navegador carregou — e elas divergem justamente na janela em que alguém está
    atualizando. Comparar só o disco com `origin/main` diria "tudo em dia" pra quem está olhando
    uma tela de dias atrás.

    Tudo em `to_thread`: são chamadas de `git` (subprocess), e o precedente de 23/07 é que elas
    nunca podem rodar na corrotina.
    """
    def _ler() -> dict:
        # `procurar=1` vai à REDE antes de comparar. Sem isto, o botão "Procurar de novo" só relia
        # o `origin/main` que já estava no disco — a foto do último fetch do laço, que roda a cada
        # 30min — e respondia "Tudo em dia" com informação velha, afirmando ter procurado. Não é
        # o padrão porque o polling da tela bate aqui a cada 2s durante uma atualização, e um
        # `git fetch` nessa cadência é rede à toa.
        if procurar:
            atualizar._git("fetch", "origin", timeout=120)
        pre = atualizar.checar()
        mudancas = _mudancas_pendentes()
        return {
            "versoes": {"repo": diag._git_describe(), "backend": diag.VERSAO_EM_EXECUCAO},
            "atualizacao_disponivel": bool(mudancas),
            "mudancas": mudancas,
            "passos": [{"id": s["id"], "titulo": s["titulo"], "texto": s["texto"]}
                       for s in atualizacoes.pendentes()],
            "pre_voo": pre,
            # `estado_para_tela`, não `estado`: converte "rodando" com o processo morto na falha
            # que ele não conseguiu escrever. Sem isso a tela fica presa numa atualização que já
            # não existe, e só sai editando o JSON na mão.
            "estado": atualizar.estado_para_tela(),
        }
    return await asyncio.to_thread(_ler)


@app.post("/api/atualizacao/iniciar", dependencies=[Depends(require_auth)])
async def post_atualizacao_iniciar():
    """Lança a atualização e devolve na hora — ela roda FORA deste processo, que vai reiniciar."""
    pre = await asyncio.to_thread(atualizar.checar)
    # Recusa ANTES de lançar o motor: a atualização alinha o disco com `origin/main` e arrastaria a
    # branch de trabalho junto (medido em 25/08/2026 numa máquina com `mobile-expo` no checkout).
    if pre.get("branch_de_trabalho"):
        raise HTTPException(409, detail=erro(
            "erro_atualizacao_branch",
            f"este checkout esta na branch {pre.get('branch')}, nao na main",
            branch=pre.get("branch")))
    if not pre.get("pode"):
        faltando = pre.get("faltando") or []
        raise HTTPException(409, detail=erro(
            "erro_atualizacao_dependencia", f"falta o que a atualizacao precisa: {', '.join(faltando)}",
            faltando=faltando))
    r = await asyncio.to_thread(atualizar.iniciar, settings.port)
    if not r.get("ok"):
        raise HTTPException(409, detail=erro("erro_atualizacao_ja_rodando",
                                             "ja existe uma atualizacao rodando"))
    return r


@app.post("/api/atualizacao/reiniciar", dependencies=[Depends(require_auth)])
async def post_atualizacao_reiniciar():
    """Reinicia o servidor sem atualizar nada — o caso do disco já estar à frente do processo."""
    r = await asyncio.to_thread(atualizar.reiniciar_agora)
    if not r.get("ok"):
        raise HTTPException(409, detail=erro(
            "erro_reinicio_indisponivel",
            f"esta maquina nao reinicia sozinha (topologia {r.get('topologia')})",
            topologia=r.get("topologia")))
    return r


async def _fetch_loop():
    """`git fetch` de tempos em tempos, senão `origin/main` é a foto do último pull de alguém.

    Sem isto o botão simplesmente nunca apareceria numa máquina que ninguém puxa à mão. Fail-soft
    e em `to_thread`: máquina sem rede não pode virar erro na tela nem derrubar o laço.
    """
    while True:
        try:
            await asyncio.to_thread(atualizar._git, "fetch", "origin", timeout=120)
        except Exception:                            # noqa: BLE001 — sem rede é o caso comum
            _log.debug("fetch periodico falhou", exc_info=True)
        await asyncio.sleep(1800)


# ─── Auto-update ────────────────────────────────────────────────────────────────────────────────
# O botão pede um clique; a correção de bug não pode esperar o clique em cada máquina. Este laço
# checa a cada hora e dispara o MESMO motor do botão (`atualizar.iniciar`), sem ninguém tocar em
# nada. Os gates vivem em `_auto_update_motivo`: qualquer um deles recusando, o tick vira um log e
# a próxima hora tenta de novo.
_AUTO_UPDATE_INTERVALO = 3600
_AUTO_UPDATE_FALHA_JANELA_S = 86400   # uma atualização que falhou segura novas tentativas por 24h
_DIST_SHA_URL = "https://github.com/jeffer1312/hangar/releases/download/dist-latest/frontend-dist.sha"


def _auto_update_motivo() -> Optional[str]:
    """Por que o auto-update NÃO deve disparar neste tick. None = dispara.

    Diferenças pro botão, de propósito: árvore suja ou commits locais adiante BLOQUEIAM aqui (o
    botão resguarda e pergunta; o automático não pode decidir sobre trabalho de ninguém), e sem o
    dist do CI deste commit exato NÃO cai no build local — espera o próximo tick (sha publicado =
    CI verde + build pronto, que é condição, não aceleração). Sobrava uma corrida de segundos — push entre o gate e o fetch do motor, ou release `dist-latest` móvel —
    em que o build local de fallback podia ainda acontecer: consequencia e lentidao, nao tela errada, entao ficou aceita e registrada aqui.
    """
    pre = atualizar.checar()
    if not pre.get("pode"):
        return "dependencias faltando"
    if pre.get("branch_de_trabalho"):
        return f"checkout na branch {pre.get('branch')}"
    # divergiu ANTES de ahead: divergiu = ahead>0 AND behind>0, e o motivo mais preciso e o dela.
    if pre.get("divergiu"):
        return "checkout divergiu de origin/main"
    if pre.get("ahead"):
        return "checkout adiante de origin/main (commits locais nao pushados)"
    if not pre.get("behind"):
        return "em dia"
    if pre.get("sujo"):
        return "arvore suja (trabalho nao commitado)"
    # estado_para_tela, NAO estado: o cru congela em "rodando" se o motor morrer sem gravar o
    # desfecho (kill, queda de energia), e cada tick devolveria "ja rodando" pra sempre — o auto-update
    # morria em silencio ate alguem abrir a tela. A conversao de dono-morto ja existe aqui.
    est = atualizar.estado_para_tela()
    if est.get("fase") == "rodando":
        return "atualizacao ja rodando"
    if est.get("ok") is False:
        try:
            idade = (datetime.now().astimezone() - datetime.fromisoformat(est.get("ts"))).total_seconds()
        except (TypeError, ValueError):
            # ts corrompido: abre, mas com log — senao a maquina re-tenta a cada hora com zero linha de log
            # explicando por que a janela de repeticao nao segurou.
            _log.warning("auto-update: ts invalido no estado da atualizacao (%r)", est.get("ts"))
            idade = _AUTO_UPDATE_FALHA_JANELA_S
        if idade < _AUTO_UPDATE_FALHA_JANELA_S:
            return "ultima atualizacao falhou"
    try:
        with urllib.request.urlopen(_DIST_SHA_URL, timeout=15) as r:
            sha_dist = r.read().decode().strip()
    except Exception:                                  # noqa: BLE001 — qualquer falha aqui = dist ilegivel
        return "sem acesso ao dist do CI"
    p = atualizar._git("rev-parse", "origin/main", timeout=30)
    sha_alvo = p.stdout.strip()
    if p.returncode != 0 or not sha_alvo:
        return "rev-parse origin/main falhou"
    if sha_dist != sha_alvo:
        return "dist do CI ainda nao e deste commit"
    return None



async def _auto_update_loop():
    """Checa a cada hora e dispara a atualização quando TODOS os gates abrem. Fail-soft inteiro:
    qualquer exceção vira log e o próximo tick, nunca derruba o laço nem o backend."""
    await asyncio.sleep(_AUTO_UPDATE_INTERVALO)   # nunca na subida: o boot já é o ponto de ruído
    while True:
        try:
            if automations_enabled():
                # rc checado, NAO so o lance de excecao: fetch falho por rc (rede fora, auth quebrada) nao lanca nada — a origin/main fica velha, o gate reporta "em dia" e a máquina NUNCA se atualiza com zero log. rc checado, loga warning com a cauda: "sem rede" deixa de parecer "sem novidade".
                p = await asyncio.to_thread(atualizar._git, "fetch", "origin", timeout=120)
                if p.returncode != 0:
                    _log.warning("auto-update: fetch falhou (rc=%s): %s", p.returncode, atualizar._cauda(p))
                motivo = await asyncio.to_thread(_auto_update_motivo)
                if motivo is None:
                    # Sessão trabalhando é gate async (classify captura os panes), não cabe no helper.
                    infos = await registry.list_with_state()
                    if any(i.state == "working" for i in infos):
                        motivo = "sessao trabalhando"
                if motivo is None:
                    _log.info("auto-update: disparando atualizacao")
                    r = await asyncio.to_thread(atualizar.iniciar, settings.port)
                    if not r.get("ok"):
                        _log.warning("auto-update: iniciar recusou (%s)", r.get("erro"))
                elif motivo != "em dia":
                    _log.info("auto-update: pulando (%s)", motivo)
        except Exception:                            # noqa: BLE001 — sem rede/sem tmux é comum
            _log.exception("auto-update: tick falhou")
        await asyncio.sleep(_AUTO_UPDATE_INTERVALO)


@app.get("/api/config", dependencies=[Depends(require_auth)])
def get_config():
    """Config editavel pelo app + o que e so-leitura (exige reiniciar o servico).

    Segredo NUNCA volta inteiro: `estado()` devolve mascarado (gsk_••••1234) — da pra conferir QUAL
    chave esta la sem conseguir copia-la de volta."""
    return {
        "campos": runtime_config.estado(),
        "somente_leitura": {
            "port": settings.port,
            "lan_bind_ip": settings.lan_bind_ip,
            "server_id": settings.server_id,
            "public_url": settings.public_url,
            # CAPACIDADE, nao nome de sistema: "da pra abrir o painel aqui?". O
            # `os.name == "posix"` que estava aqui respondia outra pergunta — e a diferenca deixou
            # de ser teorica em 22/08/2026, quando o Windows ganhou motor (ConPTY): a resposta la
            # virou True sem ninguem tocar nesta linha, que e o ponto de perguntar por capacidade.
            # Ela tambem responde False num POSIX sem `pty`. Import tardio pelo mesmo motivo de
            # sempre: o termsock nao pode ser importado no topo deste modulo.
            "terminal_panel": _painel_disponivel(),
            # A versao do PROCESSO VIVO, nao a do checkout. Durante a janela entre o `git pull` e o
            # restart as duas divergem, e e exatamente ai que o botao Atualizar vive: dizer a do
            # disco aqui seria afirmar estar rodando codigo que ninguem carregou (o defeito que
            # `diag.VERSAO_EM_EXECUCAO` corrigiu em f4013343).
            "versao": diag.VERSAO_EM_EXECUCAO,
        },
    }


# POST **e** PATCH: o PATCH morria em erro de CORS cross-origin e a culpa foi posta no "proxy na
# frente do backend". ERRADO — o culpado era o plugin apiCorsPreflight do frontend/vite.config.ts,
# que respondia TODO preflight de /api com uma LISTA FIXA de metodos que nao tinha PATCH (nem PUT,
# o que so apareceu em 2026-07-31, derrubando salvar motor). Hoje o plugin ECOA o metodo pedido e
# nao ha mais lista pra envelhecer. O cliente segue no POST porque funciona; o PATCH vale pra quem
# chamar a API na mao.
@app.post("/api/config", dependencies=[Depends(require_auth)])
@app.patch("/api/config", dependencies=[Depends(require_auth)])
async def patch_config(request: Request):
    """Grava overrides. Campo desconhecido e ignorado (o cliente nao inventa setting); tipo errado
    volta 400 com a mensagem, em vez de gravar lixo que so quebraria depois."""
    body = await request.json()
    if not isinstance(body, dict):
        raise HTTPException(400, detail=erro("erro_corpo_deve_ser_objeto", "corpo deve ser um objeto"))
    try:
        await asyncio.to_thread(runtime_config.aplicar, body)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"campos": runtime_config.estado()}


def _motores_para_cliente() -> dict[str, dict]:
    """Motores com a api_key MASCARADA: dá para conferir QUAL chave está lá sem copiá-la de volta
    (mesma regra do groq_api_key no runtime_config)."""
    out = {}
    for nome, e in engines.listar().items():
        visivel = dict(e)
        chave = visivel.pop("api_key", "")
        visivel["api_key"] = runtime_config.mascarar(chave)
        visivel["api_key_definida"] = bool(chave)
        out[nome] = visivel
    return out


@app.get("/api/providers", dependencies=[Depends(require_auth)])
async def get_providers():
    return await asyncio.to_thread(cli_probe.sondar_providers)


@app.get("/api/engines", dependencies=[Depends(require_auth)])
def get_engines():
    # arquivo_corrompido: distingue "ninguém configurou motor" de "engines.json existe mas não
    # pôde ser lido" — as duas batem em {} no listar() de propósito (não pode derrubar sessão nem
    # o tick do SSE por um hand-edit ruim), mas a tela precisa saber a diferença (item 1 do
    # review): sem isto o usuário vê "nenhum motor ainda" com um arquivo quebrado escondendo
    # motores reais, re-adiciona um, e a próxima gravação apaga os outros.
    return {
        "motores": _motores_para_cliente(),
        "arquivo_corrompido": engines.arquivo_corrompido(),
        "arquivo_caminho": str(engines.caminho()),
    }


@app.put("/api/engines/{nome}", dependencies=[Depends(require_auth)])
async def put_engine(nome: str, request: Request):
    """Cria/atualiza um motor.

    api_key ausente, vazia, ou IGUAL à máscara que o cliente recebeu = preserva a atual. Sem isso,
    salvar o formulário só para trocar o modelo apagava a chave, sem volta — o bug pago em 22ae599.

    Campo AUSENTE do corpo herda o valor do disco; campo presente vale, inclusive `""`/`0`, que é
    como se LIMPA (_normalizar descarta vazio, então o campo sai do registro). engines.salvar()
    substitui o registro inteiro, e sem a herança um cliente que só conhece parte do schema — um PUT
    de script, uma versão antiga do front — apagava o resto calado. Medido: o probe de modelos
    devolve `context_length: null` para provedor que não informa (opencode), o PUT seguinte vinha sem
    `context_window` e o motor perdia a janela de 1M, voltando a compactar em 200k sem avisar.

    `null` conta como AUSENTE de propósito (é o que o probe manda quando não sabe). Quem quer limpar
    manda `""` — a tela de Motores faz isso nos campos opcionais."""
    body = await request.json()
    if not isinstance(body, dict):
        raise HTTPException(400, detail=erro("erro_corpo_deve_ser_objeto", "corpo deve ser um objeto"))
    # I/O de disco no threadpool, igual ao resto deste handler (ver comentário acima de create_session).
    atual = (await asyncio.to_thread(engines.listar)).get(nome, {})
    chave_atual = atual.get("api_key", "")
    enviada = body.get("api_key")
    if chave_atual and (
        not isinstance(enviada, str)
        or not enviada.strip()
        or enviada.strip() == runtime_config.mascarar(chave_atual)
    ):
        body["api_key"] = chave_atual
    # `campo not in body` (não `body.get(campo) is None`): a segunda forma tratava `""` como ausente
    # e reinjetava o valor do disco, então LIMPAR um campo opcional na tela virava no-op com HTTP 200
    # — o usuário escolhia "mesmo que o principal" em subagent_model, salvava, e o modelo antigo
    # voltava sem aviso. `null` segue herdando (é o que o probe manda quando não sabe o valor).
    for campo, valor_atual in atual.items():
        if campo not in body or body[campo] is None:
            body[campo] = valor_atual
    try:
        await asyncio.to_thread(engines.salvar, nome, body)
    except ValueError as e:
        raise HTTPException(400, str(e))
    # A chave do cache é o NOME do motor: trocar base_url ou api_key mantendo o nome serviria a
    # lista do provedor ANTIGO por até 5 minutos.
    _engine_models_cache.pop(nome, None)
    return {"motores": await asyncio.to_thread(_motores_para_cliente)}


@app.delete("/api/engines/{nome}", dependencies=[Depends(require_auth)])
async def delete_engine(nome: str):
    try:
        if not await asyncio.to_thread(engines.remover, nome):
            raise HTTPException(404, detail=erro("erro_motor_nao_encontrado", "motor nao encontrado"))
    except ValueError as e:
        # engines.json corrompido: remover() recusa escrever por cima (item 1 do review) em vez de
        # apagar os outros motores. Vira 400 com a mensagem em vez de 500 cru.
        raise HTTPException(400, str(e))
    # Mesma invalidação do PUT: a chave do cache é o NOME do motor.
    _engine_models_cache.pop(nome, None)
    return {"ok": True}


class EngineProbeBody(_StrictBody):
    # `nome` de um motor já salvo (reusa a key do disco, que o cliente não tem inteira) OU
    # base_url+api_key de um motor sendo criado. Os dois modos são MUTUAMENTE EXCLUSIVOS — ver
    # o guard em engine_modelos: misturar `nome` com um `base_url` do cliente mandaria a api_key
    # REAL do motor salvo, no header Authorization, para qualquer host que o cliente escolher.
    nome: str | None = None
    base_url: str | None = None
    api_key: str | None = None


@app.post("/api/engines/modelos", dependencies=[Depends(require_auth)])
async def engine_modelos(body: EngineProbeBody):
    """Modelos que a key pode usar, direto do provedor. É também o 'Testar' da tela: 200 = key boa,
    502 = a mensagem do provedor (401, host errado, endpoint ausente).

    `nome` e `base_url`/`api_key` não se combinam: com `nome`, SÓ o base_url e a key salvos valem
    — um base_url do cliente junto seria exfiltração da key real para host arbitrário, não SSRF
    comum (o app já aceita SSRF cego por trás do token; isto seria mais forte, key sai de propósito).
    Recusa em vez de ignorar em silêncio: um ignore silencioso deixaria o cliente achando que testou
    o host que mandou."""
    if body.nome:
        if body.base_url or body.api_key:
            raise HTTPException(400, detail=erro("erro_motor_nome_com_dados", "nome já usa o motor salvo; não envie base_url/api_key junto"))
        # I/O de disco no threadpool, igual ao resto deste handler (ver comentário acima de create_session).
        salvo = (await asyncio.to_thread(engines.listar)).get(body.nome)
        if not salvo:
            raise HTTPException(404, detail=erro("erro_motor_nao_encontrado", "motor nao encontrado"))
        base_url, api_key = salvo["base_url"], salvo["api_key"]
    else:
        base_url, api_key = body.base_url, body.api_key
        if not base_url or not api_key:
            raise HTTPException(400, detail=erro("erro_motor_nome_ou_dados", "informe nome de um motor salvo, ou base_url + api_key"))
    try:
        # Mesma guarda do salvar: a key vai no header, http para host público a expõe na rede.
        base_url = engines.validar_base_url(base_url)
    except ValueError as e:
        raise HTTPException(400, str(e))
    try:
        modelos = await asyncio.to_thread(engine_probe.listar_modelos, base_url, api_key)
    except RuntimeError as e:
        raise HTTPException(502, str(e))
    except ValueError as e:
        # \r/\n na key ou no base_url (item 3 do review): engine_probe recusa ANTES de montar o
        # Request — sem isto o urllib levantaria com a key crua na mensagem, e a rota abaixo relança
        # RuntimeError pro uvicorn logar (traceback com a key no journal). 400 sem ecoar o valor.
        raise HTTPException(400, str(e))
    return {"modelos": modelos}


def _id_upload(info: SessionInfo) -> str:
    """Id durável da sessão, que é como a pasta de anexos é chaveada — nome muda no rename, id não.
    Sessão sem transcript ainda cai no nome: janela curta, e um anexo mandado nela fica para trás
    se ela for renomeada depois."""
    return session_key(info.jsonl) if info.jsonl else info.name


@app.post("/api/sessions/{name}/upload", dependencies=[Depends(require_auth)])
async def upload(name: str, request: Request):
    # Resolve o cwd da sessao (registry.list() ja traz cwd via tmux #{pane_current_path}).
    # handler async -> registry.list() (subprocess tmux) no threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if not info.cwd:
        raise HTTPException(409, detail=erro("erro_cwd_indisponivel", "cwd da sessao indisponivel"))
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 100 * 1024 * 1024:
        raise HTTPException(413, detail=erro("erro_arquivo_grande", "arquivo maior que 100 MiB"))
    data = await request.body()
    # Filename do cliente (X-Filename, percent-encoded) ou ?name= -> so a EXTENSAO e usada
    # (o nome final e gerado pelo servidor). Qualquer tipo de arquivo.
    filename = request.headers.get("x-filename") or request.query_params.get("name")
    try:
        # write_bytes (ate 100 MiB) no threadpool pra nao bloquear o loop durante o disco.
        path = await asyncio.to_thread(save_upload, info.cwd, _id_upload(info), data, filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)

    # Higiene: varre anexos velhos DESTA sessao. Barato (um listdir) e sem agendador pra manter.
    # Falhar aqui nao pode custar o upload que acabou de dar certo.
    try:
        await asyncio.to_thread(prune_old, info.cwd, runtime_config.get("upload_retention_days"))
    except Exception:
        _log.exception("prune de uploads falhou (upload seguiu)")

    # Video: o Read nao abre mp4, entao o anexo virava um caminho morto pro modelo. Extrai quadros
    # ao longo da duracao + transcreve o audio -> vira coisa legivel. Best-effort: sem ffmpeg/sem
    # audio/sem chave da Groq, devolve o que conseguiu e o upload segue igual.
    frames: list[str] = []
    fala = ""
    if is_video(path):
        try:
            frames = await asyncio.to_thread(extract_frames, path)
        except Exception:
            _log.exception("extracao de quadros falhou (upload seguiu)")
        try:
            audio = await asyncio.to_thread(extract_audio, path)
            if audio:
                bytes_audio = await asyncio.to_thread(Path(audio).read_bytes)
                fala = await asyncio.to_thread(transcribe, bytes_audio, "audio.m4a")
        except TranscribeError as e:
            _log.info("video sem transcricao: %s", e.detail)
        except Exception:
            _log.exception("transcricao do video falhou (upload seguiu)")
    return {"path": path, "frames": frames, "transcript": fala.strip()}


@app.post("/api/sessions/{name}/transcribe", dependencies=[Depends(require_auth)])
async def transcribe_audio(name: str, request: Request, limpar: bool = False, estilo: str | None = None):
    # Salva o audio (pra anexar o path no chat) E transcreve via Groq num round-trip. Mesmo padrao
    # de upload (raw body + X-Filename). Devolve {path, text} -> o front monta "texto — 📎 audio: path".
    # `limpar` so o microfone manda: audio ANEXADO (arquivo de ate 10min) nao pode pagar a limpeza.
    # Desligado (default), a resposta e byte a byte a de sempre -> quem ja consome nao muda.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if not info.cwd:
        raise HTTPException(409, detail=erro("erro_cwd_indisponivel", "cwd da sessao indisponivel"))
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 100 * 1024 * 1024:
        raise HTTPException(413, detail=erro("erro_arquivo_grande", "arquivo maior que 100 MiB"))
    data = await request.body()
    filename = request.headers.get("x-filename") or request.query_params.get("name")
    try:
        path = await asyncio.to_thread(save_upload, info.cwd, _id_upload(info), data, filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)
    # Transcricao (chamada de rede bloqueante) no threadpool pra nao travar o loop.
    try:
        text = await asyncio.to_thread(transcribe, data, filename)
    except TranscribeError as e:
        raise HTTPException(e.status, e.detail)
    if not limpar:
        return {"path": path, "text": text}
    # `estilo` = o que a PILL do composer mostrava quando a pessoa falou. Vence a config do
    # servidor (narrar.estilo_efetivo); ausente/desconhecido, a config manda como sempre.
    texto_limpo, aviso = await asyncio.to_thread(narrar.limpar_ditado, text, estilo)
    # `estilo_aplicado` = qual versao o texto de fato recebeu, pra barra do ditado no composer marcar
    # o botao certo. NAO da pra deduzir na tela: o backend rebaixa briefing pra prosa em ditado curto
    # e cai na config quando a pill ainda nao leu o servidor, entao marcar "Briefing" pelo que foi
    # PEDIDO faria o botao mentir. Com aviso, o texto que voltou e o cru — nao um estilo. Idem
    # quando limpar_ditado devolve o proprio texto sem tocar (ditado de menos de 5 palavras, ou
    # comecando com "/"): ali nao houve estilo nenhum, e dizer "prosa" seria a mesma mentira.
    aplicado = "cru" if (aviso or texto_limpo == text) else narrar.estilo_efetivo(text, estilo)
    return {"path": path, "text": texto_limpo, "raw": text, "aviso": aviso,
            "estilo_aplicado": aplicado}


class RelimparBody(_StrictBody):
    texto: str = Field(min_length=1)
    estilo: str


@app.post("/api/ditado/relimpar", dependencies=[Depends(require_auth)])
async def relimpar_ditado(body: RelimparBody):
    """Aplica OUTRO estilo ao texto CRU de um ditado que ja foi transcrito.

    Sem audio e sem sessao de proposito. A parte cara (Whisper) ja foi paga na transcricao e o cru
    volta de la no campo `raw`; trocar de estilo e so a limpeza de novo. Reenviar o audio custaria
    uma segunda transcricao — dinheiro e ~10s — pra chegar no mesmo texto cru. E limpeza nao le nada
    da sessao (nem cwd, nem provider), entao exigir `name` aqui so acrescentaria um registry.list()
    e um 404 possivel num caminho que nao precisa de nenhum dos dois.

    Estilo invalido e 400 e nao "cai no padrao": aqui a pessoa CLICOU num estilo, entao entregar
    outro calado seria mentir sobre o botao que ela apertou (na transcricao o estilo e um palpite da
    tela e cair na config e o certo)."""
    if body.estilo not in narrar.ESTILOS_DITADO:
        raise HTTPException(400, detail=erro(
            "erro_estilo_invalido",
            f"estilo '{body.estilo}' nao existe. Use um de: {', '.join(narrar.ESTILOS_DITADO)}."))
    texto, aviso = await asyncio.to_thread(narrar.limpar_ditado, body.texto, body.estilo)
    aplicado = "cru" if (aviso or texto == body.texto) else narrar.estilo_efetivo(body.texto, body.estilo)
    return {"text": texto, "aviso": aviso, "estilo_aplicado": aplicado}


class PensamentoPtBody(_StrictBody):
    # Teto por ITEM, e não só na quantidade: o texto vai pro provedor de LLM, e no Pi e no Kimi
    # este campo carrega raciocínio CRU, sem tamanho previsível. Mesma regra do TtsBody.
    textos: list[Annotated[str, Field(max_length=pensamento_pt.MAX_CHARS)]] = Field(
        min_length=1, max_length=20)


@app.post("/api/pensamento/pt", dependencies=[Depends(require_auth)])
async def pensamento_para_pt(body: PensamentoPtBody):
    """Resumo do pensamento em portugues, curto. Chamado quando a pessoa ABRE o bloco.

    Nunca 502: falha de provedor devolve o texto ORIGINAL (ver pensamento_pt.traduzir). O bloco ja
    esta aberto na tela quando esta chamada sai — trocar o conteudo por uma mensagem de erro seria
    apagar o que ela acabou de pedir pra ler.
    """
    saida = await asyncio.to_thread(pensamento_pt.traduzir_varios, body.textos)
    return {"textos": saida}


@app.get("/api/sessions/{name}/uploads/{filename}", dependencies=[Depends(require_auth)])
def serve_upload(name: str, filename: str):
    info = _cached_info_sync(name)
    if info is None or not info.cwd:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    try:
        path = resolve_upload(info.cwd, _id_upload(info), filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)
    return FileResponse(path)


@app.get("/api/sessions/{name}/uploads", dependencies=[Depends(require_auth)])
def list_session_uploads(name: str):
    # Galeria de anexos: a retencao vive no servidor, entao o prazo sai daqui pronto (o front so
    # desenha). Le do runtime_config, nao do env cru — senao a galeria mostraria um prazo e o
    # prune usaria outro.
    info = _cached_info_sync(name)
    if info is None or not info.cwd:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    return {"files": list_uploads(info.cwd, _id_upload(info), runtime_config.get("upload_retention_days"))}


class CheckoutBody(_StrictBody):
    branch: str


class GitActionBody(_StrictBody):
    # allowlist declarativa no schema (alem do git_ops)
    action: Literal["status", "pull", "fetch", "stash", "stash-pop", "log",
                    "revert-abort", "cherry-pick-abort"]


class GitPathBody(_StrictBody):
    path: str   # validado em git_ops contra a lista real de arquivos alterados (anti-traversal)


class GitPathDiffBody(_StrictBody):
    # `escopo` e str de proposito, nao Literal: o Literal era validado pelo FastAPI e o
    # cliente recebia 422 com detail em LISTA — o envelope erro_git_diff nunca chegava a
    # nascer. Quem rejeita o valor e o git_ops.path_diff, e o erro sai no envelope.
    path: str
    escopo: str = "branch"


class GitCommitBody(_StrictBody):
    message: str = Field(min_length=1)
    paths: list[str] = []        # sem min_length: amend=True aceita [] (reword); git_ops barra [] sem amend
    amend: bool = False
    new_branch: str | None = None


def _session_cwd(name: str) -> str:
    # cwd da sessao tmux (mesmo lookup do upload). 404 se a sessao/cwd nao existe.
    info = _cached_info_sync(name)
    if info is None or not info.cwd:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    return info.cwd


@app.get("/api/sessions/{name}/plan", dependencies=[Depends(require_auth)])
async def session_plan(name: str):
    """Detalhe do plano ativo da sessao + o markdown cru. O markdown vem JUNTO de proposito: o
    GET /sessions/{name}/file so serve path que aparece no transcript, e um plano descoberto por
    varredura (sessao nova, pos-/clear) nunca aparece la. O arquivo ja foi lido e parseado aqui."""
    # to_thread e obrigatorio: _session_cwd chama registry.list(), que forka `tmux list-panes` e
    # varre /proc inteiro. As outras 16 rotas que usam _session_cwd sao `def` sync (o FastAPI as
    # joga no threadpool sozinho); esta e async, entao I/O direto na corrotina travaria o loop
    # (mesma classe do incidente 2026-07-23). A HTTPException do 404 propaga pelo to_thread normal.
    cwd = await asyncio.to_thread(_session_cwd, name)   # ja levanta 404 sem sessao/cwd
    p = await asyncio.to_thread(plan_progress, cwd)
    if p is None:
        raise HTTPException(404, detail=erro("erro_sem_plano_ativo", "sem plano ativo"))
    try:
        markdown = await asyncio.to_thread(
            lambda: Path(p.path).read_text(encoding="utf-8", errors="replace"))
    except OSError:
        # plan_progress vem de cache; o arquivo pode ter sumido/perdido permissao entre a leitura
        # cacheada e esta segunda leitura. Degrada pra markdown vazio, mas NAO engole em silencio.
        _log.warning("falha lendo markdown do plano path=%s", p.path, exc_info=True)
        markdown = ""
    return {
        # `stem` (nome do arquivo) alem do `name` (ja sem o prefixo de data): e a chave que o
        # cliente devolve pra marcar step e arquivar, e sao os dois caminhos que o `name`, com a
        # data cortada, nao consegue reabrir.
        "name": p.name, "path": p.path, "stem": Path(p.path).stem,
        "task": p.task_idx, "task_total": p.task_total,
        "done": p.done, "total": p.total, "complete": p.complete,
        "tasks": [{"title": t.title, "done": t.done, "total": t.total,
                   "steps": [{"title": s.title, "done": s.done, "manual": s.manual, "idx": s.idx}
                             for s in t.steps]}
                  for t in p.tasks],
        "markdown": markdown,
    }


@app.get("/api/sessions/{name}/plans", dependencies=[Depends(require_auth)])
async def session_plans(name: str):
    """Todos os planos do repo, pro seletor. Inclui os nao-comecados e os completos — que a eleicao
    automatica descarta, e que sao exatamente os que o usuario precisa poder escolher."""
    cwd = await asyncio.to_thread(_session_cwd, name)
    r = await asyncio.to_thread(list_plans, cwd)
    if r is None:
        raise HTTPException(404, detail=erro("erro_sem_pasta_planos", "repo sem pasta de planos"))
    return r


@app.get("/api/orq", dependencies=[Depends(require_auth)])
async def orq_lista():
    """Execucoes de orquestracao (eventos.jsonl escrito pelo arbitro), mais recentes primeiro.
    A lista vem SEM os eventos crus — quem quer a linha do tempo pede o detalhe."""
    execs = await asyncio.to_thread(orq.listar_execucoes, orq.raiz_padrao())

    def _resumo(e):
        d = asdict(e)
        d.pop("eventos_execucao", None)
        for t in d["tasks"]:
            t.pop("eventos", None)
        return d

    return {"execucoes": [_resumo(e) for e in execs], "fichas": orq.fichas(execs)}


@app.get("/api/orq/{exec_id}", dependencies=[Depends(require_auth)])
async def orq_detalhe(exec_id: str):
    e = await asyncio.to_thread(orq.detalhe, orq.raiz_padrao(), exec_id)
    if e is None:
        raise HTTPException(404, detail=erro("erro_nao_encontrado", "execucao nao encontrada"))
    return asdict(e)


class PlanPinBody(_StrictBody):
    stem: str | None = None   # None = solta o pin e volta pra eleicao automatica


@app.post("/api/sessions/{name}/plan-pin", dependencies=[Depends(require_auth)])
async def session_plan_pin(name: str, body: PlanPinBody):
    """Fixa qual plano o painel mostra. Vale ate o plano fechar: em 100% o pin e ignorado e a
    eleicao automatica volta (ver planprog.plan_progress)."""
    cwd = await asyncio.to_thread(_session_cwd, name)
    root = await asyncio.to_thread(_plans_dir, cwd)
    if root is None:
        raise HTTPException(404, detail=erro("erro_sem_pasta_planos", "repo sem pasta de planos"))
    if body.stem is not None and body.stem != PIN_NONE:
        # So um plano que existe DE VERDADE nesta raiz. Sem isto, o stem viraria nome de arquivo
        # vindo do cliente — e a checagem de traversal do read_pin nao cobriria um nome valido
        # apontando pra plano de outro repo. A guarda de separador vem ANTES do isfile: com um
        # `../..` o proprio isfile ja responderia se existe .md fora da pasta de planos.
        if not is_safe_stem(body.stem):
            raise HTTPException(400, detail=erro("erro_nome_plano_invalido", f"nome de plano invalido: {body.stem}", nome=body.stem))
        alvo = os.path.join(root, body.stem + ".md")
        if not await asyncio.to_thread(os.path.isfile, alvo):
            raise HTTPException(404, detail=erro("erro_plano_nao_encontrado", f"plano nao encontrado: {body.stem}", nome=body.stem))
    try:
        await asyncio.to_thread(write_pin, root, body.stem)
    except PlanPinError as e:
        raise HTTPException(500, detail=erro("erro_gravar_pin", f"nao deu pra gravar o pin: {e}", erro=str(e)))
    return {"pinned": body.stem}


async def _plans_root(name: str) -> tuple[str, str]:
    """(cwd, raiz de planos). Devolve os DOIS porque `_session_cwd` chama `registry.list()`, que
    forka tmux e varre /proc — pedir o cwd de novo depois dobraria esse scan por clique."""
    cwd = await asyncio.to_thread(_session_cwd, name)
    root = await asyncio.to_thread(_plans_dir, cwd)
    if root is None:
        raise HTTPException(404, detail=erro("erro_sem_pasta_planos", "repo sem pasta de planos"))
    return cwd, root


class PlanStepBody(_StrictBody):
    stem: str
    idx: int      # 0-based, na ordem do documento — vem do proprio /plan
    done: bool


@app.post("/api/sessions/{name}/plan-step", dependencies=[Depends(require_auth)])
async def session_plan_step(name: str, body: PlanStepBody):
    """Marca/desmarca UM step no .md do plano. Quem marca no fluxo normal e o agente; isto existe
    pro caso dele esquecer — sem marcacao o plano nunca fecha e trava o painel na etapa errada."""
    cwd, root = await _plans_root(name)
    try:
        path = caminho_do_plano(root, body.stem)
        await asyncio.to_thread(marcar_step, path, body.idx, body.done)
    except PlanWriteError as e:
        # 409 e nao 500: os modos de falha reais aqui sao "o arquivo mudou/nao serve", nao bug do
        # servidor — e a UI PRECISA mostrar o texto (o clique some sem explicacao, senao).
        raise HTTPException(409, detail=erro("erro_marcar_step", f"nao deu pra marcar o step: {e}", erro=str(e)))
    p = await asyncio.to_thread(plan_progress, cwd)
    return {"done": p.done if p else None, "total": p.total if p else None,
            "complete": bool(p and p.complete)}


class PlanArchiveBody(_StrictBody):
    stem: str


@app.post("/api/sessions/{name}/plan-archive", dependencies=[Depends(require_auth)])
async def session_plan_archive(name: str, body: PlanArchiveBody):
    """Encerra o plano: move o .md (e o .html irmao) pra docs/superpowers/plans/feitos/."""
    _, root = await _plans_root(name)
    try:
        movidos = await asyncio.to_thread(arquivar, root, body.stem)
    except PlanWriteError as e:
        raise HTTPException(409, detail=erro("erro_arquivar_plano", f"nao deu pra arquivar: {e}", erro=str(e)))
    return {"moved": movidos}


@app.get("/api/sessions/{name}/branches", dependencies=[Depends(require_auth)])
def branches(name: str):
    try:
        return list_branches(_session_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/checkout", dependencies=[Depends(require_auth)])
def checkout(name: str, body: CheckoutBody):
    try:
        return switch_branch(_session_cwd(name), body.branch)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git", dependencies=[Depends(require_auth)])
def git(name: str, body: GitActionBody):
    try:
        return git_action(_session_cwd(name), body.action)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/files", dependencies=[Depends(require_auth)])
def git_files(name: str):
    try:
        cwd = _session_cwd(name)
        # sequencer: revert/cherry-pick em andamento (conflito) — o front deriva o botao de abort
        # DAQUI, nao de memoria de sessao (ver gitStore.svelte.ts:pendingAbort).
        return {"files": changed_files(cwd), "sequencer": sequencer_state(cwd)}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/log", dependencies=[Depends(require_auth)])
def git_log_route(name: str, q: str | None = None):
    try:
        commits = git_log(_session_cwd(name), grep=q)
        # Com busca ativa (q), NAO monta o grafo: --grep tira commits do meio e assign_lanes
        # desenharia arestas pra parents que sumiram da lista (lane que nunca fecha).
        return {"commits": commits if q else assign_lanes(commits)}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/diff", dependencies=[Depends(require_auth)])
def git_diff(name: str, body: GitPathBody):
    try:
        return file_diff(_session_cwd(name), body.path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/discard", dependencies=[Depends(require_auth)])
def git_discard(name: str, body: GitPathBody):
    try:
        return discard_file(_session_cwd(name), body.path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/commit", dependencies=[Depends(require_auth)])
def git_commit(name: str, body: GitCommitBody):
    try:
        return commit(_session_cwd(name), body.message, body.paths, body.amend, body.new_branch)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/last-message", dependencies=[Depends(require_auth)])
def git_last_message(name: str):
    try:
        return last_commit_message(_session_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/files", dependencies=[Depends(require_auth)])
def git_commit_files(name: str, sha: str):
    try:
        return {"files": commit_files(_session_cwd(name), sha)}
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/diff", dependencies=[Depends(require_auth)])
def git_commit_diff(name: str, sha: str, path: str):
    try:
        return commit_file_diff(_session_cwd(name), sha, path)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


class GitShaBody(_StrictBody):
    sha: str   # validado em git_ops por _SHA_RE (hex 7-40) antes de virar argv


class GitResetBody(_StrictBody):
    sha: str
    mode: Literal["soft", "mixed", "hard"]   # enum no schema E no git_ops


class GitBranchBody(_StrictBody):
    name: str
    sha: str | None = None
    switch_after: bool = False


class GitTagBody(_StrictBody):
    name: str
    sha: str | None = None
    message: str | None = None


@app.get("/api/sessions/{name}/git/commit/{sha}/diff-full", dependencies=[Depends(require_auth)])
def git_commit_diff_full(name: str, sha: str):
    try:
        return commit_diff(_session_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/revert", dependencies=[Depends(require_auth)])
def git_revert(name: str, body: GitShaBody):
    try:
        return revert_commit(_session_cwd(name), body.sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/cherry-pick", dependencies=[Depends(require_auth)])
def git_cherry_pick(name: str, body: GitShaBody):
    try:
        return cherry_pick(_session_cwd(name), body.sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/push", dependencies=[Depends(require_auth)])
def git_push(name: str):
    try:
        return push_branch(_session_cwd(name))
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/reset", dependencies=[Depends(require_auth)])
def git_reset(name: str, body: GitResetBody):
    try:
        return reset_to(_session_cwd(name), body.sha, body.mode)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/branch", dependencies=[Depends(require_auth)])
def git_branch_create(name: str, body: GitBranchBody):
    try:
        return create_branch_at(_session_cwd(name), body.name, body.sha, body.switch_after)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/git/tag", dependencies=[Depends(require_auth)])
def git_tag_create(name: str, body: GitTagBody):
    try:
        return create_tag(_session_cwd(name), body.name, body.sha, body.message)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/diff-worktree", dependencies=[Depends(require_auth)])
def git_commit_diff_worktree(name: str, sha: str):
    try:
        return diff_vs_worktree(_session_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/git/commit/{sha}/branches", dependencies=[Depends(require_auth)])
def git_commit_branches(name: str, sha: str):
    try:
        return branches_containing(_session_cwd(name), sha)
    except GitError as e:
        raise HTTPException(e.status, e.detail)


# Textos FIXOS do envelope, por familia de erro: o detalhe interno (e.msg/str(e) do
# SearchError e do GitError pode carregar caminho absoluto ou stderr do git, e atravessar
# a API exporia segredo. O detalhe vai SO para o log, passando pelo _scrub (redige
# userinfo de remote). O front mostra a chave traduzida; o `msg` do envelope e a rede
# quando o front nao conhece o code — e ele tambem e fixo, por isso.
_MSG_ARQ = "Nao deu pra acessar esse arquivo ou pasta."
_MSG_BUSCA = "Nao deu pra completar a busca."
_MSG_DIFF = "Nao deu pra montar o diff."


def _erro_arq(e: FileError | SearchError) -> HTTPException:
    # As chaves `erro_arq_busca_falhou` e `erro_git_diff` trazem `{msg}` no texto, e a
    # funcao do paraglide exige o argumento — sem ele o front renderiza `undefined` ou
    # nem compila. O `erro()` tem `msg` como parametro nomeado, entao o valor entra no
    # dict de params DEPOIS, por chave.
    fixo = _MSG_BUSCA if isinstance(e, SearchError) else _MSG_ARQ
    _log.warning("files: %s", git_ops._scrub(e.msg))
    d = erro(e.code, fixo)
    d["params"]["msg"] = fixo
    return HTTPException(status_code=e.status, detail=d)


@app.get("/api/sessions/{name}/files/list", dependencies=[Depends(require_auth)])
def files_list(name: str, path: str | None = None, so_modificados: bool = True):
    try:
        return filetree.list_dir(_session_cwd(name), path, so_modificados)
    except FileError as e:
        raise _erro_arq(e)


@app.get("/api/sessions/{name}/files/read", dependencies=[Depends(require_auth)])
def files_read(name: str, path: str):
    try:
        return filetree.read_file(_session_cwd(name), path)
    except FileError as e:
        raise _erro_arq(e)


class FileWriteBody(_StrictBody):
    path: str
    text: str
    # A impressão da leitura. Sem ela a gravação é recusada: escrever às cegas por cima do que o
    # agente da sessão acabou de mudar é o desfecho que este campo existe pra impedir.
    digest: str | None = None


@app.post("/api/sessions/{name}/files/write", dependencies=[Depends(require_auth)])
def files_write(name: str, body: FileWriteBody):
    try:
        return filetree.write_file(_session_cwd(name), body.path, body.text, body.digest)
    except FileError as e:
        raise _erro_arq(e)


@app.get("/api/sessions/{name}/files/search", dependencies=[Depends(require_auth)])
def files_search(name: str, q: str, mode: str = "names"):
    # `mode` e str de proposito, nao Literal: o Literal era validado pelo FastAPI e o 422
    # com detail em LISTA engolia o erro_arq_modo_invalido. Quem rejeita e o filesearch.
    try:
        return filesearch.search(_session_cwd(name), q, mode)
    except SearchError as e:
        raise _erro_arq(e)


class ResolverBody(_StrictBody):
    caminhos: list[str]


@app.post("/api/sessions/{name}/files/resolver", dependencies=[Depends(require_auth)])
def files_resolver(name: str, body: ResolverBody):
    """Visão "citados": confere de uma vez quais caminhos citados existem (e resolve os relativos
    a outra pasta pelo sufixo). Quem não existe não entra na lista."""
    try:
        return filesearch.resolver(_session_cwd(name), body.caminhos)
    except SearchError as e:
        raise _erro_arq(e)


@app.post("/api/sessions/{name}/git/path-diff", dependencies=[Depends(require_auth)])
def git_path_diff(name: str, body: GitPathDiffBody):
    try:
        return git_ops.path_diff(_session_cwd(name), body.path, body.escopo)
    except GitError as e:
        _log.warning("path-diff: %s", git_ops._scrub(str(e)))
        d = erro("erro_git_diff", _MSG_DIFF)
        d["params"]["msg"] = _MSG_DIFF
        raise HTTPException(status_code=e.status, detail=d)


@app.get("/api/sessions/{name}/runners", dependencies=[Depends(require_auth)],
         response_model=RunnersResponse)
def list_runners(name: str):
    cwd = _session_cwd(name)
    return RunnersResponse(
        detected=runner.detect_runners(cwd),
        remembered=runner.remembered(cwd),
        running=runner.run_status(cwd),
    )


@app.post("/api/sessions/{name}/run", dependencies=[Depends(require_auth)],
          response_model=RunInfo)
def start_runner(name: str, body: RunBody):
    # RunnerError = "o play NAO aconteceu" (a sessao velha sobreviveu, ou o new-session falhou).
    # Antes isso virava 200 com o estado da sessao VELHA dentro; 500 cru tambem nao serve, porque
    # o texto diz o que houve e a tela do run mostra o detail, igual ao painel de projetos.
    try:
        return runner.start_run(_session_cwd(name), body.command)
    except runner.RunnerError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/sessions/{name}/run/stop", dependencies=[Depends(require_auth)])
def stop_runner(name: str):
    try:
        runner.stop_run(_session_cwd(name))
    except runner.RunnerError as e:
        raise HTTPException(e.status, e.detail)   # `{"ok": True}` com o processo vivo era mentira
    return {"ok": True}


@app.get("/api/sessions/{name}/run/pane", dependencies=[Depends(require_auth)])
def runner_pane(name: str):
    return {"pane": runner.run_pane(_session_cwd(name))}


# --- launcher de projetos (standalone, chaveado pelo projects.json — nao por sessao viva) ----

@app.get("/api/projects", dependencies=[Depends(require_auth)],
         response_model=list[ProjectStatus])
def projects_list():
    try:
        return projects.list_projects()
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/projects/{name}/start", dependencies=[Depends(require_auth)],
          response_model=ProjectStatus)
def project_start(name: str):
    try:
        return projects.start(name)
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/projects/{name}/stop", dependencies=[Depends(require_auth)])
def project_stop(name: str):
    try:
        projects.stop(name)
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)
    return {"ok": True}


@app.get("/api/projects/{name}/pane", dependencies=[Depends(require_auth)])
def project_pane(name: str):
    try:
        return {"pane": projects.pane(name)}
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)


class ProjectUpsert(BaseModel):
    name: str
    cwd: str
    command: str
    port: Optional[int] = None          # Pydantic coage "3000" (string do form QML) -> int
    stop_command: Optional[str] = None


@app.post("/api/projects", dependencies=[Depends(require_auth)], response_model=ProjectStatus)
def project_upsert(body: ProjectUpsert):
    try:
        return projects.upsert(body.name, body.cwd, body.command, body.port, body.stop_command)
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)


@app.delete("/api/projects/{name}", dependencies=[Depends(require_auth)])
def project_delete(name: str):
    try:
        projects.remove(name)
    except projects.ProjectError as e:
        raise HTTPException(e.status, e.detail)
    return {"ok": True}


@app.post("/api/sessions/{name}/open-editor", dependencies=[Depends(require_auth)])
def open_editor(name: str):
    # So-desktop: abre o editor na MAQUINA do backend, no cwd da sessao. Binario fixo (settings.editor,
    # nao input do cliente) + arg unico validado -> sem shell, sem injecao. GUI precisa do DISPLAY/
    # WAYLAND_DISPLAY do backend (sessao grafica); sob systemd headless pode nao abrir -> 500.
    cwd = _session_cwd(name)
    binario = runtime_config.get("editor")
    # Rastro: com o editor editavel pelo app, um exec silencioso seria o caminho menos auditavel do
    # backend (o fluxo normal de comando fica gravado no transcript; este nao ficava em lugar nenhum).
    _log.info("OPEN-EDITOR name=%s bin=%r cwd=%r", name, binario, cwd)
    try:
        subprocess.Popen([binario, cwd],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError as e:
        raise HTTPException(500, detail=erro("erro_editor_falhou", f"editor '{binario}' falhou: {e}", binario=binario, erro=str(e)))
    return {"ok": True}


@app.get("/api/sessions/{name}/transcript-image/{uuid}/{idx}", dependencies=[Depends(require_auth)])
def transcript_image(name: str, uuid: str, idx: int):
    # Serve uma imagem colada no TERMINAL (base64 no .jsonl) sob demanda. Decodifica por uuid+idx.
    info = _cached_info_sync(name)
    jsonl = info.jsonl if info else None
    if not jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.transcript import get_transcript_image
    got = get_transcript_image(jsonl, uuid, idx)
    if got is None:
        raise HTTPException(404, detail=erro("erro_imagem_nao_encontrada", "image not found"))
    raw, media = got
    # immutable: o conteudo de um uuid+idx nunca muda -> cache agressivo no cliente.
    return Response(content=raw, media_type=media, headers={"Cache-Control": "max-age=31536000, immutable"})


def _json_dict(linha: str) -> dict | None:
    try:
        o = json.loads(linha)
    except (json.JSONDecodeError, ValueError):
        return None
    return o if isinstance(o, dict) else None


# ── Arquivo: conversas mortas (transcripts sem sessao tmux viva) ──────────────
@app.get("/api/archive", dependencies=[Depends(require_auth)], response_model=list[ArchiveFolder])
def archive_index():
    # Nivel 1: so as PASTAS (agregado barato). As conversas vem por pasta, no endpoint abaixo.
    return list_folders()


@app.get("/api/archive-por-cwd", dependencies=[Depends(require_auth)],
         response_model=list[ArchiveEntry])
def archive_por_cwd(cwd: str, config_dir: str | None = None, cap: int = 12,
                    provider: str = "claude"):
    """Conversas retomaveis de UM cwd, do agente e da conta pedidos — o que o modal de sessao nova
    lista embaixo do formulario. Path proprio (nao `/api/archive/{project}`) pra nao disputar a rota
    com um nome de projeto. Pasta sem conversa nenhuma = lista vazia, nao 404: no modal isso e o
    caso comum (pasta nova), nao erro. `cap` baixo porque aqui a lista e um atalho, nao o Arquivo.

    `config_dir` so vale pro Claude — Pi, Kimi e Codex nao tem conta, e passa-lo os excluiria."""
    if config_dir is not None and config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if provider != "claude" and provider not in archive_providers.PROVIDERS:
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider invalido"))
    live = {os.path.realpath(s.jsonl) for s in registry.list() if s.jsonl}
    try:
        todas = list_conversations(sanitize_cwd(cwd), live, cap=cap * 4,
                                   config_dir=config_dir if provider == "claude" else None)
    except (ValueError, FileNotFoundError):
        return []
    return [e for e in todas if e.provider == provider][:cap]


@app.get("/api/archive/{project}", dependencies=[Depends(require_auth)],
         response_model=list[ArchiveEntry])
def archive_folder(project: str):
    # live = transcripts em uso agora (badge na lista; a conversa viva abre pelo chat normal).
    live = {os.path.realpath(s.jsonl) for s in registry.list() if s.jsonl}
    try:
        return list_conversations(project, live)
    except ValueError:
        raise HTTPException(400, detail=erro("erro_path_invalido", "invalid path"))
    except FileNotFoundError:
        raise HTTPException(404, detail=erro("erro_projeto_nao_encontrado", "project not found"))


@app.get("/api/archive/{project}/{session_id}/history",
         dependencies=[Depends(require_auth)], response_model=list[ChatEvent])
def archive_history(project: str, session_id: str, tail: int = 0, config_dir: str | None = None,
                    provider: str = "claude"):
    # `tail=N` = so as N ultimas mensagens, lidas pelo FIM do arquivo (a previa do modal de sessao
    # nova). Sem ele, o historico inteiro, como sempre — e um transcript de 19MB carregado inteiro
    # so pra mostrar cinco balões era o que essa via evita.
    if config_dir is not None and config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if provider != "claude" and provider not in archive_providers.PROVIDERS:
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider invalido"))
    try:
        if tail > 0:
            return tail_events(project, session_id, min(tail, 200), config_dir, provider)
        p = archive_jsonl(project, session_id, config_dir, provider)
        if provider != "claude":
            # Fora do Claude nao ha fila duravel keyed por este arquivo: o transcript e a conversa
            # inteira, e cada provider tem o parser dele.
            return [ev for linha in p.read_text(encoding="utf-8", errors="replace").splitlines()
                    if (o := _json_dict(linha)) is not None
                    for ev in archive_providers.parse_obj(provider, o)]
    except ValueError:
        raise HTTPException(400, detail=erro("erro_path_invalido", "invalid path"))
    except FileNotFoundError:
        raise HTTPException(404, detail=erro("erro_transcript_nao_encontrado", "transcript not found"))
    from app.pqueue import merged_history
    # Nome de fila inexistente -> sem entradas de fila: so os eventos do transcript, ordenados por ts.
    return merged_history("__archive__", str(p))


@app.get("/api/archive/{project}/{session_id}/transcript-image/{uuid}/{idx}",
         dependencies=[Depends(require_auth)])
def archive_image(project: str, session_id: str, uuid: str, idx: int):
    try:
        p = archive_jsonl(project, session_id)
    except (ValueError, FileNotFoundError):
        raise HTTPException(404, detail=erro("erro_nao_encontrado", "not found"))
    from app.transcript import get_transcript_image
    got = get_transcript_image(str(p), uuid, idx)
    if got is None:
        raise HTTPException(404, detail=erro("erro_imagem_nao_encontrada", "image not found"))
    raw, media = got
    return Response(content=raw, media_type=media, headers={"Cache-Control": "max-age=31536000, immutable"})


class ResumeArchivedBody(_StrictBody):
    # Motor de modelo pro resume do Arquivo. O pane que rodava a sessao original ja morreu -> nao ha
    # /proc pra descobrir o motor de entao (ver registry._engine_of); quem retoma escolhe de novo.
    # Sem escolha, volta na conta Anthropic (comportamento de hoje).
    engine: str | None = None
    # A CONTA dona do transcript. Sem isto o resume nascia sempre na conta padrao, e um `claude
    # --resume <uuid>` de outra conta morre na hora com "No conversation found with session ID".
    config_dir: str | None = None
    # Agente dono da conversa. Pi e Kimi retomam com o comando DELES (`pi --session-id`,
    # `kimi --session`); Codex nao tem via de resume aqui e e recusado logo abaixo.
    provider: str = "claude"


@app.post("/api/archive/{project}/{session_id}/resume", dependencies=[Depends(require_auth)],
          response_model=SessionInfo)
def resume_archived(project: str, session_id: str, body: ResumeArchivedBody = ResumeArchivedBody()):
    # "Retomar conversa" do Arquivo: sobe uma sessao tmux NOVA no cwd original com `claude --resume
    # <uuid>` -- reusa registry.create (nome/config_dir/spawn tmux ja tratados), so troca o comando pro
    # uuid EXISTENTE (nao um novo transcript). Nome derivado do basename do cwd, igual ao
    # CreateSessionSheet do front; colisao suffixa -2/-3... (mesmo esquema, do lado do backend pq aqui
    # nao ha form pro usuario escolher nome).
    from app import tmux
    if body.config_dir is not None and body.config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, detail=erro("erro_config_dir_invalido", "config_dir invalido"))
    if body.provider != "claude" and body.provider not in archive_providers.PROVIDERS:
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider invalido"))
    # O id da conversa Codex e o uuid do FIM do nome do rollout, e e ele que o `codex resume` recebe.
    # Um nome fora desse padrao nao tem id pra retomar — e dizer "caminho invalido" (o que o
    # ValueError generico daqui a pouco daria) manda procurar defeito no lugar errado.
    if body.provider == "codex" and not archive_providers.UUID_RE.match(session_id):
        raise HTTPException(400, detail=erro("erro_rollout_sem_id",
                                             "nome de rollout sem id de conversa"))
    # Conta omitida (link antigo, chamador que nao sabe): descobre no disco. Deixar None aqui subia
    # o pane na conta do backend e o `--resume` morria com "No conversation found with session ID".
    # Conversa que nao existe nao vira erro AQUI: o archive_cwd logo abaixo faz a mesma busca e e
    # ele quem devolve o 400/404 -- duplicar a recusa so daria duas mensagens pro mesmo caso.
    # So o Claude tem conta: Pi e Kimi guardam transcript fora do config dir.
    cfg = body.config_dir
    if cfg is None and body.provider == "claude":
        try:
            cfg = conta_de(project, session_id)
        except (ValueError, FileNotFoundError):
            cfg = None
    try:
        cwd = archive_cwd(project, session_id, cfg, body.provider)
    except ValueError:
        raise HTTPException(400, detail=erro("erro_path_invalido", "invalid path"))
    except FileNotFoundError:
        raise HTTPException(404, detail=erro("erro_transcript_nao_encontrado", "transcript not found"))
    if not cwd:
        raise HTTPException(422, detail=erro("erro_cwd_ausente", "cwd not found in transcript"))
    if body.engine is not None and body.engine not in engines.listar():
        raise HTTPException(400, detail=erro("erro_motor_invalido", "motor invalido"))
    base = sanitize_session_name(Path(cwd).name) or "sessao"
    name, i = base, 2
    # As MESMAS duas fontes que a criacao normal consulta (registry.create). Olhando so o tmux, um
    # nome ja usado por uma sessao Codex viva passava por aqui e o conflito estourava la dentro,
    # como um 409 com a mensagem de outro assunto.
    while tmux.has_session(name) or codex_sessions.exists(name):
        name = f"{base}-{i}"
        i += 1
    try:
        return registry.create(name, cwd, config_dir=cfg, provider=body.provider,
                               resume_session_id=session_id, engine=body.engine)
    except ValueError as e:
        raise HTTPException(409, str(e))


# ── Busca de conteudo cross-session: grep (rg) em todos os transcripts (vivos + arquivados) ──
@app.get("/api/search", dependencies=[Depends(require_auth)], response_model=list[SearchHit])
def search_transcripts(q: str = ""):
    # live: realpath(jsonl) -> nome tmux das sessoes VIVAS (mesmo join do /api/archive). A busca marca
    # o hit como vivo e carrega o nome pra a UI abrir o chat (viva) ou o arquivo (morta). q vazia -> [].
    live = {os.path.realpath(s.jsonl): s.name for s in registry.list() if s.jsonl}
    return search(q, live)


class AskHistoryBody(_StrictBody):
    question: str = Field(min_length=1, max_length=500)


@app.post("/api/ask-history", dependencies=[Depends(require_auth)])
def ask_history(body: AskHistoryBody):
    """RAG lexical ("onde falei sobre X"): extrai termos da pergunta -> busca OR nos transcripts ->
    claude -p resume EM QUAL sessao o assunto apareceu. Sob o kill-switch (dispara claude -p). Sem
    trecho -> resposta vazia sem chamar o CLI. v1: so o servidor que recebe a chamada (cross-server v2)."""
    if not automations_enabled():
        raise HTTPException(409, detail=erro("erro_automacoes_desligadas", "automações desligadas (kill-switch)"))
    live = {os.path.realpath(s.jsonl): s.name for s in registry.list() if s.jsonl}
    hits = search_terms(extract_terms(body.question), live)
    if not hits:
        return {"answer": "não achei nada sobre isso nas conversas", "hits": []}
    try:
        answer = loop_mod._claude_p(build_ask_prompt(body.question, hits))
    except loop_mod.ClaudePError as e:
        _log.warning("ask-history falhou: %s", e)
        raise HTTPException(502, str(e))
    return {"answer": answer, "hits": hits}


# Anexo citado na conversa: 60s sem perguntar + ETag pro resto. A miniatura de 96px do
# FileAttachment carrega o arquivo ORIGINAL, entao sem cache toda repintura da lista rebaixava o
# PNG inteiro. Starlette 1.3.1 poe o ETag no FileResponse mas NAO responde 304 — o 304 abaixo e
# nosso. ponytail: arquivo reescrito no MESMO caminho so aparece depois dos 60s; e o preco de nao
# perguntar. Documento (html/pdf) que o agente regenera e o caso que mais sente isso.
_CACHE_ARQUIVO = "max-age=60"


@app.get("/api/sessions/{name}/file", dependencies=[Depends(require_auth)])
def serve_file(name: str, path: str, request: Request):
    # Serve QUALQUER arquivo referenciado na conversa (video/html/codigo/pdf/...). TRAVA de seguranca:
    # so serve se o `path` aparece no transcript desta sessao (citado por voce ou pelo Claude =
    # consentido) E existe E e arquivo regular -> bloqueia leitura arbitraria de disco / path-traversal.
    # FileResponse trata Range -> <video> faz seek/streaming.
    # Path RELATIVO (ex "./mock.png", "sub/x.png") resolve contra o CWD DA SESSAO (onde o Claude criou
    # o arquivo), nao o cwd do processo backend; guard extra: o resolvido nao pode ESCAPAR do cwd.
    info = _cached_info_sync(name)
    if info is None or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "session or transcript not found"))
    from app.transcript import path_in_transcript
    if not path_in_transcript(info.jsonl, path):
        raise HTTPException(403, detail=erro("erro_arquivo_nao_citado", "file not referenced in this conversation"))
    expanded = os.path.expanduser(path)
    if os.path.isabs(expanded):
        real = os.path.realpath(expanded)
    else:
        if not info.cwd:
            raise HTTPException(409, detail=erro("erro_cwd_indisponivel", "cwd da sessao indisponivel"))
        base = os.path.realpath(info.cwd)
        real = os.path.realpath(os.path.join(base, expanded))
        if real != base and not real.startswith(base + os.sep):
            raise HTTPException(403, detail=erro("erro_caminho_fora_cwd", "path escapes session cwd"))
    if not os.path.isfile(real):
        raise HTTPException(404, detail=erro("erro_arquivo_nao_encontrado", "file not found"))
    media = mimetypes.guess_type(real)[0] or "application/octet-stream"
    st = os.stat(real)
    etag = f'"{st.st_mtime_ns:x}-{st.st_size:x}"'
    cabecalhos = {"etag": etag, "cache-control": _CACHE_ARQUIVO}
    # Depois da trava do transcript, nunca antes: 304 e resposta sobre um arquivo, e quem nao pode
    # ver o arquivo tambem nao pode saber que ele mudou.
    if request.headers.get("if-none-match") == etag:
        return Response(status_code=304, headers=cabecalhos)
    return FileResponse(real, media_type=media, headers=cabecalhos)


class AnswerItem(_StrictBody):
    kind: str
    indices: list[int] | None = None
    multi: bool = False
    value: str | None = None
    labels: list[str] = []
    type_index: int | None = None
    chat_index: int | None = None


class AnswerBody(_StrictBody):
    answers: list[AnswerItem]


def _askq_fallback_text(answers: list[dict], jsonl: str | None) -> str:
    """Monta a resposta em TEXTO pro fallback do AskUserQuestion (drive da TUI falhou): pareia cada
    answer com a pergunta do sidecar (mesma ordem — o stepper monta answers via questions.map) e vira
    linhas "pergunta → resposta". Sem sidecar, so as respostas. kind=chat nao vira linha (o usuario
    escolheu conversar — o Escape do fallback ja o poe no chat)."""
    questions = []
    if jsonl:
        askq = read_pending_askq(jsonl)
        if askq:
            questions = askq.questions
    lines = []
    for i, a in enumerate(answers):
        if a["kind"] == "option":
            resp = ", ".join(a.get("labels") or [])
        elif a["kind"] == "text":
            resp = a.get("value") or ""
        else:  # chat: sem resposta estruturada
            continue
        if not resp:
            continue
        q = questions[i].question if i < len(questions) else None
        lines.append(f"- {q} → {resp}" if q else f"- {resp}")
    if not lines:
        return ""
    return "Respondendo as perguntas (o seletor de opções falhou, vai por texto):\n" + "\n".join(lines)


def _pi_answer_fallback_text(a: dict) -> str:
    """Resposta em TEXTO pro fallback da pergunta do Pi (drive do picker falhou). Mesma filosofia
    do _askq_fallback_text do Claude: a resposta do usuario NUNCA se perde — vira mensagem normal."""
    if a.get("kind") == "option":
        resp = ", ".join(a.get("labels") or [])
    elif a.get("kind") == "text":
        resp = a.get("value") or ""
    else:
        resp = ""
    if not resp:
        return ""
    # Texto NEUTRO. Dizia "o seletor de opções falhou" — e no Kimi isso era mentira ate hoje: la nao
    # havia drive de teclas, o texto era o caminho NORMAL e unico. O usuario lia "falhou" na propria
    # conversa e achava que a resposta tinha dado errado (relatado em 13/08/2026), e o agente lia a
    # mesma frase e respondia ao fantasma.
    #
    # Quem soube que houve fallback foi o LOG do servidor. O `fallback: true` volta no corpo da
    # resposta, mas o front descarta (`api.ts answerQuestions` tipa so `{ok}`) — entao nao prometa
    # aqui que o usuario ve isso. O que ele ve e a propria resposta virando mensagem no chat, que ja
    # diz "foi por texto" sem precisar de aviso.
    return f"Respondendo à pergunta: {resp}"


@app.post("/api/sessions/{name}/answer", dependencies=[Depends(require_auth)])
def answer(name: str, body: AnswerBody):
    # Dirige o AskUserQuestion tabbed: reproduz as teclas (nav em malha fechada), confere o Review e
    # submete. Input invalido -> 409. Drive falhou (DriveError: nada submetido, sem Escape) ->
    # FALLBACK automatico: Escape (fecha o picker; o "declined" e intencional aqui) + resposta como
    # texto via _send_one (fila duravel: se o pane ainda estiver em overlay vira deferred e o drain
    # entrega). A resposta do usuario NUNCA se perde — pior caso chega como texto, nao como interrupt mudo.
    _recusa_se_painel_aberto(name)
    from app import terminal_input
    answers = [a.model_dump() for a in body.answers]
    info = _cached_info_sync(name)
    jsonl = info.jsonl if info else None
    fallback = False

    # Pi: a pergunta nativa (tool `question`) mora no proprio transcript — o front sintetiza o
    # payload do AskUserQuestion a partir do tool_use pendente e posta aqui igual; o drive e outro
    # (picker ascii do Pi, sem tela de Review). A pergunta some da fila (respondida no terminal)
    # entre o card abrir e o toque -> 409 legivel, nunca drive as cegas.
    if getattr(info, "provider", "claude") == "pi":
        from app.adapters.pi.transcript import read_pending_question
        q = read_pending_question(jsonl) if jsonl else None
        if q is None:
            raise HTTPException(409, detail=erro("erro_sem_pergunta_pi", "nenhuma pergunta do Pi pendente (ja respondida no terminal?)"))
        if not answers:
            raise HTTPException(409, detail=erro("erro_sem_resposta", "sem resposta"))
        try:
            terminal_input.answer_question_pi(name, answers[0], q)
        except ValueError as e:
            raise HTTPException(409, str(e))
        except terminal_input.DriveError as e:
            text = _pi_answer_fallback_text(answers[0])
            _log.warning("PI-QUESTION fallback name=%s reason=%s text=%r", name, e, text[:120])
            if not text:
                # Sem texto de fallback, NAO manda o Escape: picker aberto = usuario ainda responde
                # no terminal. Fechar e devolver ok sem entregar nada seria a pior saida (silencio).
                raise HTTPException(409, detail=erro("erro_drive_sem_fallback", f"drive falhou ({e}) e nao ha texto de fallback — responda no terminal", erro=str(e)))
            terminal.interrupt(name)  # Escape unico: fecha o picker do Pi (sem clear — input vazio)
            _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
            res = _send_one(name, text)
            if not res["ok"]:
                raise HTTPException(409, detail=erro("erro_drive_fallback_falhou", f"drive falhou e fallback por texto tambem: {_erro_texto(res['error'])}", erro=res['error']))
            _recusa_se_so_enfileirou(name, res)
            fallback = True
        return {"ok": True, "fallback": fallback}

    # Kimi: a pergunta nativa (tool AskUserQuestion) mora no wire — o front sintetiza o card a
    # partir do tool_use pendente, igual ao Pi. O drive do picker foi medido em 13/08/2026 (Kimi
    # 0.36.0) e e mais confiavel que o dos outros dois: as opcoes sao numeradas e a tecla numerica
    # escolhe e avanca (sem contar linha), e a CONFIRMACAO nao e visual — o `tool.result` daquele
    # toolCallId aparecendo no wire prova que a ferramenta recebeu. Drive falhou -> Escape +
    # fallback por texto, igual Claude/Pi: a resposta do usuario nunca se perde.
    if getattr(info, "provider", "claude") == "kimi":
        from app.adapters.kimi.transcript import read_pending_call, resposta_chegou
        pend = read_pending_call(jsonl) if jsonl else None
        if pend is None:
            raise HTTPException(409, detail=erro("erro_sem_pergunta_kimi", "nenhuma pergunta do Kimi pendente (ja respondida no terminal?)"))
        if not answers:
            raise HTTPException(409, detail=erro("erro_sem_resposta", "sem resposta"))
        call_id, args = pend
        perguntas = args.get("questions") if isinstance(args.get("questions"), list) else []
        try:
            terminal_input.answer_question_kimi(name, answers, perguntas)
            # PROVA no transcript, nao no pane: o Kimi so escreve o tool.result depois de a
            # ferramenta receber as respostas. Sem esta checagem, um Submit que nao pegou voltaria
            # 200 com cara de sucesso — o mesmo "sent sem chegar" que ja custou uma resposta perdida.
            if not _espera_resposta_kimi(jsonl, call_id):
                # Prazo estourado NAO prova que nada foi submetido — pode ser o Kimi demorando pra
                # gravar. Os outros dois drivers so levantam DriveError com prova estrutural (o
                # picker AINDA na tela), e aqui vale o mesmo: se o picker sumiu, alguem submeteu.
                # Cair no fallback nesse caso mandaria Escape num turno que ja processa a resposta
                # certa e entregaria a mesma resposta DUAS vezes — uma pela ferramenta, outra como
                # mensagem. Entre duplicar calado e admitir a duvida, admite-se a duvida.
                if terminal_input.picker_kimi_aberto(name):
                    raise terminal_input.DriveError(
                        "Submit nao pegou: o picker continua aberto e o tool.result nao apareceu")
                raise HTTPException(409, detail=erro("erro_sem_confirmacao_resposta",
                                             "resposta enviada, mas nao deu pra confirmar a tempo — "
                                             "confira na sessao antes de responder de novo"))
        except ValueError as e:
            raise HTTPException(409, str(e))
        except terminal_input.DriveError as e:
            text = _pi_answer_fallback_text(answers[0])
            _log.warning("KIMI-QUESTION fallback name=%s reason=%s text=%r", name, e, text[:120])
            if not text:
                # Sem texto de fallback, NAO manda o Escape: picker aberto = o usuario ainda pode
                # responder no terminal. Fechar e devolver ok sem entregar nada seria a pior saida.
                raise HTTPException(409, detail=erro("erro_drive_sem_fallback", f"drive falhou ({e}) e nao ha texto de fallback — responda no terminal", erro=str(e)))
            terminal.interrupt(name)  # Escape unico: fecha o picker do Kimi (sem clear — input vazio)
            _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
            res = _send_one(name, text)
            if not res["ok"]:
                raise HTTPException(409, detail=erro("erro_drive_fallback_falhou", f"drive falhou e fallback por texto tambem: {_erro_texto(res['error'])}", erro=res['error']))
            _recusa_se_so_enfileirou(name, res)
            return {"ok": True, "fallback": True}
        return {"ok": True, "fallback": False}
    try:
        terminal_input.answer_questions(name, answers)
    except ValueError as e:
        raise HTTPException(409, str(e))
    except terminal_input.DriveError as e:
        text = _askq_fallback_text(answers, jsonl)
        _log.warning("ASKQ fallback name=%s reason=%s text=%r", name, e, text[:120])
        # Diario: o log do servico vive o que a maquina deixar viver (o journal do dia seguinte ja
        # nao tinha as duas quedas de 28/08/2026), e sem o MOTIVO nao da pra separar "picker preso"
        # de "nav drift" quando o relato chega dias depois.
        diag.registrar("pergunta.fallback_texto", "erro", sessao=name, detalhe=str(e))
        if not text:
            # Sem texto de fallback (resposta `chat`, ou rotulos vazios) nao ha o que entregar. Nao
            # manda o Escape e nao limpa o sidecar: o picker segue aberto pra quem responder no
            # terminal. Ate 01/09/2026 este ramo caia em `fallback = True` e apagava o sidecar como
            # se tivesse respondido, sem uma tecla ter saido — os ramos Pi e Kimi ja barravam.
            raise HTTPException(409, detail=erro(
                "erro_drive_sem_fallback",
                f"drive falhou ({e}) e nao ha texto de fallback — responda no terminal", erro=str(e)))
        terminal.interrupt(name)  # Escape unico: fecha o picker (sem clear — input vazio)
        _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
        res = _send_one(name, text)
        if not res["ok"]:
            raise HTTPException(409, detail=erro("erro_drive_fallback_falhou", f"drive falhou e fallback por texto tambem: {_erro_texto(res['error'])}", erro=res['error']))
        _recusa_se_so_enfileirou(name, res)
        fallback = True
    # Respondido: limpa o sidecar do hook pra um stale nao reabrir o stepper depois. Resolve o jsonl
    # igual aos outros endpoints; se nao resolver, pula a limpeza sem falhar a request.
    if jsonl:
        clear_pending_askq(jsonl)
    return {"ok": True, "fallback": fallback}


@app.post("/api/sessions/{name}/model-effort", dependencies=[Depends(require_auth)])
def model_effort(name: str, body: ModelEffortBody):
    # Dirige o picker interativo do /model pra aplicar modelo/esforco SO na sessao (scope
    # 'session') ou como default ('default'). PickerError -> 409/422; entrada invalida -> 422.
    _recusa_se_painel_aberto(name)
    try:
        return terminal.set_model_effort(name, body.model, body.effort, body.scope)
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    except ValueError as e:
        raise HTTPException(422, str(e))


# ── Modo de permissão em sessão viva (Task 5) ─────────────────────────────────────────
# Leitura pelo rodapé do pane (⏸/⏵⏵) e troca via BTab (Shift+Tab). Medido em
# 2026-08-20: stdin da statusline não traz o modo, /permissions não aceita arg,
# BTab cicla 4 (plan/auto/manual/acceptEdits) ou 5 com bypassPermissions no arranque,
# dontAsk só no arranque e sai do ciclo. Ver docs/superpowers/specs/2026-08-19-medicao-permissao-viva.md
import app.permission_mode as perm_mode

class PermissionModeBody(_StrictBody):
    mode: str | None = None
    permission_mode: str | None = None

# Cache da lista viva por sessão (enquanto ela viver). Chave = "nome::jsonl" ou
# "nome::sem-jsonl" quando ainda sem transcript; valor = (current, modos).
_perm_modes_cache: dict[str, tuple[str, list[str]]] = {}

def _cache_key_perm(name: str, info) -> str:
    j = getattr(info, "jsonl", None) if info else None
    return f"{name}::{j or 'sem-jsonl'}"

def _guard_perm(name: str, info, escrita: bool) -> None:
    """409 quando sessão não é claude, painel aberto, ou estado recusa digitação."""
    if info is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if getattr(info, "provider", "claude") not in (None, "claude"):
        raise HTTPException(409, detail=erro("erro_permissao_so_claude", "modo de permissao so vale para claude"))
    _recusa_se_painel_aberto(name)
    if escrita:
        try:
            terminal._require_drivable(name)
        except terminal.NaoDigitou as e:
            raise HTTPException(e.status, e.detail)
        except PickerError as e:
            raise HTTPException(e.status, e.detail)
    else:
        from app import tmux
        from app.state import is_overlay
        if not tmux.has_session(name):
            raise HTTPException(409, "sessao nao esta viva")
        try:
            pane = tmux.capture_pane(name)
        except Exception:
            pane = ""
        if pane and is_overlay(pane):
            raise HTTPException(409, "ha um menu aberto no terminal da sessao")

@app.get("/api/sessions/{name}/permission-modes", dependencies=[Depends(require_auth)])
async def permission_modes(name: str, sondar: bool = False):
    """Lista dos modos de permissão.

    Sem sondar (default): só lê o modo atual via capture-pane (zero teclas) e devolve
    o cache de `modes` se já existir, ou [] — não sonda. Com `?sondar=1`: dá a volta
    completa de BTab, anota os modos, volta ao original e cacheia. `sondavel` diz se
    a sessão pode ser sondada (false quando current == dontAsk, que não tem volta).
    """
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    _guard_perm(name, info, escrita=sondar)
    key = _cache_key_perm(name, info)
    # leitura do atual sem tecla (bloqueador 1)
    try:
        cur_now = await asyncio.to_thread(perm_mode.ler_modo, name)
    except Exception:
        # Pane em transição e bug de parse caem no mesmo 409; sem o log os dois ficam
        # indistinguíveis pra quem for depurar.
        _log.debug("permission-modes: leitura do modo falhou em %s", name, exc_info=True)
        cur_now = None
    if cur_now is None:
        raise HTTPException(409, detail=erro("erro_permissao_leitura", "não consegui ler o modo atual no rodapé"))
    sondavel = cur_now != "dontAsk"
    if not sondar:
        # sem sondar: devolver cache se houver, ou []
        hit = _perm_modes_cache.get(key)
        if hit is not None:
            _, modos_cached = hit
            # revalida current mas mantém modos do cache
            return {"current": cur_now, "modes": modos_cached, "sondavel": sondavel}
        return {"current": cur_now, "modes": [], "sondavel": sondavel}
    # com sondar=1: comportamento de antes (listar_modos + cache)
    # se não sondável (dontAsk), não chamar listar_modos (bloqueador 2)
    if not sondavel:
        return {"current": cur_now, "modes": [], "sondavel": False}
    hit = _perm_modes_cache.get(key)
    if hit is not None:
        _, modos_cached = hit
        return {"current": cur_now, "modes": modos_cached, "sondavel": sondavel}
    try:
        cur, modos = await asyncio.to_thread(perm_mode.listar_modos, name)
    except RuntimeError as e:
        raise HTTPException(409, detail=erro("erro_permissao_leitura", str(e)))
    # Chave é nome::jsonl, então sessão nova nunca reusa entrada: sem poda o dict cresce pela
    # vida do processo. ponytail: teto burro, o cache é só pra evitar re-sondar a mesma sessão.
    if len(_perm_modes_cache) > 200:
        _perm_modes_cache.clear()
    _perm_modes_cache[key] = (cur, modos)
    # A sonda dá voltas de BTab de verdade. Se não conseguiu voltar, a sessão FICOU noutro modo de
    # permissão por causa de uma chamada que o usuário leu como leitura — isso não pode sair calado.
    restaurado = cur == cur_now
    if not restaurado:
        _log.warning("permission-modes: sonda deixou %s em %s (era %s)", name, cur, cur_now)
    return {"current": cur, "modes": modos, "sondavel": cur != "dontAsk",
            "restaurado": restaurado}

@app.post("/api/sessions/{name}/permission-mode", dependencies=[Depends(require_auth)])
async def permission_mode_set(name: str, body: PermissionModeBody):
    """Troca o modo de permissão via BTab até casar o alvo (teto 6 teclas).

    Devolve SEMPRE o modo que FICOU, nunca o pedido. Teto estourado ou alvo fora do
    ciclo → 409 com o modo que ficou. 409 também quando sessão não é claude,
    painel aberto, ou estado recusa digitação.
    """
    alvo = body.mode if body.mode is not None else body.permission_mode
    if not alvo:
        raise HTTPException(422, detail=erro("erro_permissao_invalida", "informe o modo desejado"))
    # valida contra lista fechada antes de qualquer efeito
    if alvo not in model_args.MODOS_PERMISSAO_CLAUDE:
        raise HTTPException(409, detail=erro("erro_permissao_invalida", f"permission_mode: use um de {', '.join(model_args.MODOS_PERMISSAO_CLAUDE)}"))
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    _guard_perm(name, info, escrita=True)
    try:
        ficou = await asyncio.to_thread(perm_mode.trocar_modo, name, alvo)
    except RuntimeError as e:
        raise HTTPException(409, detail=erro("erro_permissao_leitura", str(e)))
    except ValueError as e:
        raise HTTPException(409, detail=erro("erro_permissao_invalida", str(e)))
    # cache da lista pode ter ficado com current velho; atualiza o current mas mantém modos
    key = _cache_key_perm(name, info)
    hit = _perm_modes_cache.get(key)
    if hit is not None:
        _, modos_cached = hit
        _perm_modes_cache[key] = (ficou, modos_cached)
    if ficou != alvo:
        raise HTTPException(status_code=409, detail=erro("erro_permissao_teto", f"não alcançou {alvo!r} em {perm_mode.TETO_TECLAS} teclas — ficou em {ficou!r}", alvo=alvo, ficou=ficou, mode=ficou))
    return {"mode": ficou, "current": ficou}


# ── Catalogo de modelos de uma sessao Claude Code ───────────────────────────────────────────────
# Duas fontes, escolhidas pelo que a sessao E, porque medimos que so uma funciona em cada caso:
#   * sessao de MOTOR -> o /v1/models do provedor (o mesmo probe da tela de Motores). O picker do
#     Claude Code ali lista so os 4 aliases, todos apontando pro mesmo ANTHROPIC_MODEL — inutil.
#   * sessao da CONTA -> as linhas do proprio picker, lidas ao vivo. A lista muda com a conta e com
#     a versao do CC (o Fable entrou e a lista chumbada no front nao soube), entao ela nao pode
#     morar no codigo.

class EngineModelBody(_StrictBody):
    model: str
    effort: str | None = None


# Catalogo por motor: e uma chamada de REDE ao provedor, e abrir a folha nao pode pagar isso toda
# vez. TTL curto porque a lista muda com o plano do usuario, nao a cada minuto.
_ENGINE_MODELS_TTL = 300.0
_engine_models_cache: dict[str, tuple[float, list[dict]]] = {}

# Catalogo da conta Anthropic: ler o picker DIRIGE O TERMINAL, e isso deixa RASTRO — o `❯ /model` e
# o `⎿ Kept model as …` (o Esc de saida) ficam no scrollback do tmux pra sempre. Nao aparece no chat
# do app (entra no jsonl como `type: system`, que o transcript ignora), mas aparece pra quem estiver
# com aquele terminal aberto: foi o que pareceu bug quando 5 leituras seguidas empilharam ali.
# Sete dias, porque a lista muda quando a Anthropic lanca modelo ou o plano do usuario muda —
# eventos de semanas, nao de horas. Uma hora (o valor antigo) fazia o `/model` reaparecer no
# terminal do usuario "sozinho" no meio de sessoes longas, e cada restart do backend zerava o
# cache em memoria e relia tudo de novo — dai o espelho em DISCO, dentro do proprio config dir
# (`.hangar-models.json`): a leitura dirigida do picker vira acontecimento raro.
# A chave e o config dir, nao a sessao: a lista vem da CONTA, e a mesma pra todas as sessoes dela.
_CLAUDE_MODELS_TTL = 7 * 24 * 3600.0
_claude_models_cache: dict[str, tuple[float, dict]] = {}


def _models_cache_path(chave: str) -> Path:
    return Path(chave) / ".hangar-models.json"


def _models_cache_get(chave: str) -> dict | None:
    hit = _claude_models_cache.get(chave)
    if hit and time.monotonic() - hit[0] < _CLAUDE_MODELS_TTL:
        return hit[1]
    try:
        bruto = json.loads(migracao_sidecars.caminho_de_leitura(_models_cache_path(chave)).read_text(encoding="utf-8"))
        resp = bruto["resp"]
        if not isinstance(resp, dict) or time.time() - float(bruto["ts"]) >= _CLAUDE_MODELS_TTL:
            return None
    except (OSError, ValueError, KeyError, TypeError):
        return None
    # Promove pra memoria DESCONTANDO a idade que o registro ja tem no disco — carimbar com o
    # monotonic de agora zerava o relogio e um dado de 6d23h passava a valer mais 7 dias.
    idade = time.time() - float(bruto["ts"])
    _claude_models_cache[chave] = (time.monotonic() - idade, resp)
    return resp


def _models_cache_put(chave: str, resp: dict) -> None:
    _claude_models_cache[chave] = (time.monotonic(), resp)
    alvo = _models_cache_path(chave)
    tmp = alvo.with_name(f"{alvo.name}.{os.getpid()}.tmp")
    try:
        tmp.write_text(json.dumps({"ts": time.time(), "resp": resp}), encoding="utf-8")
        atomico.substituir(tmp, alvo)
    except OSError:
        # Config dir somente-leitura ou inexistente: o cache em memoria segue valendo.
        tmp.unlink(missing_ok=True)


def _chave_config(p) -> str:
    """Chave única do cache de modelos da conta. As duas rotas TÊM que passar por aqui: a da sessão
    viva deriva do /proc (vazio = "~") e a da abertura recebe caminho do cliente."""
    s = str(p or "").strip()
    if not s or s == "~":
        return str(Path.home() / ".claude")
    return str(Path(s).expanduser().resolve())


async def _engine_models(nome: str, fresco: bool = False) -> list[dict]:
    """Catalogo do provedor. `fresco=True` ignora o cache.

    Quem VALIDA uma troca pede fresco: o cache existe pra folha abrir rapido, mas a promessa do
    check ("recusa aqui em vez de deixar a falha aparecer so no proximo turno") nao sobrevive a 5
    minutos de lista velha — modelo tirado do plano passaria pela validacao e falharia depois. Uma
    chamada de rede numa acao deliberada do usuario e barata; num tick de tela, nao.
    """
    hit = _engine_models_cache.get(nome)
    if hit and not fresco and time.monotonic() - hit[0] < _ENGINE_MODELS_TTL:
        return hit[1]
    cfg = engines.listar().get(nome)
    if not cfg:
        raise HTTPException(409, detail=erro("erro_motor_ausente", f"motor {nome!r} nao esta mais no engines.json", nome=nome))
    try:
        modelos = await asyncio.to_thread(engine_probe.listar_modelos, cfg["base_url"], cfg["api_key"])
    except RuntimeError as e:
        # A mensagem do provedor E a informacao util (key invalida, host fora do ar).
        raise HTTPException(502, detail=erro("erro_provedor_offline", f"o provedor do motor {nome!r} nao respondeu: {e}", nome=nome, erro=str(e)))
    _engine_models_cache[nome] = (time.monotonic(), modelos)
    return modelos


@app.get("/api/sessions/{name}/model/options", dependencies=[Depends(require_auth)])
async def model_options(name: str):
    """Modelos que ESTA sessao pode escolher. `kind` diz de onde vieram e como aplicar."""
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if info.provider not in (None, "claude"):
        raise HTTPException(400, detail=erro("erro_rota_so_claude", "esta rota so existe pra sessoes Claude Code"))
    if info.engine:
        # Motor: catalogo vem do /v1/models do provedor (HTTP), nao do pane -- nao conta linha,
        # nao depende do tamanho da janela. A guarda so vale pro ramo abaixo (le o picker).
        modelos = await _engine_models(info.engine)
        return {"kind": "engine", "engine": info.engine,
                "models": [{"id": m["id"], "context_length": m.get("context_length"),
                            "vision": m.get("vision")} for m in modelos]}
    # Conta Anthropic: le o picker de verdade. Abre e fecha um overlay — nao vai pro scrollback,
    # nao entra no transcript e nao gasta token.
    _recusa_se_painel_aberto(name)
    chave = _chave_config(_session_config_dir(name))
    cacheado = _models_cache_get(chave)
    if cacheado is not None:
        return cacheado
    try:
        lido = await asyncio.to_thread(terminal.list_model_options, name)
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    resp = {"kind": "claude", "engine": None, "effort": lido["effort"],
            # `id` (único por linha), não `keyword`: duas linhas do picker compartilham a keyword
            # `opus` ("Opus" e "Opus (1M context)"), e id repetido derrubava a lista na tela.
            "models": [{"id": r["id"], "name": r["name"], "desc": r["desc"],
                        "active": r["active"]} for r in lido["models"]]}
    _models_cache_put(chave, resp)
    return resp


@app.get("/api/model-options", dependencies=[Depends(require_auth)])
async def model_options_sem_sessao(provider: str = "claude", engine: str = "", config_dir: str = ""):
    """Modelos oferecidos na tela de ABERTURA, onde ainda não existe sessão.

    Irmã de /api/sessions/{name}/model/options, que não serve aqui: no ramo da conta Anthropic
    aquela LÊ O PICKER dirigindo o terminal de uma sessão viva. Sem sessão, o melhor que existe é o
    cache por config dir que aquela rota já alimentou — e, frio, os aliases mínimos, ditos como
    reduzidos em vez de fingirem ser a lista completa (ver o comentário acima sobre a lista
    chumbada que não soube do Fable).
    """
    if provider == "pi":
        try:
            return {"kind": "pi", "reduced": False,
                    "models": await asyncio.to_thread(pi_catalog.listar)}
        except pi_catalog.PiAusente as e:
            # Codigo proprio: "nao achei o pi" nao e "o pi falhou". Antes isso chegava como
            # `[WinError 2] O sistema nao pode encontrar o arquivo especificado` dentro da mensagem
            # de falha do comando — a pessoa ia procurar defeito no `pi --list-models` de um pi que
            # nem estava instalado ali.
            raise HTTPException(502, detail=erro("erro_pi_ausente", str(e), erro=str(e)))
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            raise HTTPException(502, detail=erro("erro_pi_list_models", f"pi --list-models falhou: {e}", erro=str(e)))
    if provider == "kimi":
        # Sem subprocess aqui (não existe `kimi --list-models`): o catálogo é o config.toml.
        cat = kimi_models.read_catalog()
        if cat is None:
            raise HTTPException(409, detail=erro("erro_catalogo_kimi_indisponivel",
                                                 "catalogo do Kimi indisponível — ~/.kimi-code/config.toml "
                                                 "ausente ou sem seções [models.*]"))
        return {"kind": "kimi", "reduced": False, "models": cat["models"], "default": cat["default"]}
    if provider == "codex":
        # Nem config no disco (o ~/.codex/config.toml guarda o modelo escolhido, nunca a lista) nem
        # `codex --list-models`: a fonte e o `model/list` de um app-server efemero em stdio, a MESMA
        # que a folha da sessao viva usa. Ver app/codex_models.py.
        try:
            return {"kind": "codex", "reduced": False,
                    "models": await asyncio.to_thread(codex_models.listar)}
        except codex_models.CodexAusente as e:
            # Codigo proprio pelo mesmo motivo do Pi: "nao achei o codex" nao e "o codex falhou".
            raise HTTPException(502, detail=erro("erro_codex_ausente", str(e), erro=str(e)))
        except (RuntimeError, OSError) as e:
            # Sem `TimeoutExpired` aqui, ao contrario do ramo do Pi: o teto de tempo do
            # `codex_models` mata o processo por um Timer, entao ele vira "nao respondeu" (um
            # RuntimeError) — capturar a outra seria um ramo que o codigo nunca produz.
            raise HTTPException(502, detail=erro("erro_codex_model_list", f"codex app-server model/list falhou: {e}", erro=str(e)))
    if provider != "claude":
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider deve ser 'claude', 'pi', 'kimi' ou 'codex'"))
    if engine:
        modelos = await _engine_models(engine)
        return {"kind": "engine", "reduced": False,
                "models": [{"id": m["id"], "context_length": m.get("context_length"),
                            "vision": m.get("vision")} for m in modelos]}
    cacheado = _models_cache_get(_chave_config(config_dir))
    if cacheado is not None:
        return {**cacheado, "reduced": False}
    return {"kind": "claude", "reduced": True,
            "models": [{"id": a} for a in ("opus", "sonnet", "haiku")]}


@app.post("/api/sessions/{name}/engine/model", dependencies=[Depends(require_auth)])
async def engine_model_set(name: str, body: EngineModelBody):
    """Troca o modelo (e opcionalmente o esforco) de uma sessao que roda num motor.

    O `/model <id>` do Claude Code aplica na sessao E grava o id como default GLOBAL pra sessoes
    novas — inclusive as da conta Anthropic, que nao conhecem esse id. Por isso o valor anterior do
    settings.json e capturado antes e reposto depois: a troca vale onde foi pedida e em lugar nenhum
    mais. Ver app/default_model.py.
    """
    _recusa_se_painel_aberto(name)
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao nao encontrada"))
    if not info.engine:
        raise HTTPException(400, detail=erro("erro_rota_so_motor", "esta rota so existe pra sessoes que rodam num motor"))
    # fresco=True: a validacao promete "recusa aqui em vez de deixar a falha aparecer so no proximo
    # turno", e essa promessa nao sobrevive ao cache de 5 min (ver _engine_models).
    modelos = await _engine_models(info.engine, fresco=True)
    if not any(m["id"] == body.model for m in modelos):
        # Recusar aqui em vez de digitar: o CC aceitaria o id, a sessao passaria a mandar request
        # pra um modelo que o provedor nao tem, e a falha apareceria so no proximo turno.
        raise HTTPException(422, detail=erro("erro_modelo_fora_catalogo", f"modelo fora do catalogo do motor {info.engine!r}: {body.model}", motor=info.engine, modelo=body.model))

    cfg_dir = _session_config_dir(name)  # mesma leitura de /proc que resolve o config dir das outras rotas
    antes = await asyncio.to_thread(default_model.snapshot, cfg_dir)
    digitou = True
    try:
        res = await asyncio.to_thread(terminal.set_engine_model, name, body.model)
    except TerminalInput.NaoDigitou as e:
        # Recusado antes de qualquer tecla (sessao ocupada/morta, menu aberto): o settings.json esta
        # intocado, entao esperar a escrita aterrissar so faria o erro demorar ~3.6s a aparecer.
        digitou = False
        raise HTTPException(e.status, e.detail)
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    except ValueError as e:
        raise HTTPException(422, str(e))
    finally:
        # No finally de proposito: se o comando foi digitado mas a confirmacao nao pode ser lida, o
        # settings.json PODE ja ter sido reescrito — deixar o default global vazado por causa de um
        # erro de leitura seria a pior combinacao.
        if digitou:
            await asyncio.to_thread(default_model.restore_quando_aterrissar, cfg_dir, antes)

    if body.effort:
        # Esforco continua saindo do picker (Left/Right): medido que ele funciona igual em sessao de
        # motor — o chip `(high✦)` e real, nao maquiagem. Falha aqui nao desfaz o modelo, que ja
        # pegou; reporta junto pra tela nao dizer que tudo deu certo.
        try:
            await asyncio.to_thread(terminal.set_model_effort, name, None, body.effort, "session")
        except (PickerError, ValueError) as e:
            return {**res, "model": body.model, "effort_error": str(e)}
    return {**res, "model": body.model}


# ── Modelo + raciocinio de uma sessao Pi ────────────────────────────────────────────────────────
# Rotas separadas das do Claude (/model-effort, picker do TUI) e das do Codex (/models, app-server)
# porque o mecanismo e um terceiro: a extensao hangar-state.ts publica o catalogo num sidecar e expoe
# dois comandos que aplicam a troca pela API do Pi. Ver app/pi_models.py pro porque de nao raspar
# o TUI aqui.

class PiModelBody(_StrictBody):
    provider: str | None = None
    model: str | None = None
    effort: str | None = None


def _session_config_dir(name: str) -> Path | None:
    """CLAUDE_CONFIG_DIR do processo do pane (o sidecar do Pi e o settings.json moram la dentro).
    None -> ~/.claude.
    Usa o mesmo `_config_dir_of` do registry (privado do pacote) que ja resolve o transcript do Pi:
    duas leituras diferentes do /proc dariam respostas diferentes pra mesma sessao."""
    from app import registry as registry_mod
    from app import tmux
    try:
        pid = tmux.pane_pid(name)
        return registry_mod._config_dir_of(pid) if pid else None
    except Exception:
        # Cair no ~/.claude CALADO transformava um bug de resolucao (pane sem pid, /proc ilegivel)
        # num 409 mentiroso "extensao desatualizada": o sidecar existe, so estavamos procurando na
        # pasta errada. Nao propaga — o default ainda e o certo pra maioria das sessoes.
        _log.warning("pi: nao consegui resolver o config dir de %s; usando ~/.claude", name,
                     exc_info=True)
        return None


def _session_config_dir_strict(name: str) -> tuple[Path | None, bool]:
    """CLAUDE_CONFIG_DIR da sessão pro DELETE de conta: (Path | None, confiável).

    A irmã acima (fallback silencioso pro ~/.claude) é certa pra LEITURA e perigosa numa operação
    DESTRUTIVA: falha de resolução virava None, None não casa com o alvo, e o apagar seguia como
    se a sessão usasse a conta padrão. Aqui falha devolve confiável=False e quem chama recusa —
    na dúvida, não apaga. None + True = processo vivo SEM a var no ambiente: usa a conta padrão,
    não a que está sendo apagada.
    """
    from app import tmux
    try:
        pid = tmux.pane_pid(name)
    except Exception:
        return None, False
    if not pid:
        return None, True   # sem processo vivo: ninguém está usando nada
    try:
        with open(procinfo._proc_environ_path(pid), "rb") as fh:
            env = fh.read()
    except OSError:
        return None, False
    for kv in env.split(b"\x00"):
        if kv.startswith(b"CLAUDE_CONFIG_DIR="):
            return (Path(kv.split(b"=", 1)[1].decode("utf-8", "surrogateescape")), True)
    return None, True


async def _pi_catalog(name: str) -> tuple[dict, str]:
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessao ou transcript nao encontrado"))
    if info.provider != "pi":
        raise HTTPException(400, detail=erro("erro_rota_so_pi", "esta rota so existe pra sessoes Pi"))
    cat = await asyncio.to_thread(pi_models.read_catalog, info.jsonl, _session_config_dir(name))
    if cat is None:
        # Falha ALTA: sem o sidecar nao ha catalogo real, e inventar um faria o app oferecer
        # modelos que o `/cp-model` nao encontraria. Instrucao junto porque a causa e sempre a
        # mesma (extensao velha/ausente) e o conserto e um comando.
        raise HTTPException(409, detail=erro("erro_catalogo_pi_indisponivel",
                                             "catalogo do Pi indisponivel — rode ./scripts/install-claude-wrapper.sh "
                                             "e reinicie a sessao (extensao hangar-state.ts desatualizada)"))
    return cat, info.jsonl


@app.get("/api/sessions/{name}/pi/models", dependencies=[Depends(require_auth)])
async def pi_models_list(name: str):
    cat, _ = await _pi_catalog(name)
    return {"models": cat.get("models", []), "current": cat.get("current"),
            "thinking": cat.get("thinking"), "levels": cat.get("levels", [])}


@app.post("/api/sessions/{name}/pi/model", dependencies=[Depends(require_auth)])
async def pi_model_set(name: str, body: PiModelBody):
    cat, jsonl = await _pi_catalog(name)
    cmds: list[str] = []
    try:
        if body.model:
            if not body.provider:
                raise pi_models.PiModelError(422, "provider obrigatorio junto com model")
            pi_models.check_known(cat, body.provider, body.model)
            cmds.append(pi_models.model_command(body.provider, body.model))
        if body.effort:
            cmds.append(pi_models.think_command(body.effort))
    except pi_models.PiModelError as e:
        raise HTTPException(e.status, e.detail)
    if not cmds:
        raise HTTPException(422, detail=erro("erro_model_effort_faltando", "informe model (com provider) e/ou effort"))
    try:
        await asyncio.to_thread(terminal.send_pi_commands, name, cmds)
    except terminal_input.DriveError as e:
        raise HTTPException(409, str(e))
    # Re-le o sidecar ATE ele confirmar (ou estourar 2s): o Pi CLAMPA o nivel pro que o modelo
    # suporta, entao o que voltamos e o que FICOU, nao o que foi pedido — e o `/cp-model` pode
    # RECUSAR sem levantar nada (sem chave pro provedor: notifica no TUI e o sidecar segue no modelo
    # velho). Devolver ok=True sem comparar era declarar sucesso sobre um no-op, com a folha
    # fechando calada.
    after = await asyncio.to_thread(pi_models.read_back, jsonl, _session_config_dir(name),
                                    body.provider, body.model, body.effort)
    if after is not None and pi_models.confirms(after, body.provider, body.model, body.effort):
        return {"ok": True, "current": after.get("current"), "thinking": after.get("thinking"),
                "levels": after.get("levels", [])}
    # Nao confirmou. As duas causas pedem acoes diferentes do usuario, entao nao viram a mesma frase:
    # sidecar ilegivel ou parado no MESMO `ts` = o Pi nem republicou o catalogo (comando pode nao ter
    # chegado) -> indeterminado; `ts` novo com o modelo velho = o Pi processou e RECUSOU.
    if after is None or after.get("ts") == cat.get("ts"):
        raise HTTPException(409, detail=erro("erro_sem_confirmacao_troca",
                                             "comandos digitados, mas o Pi nao republicou o catalogo — nao da "
                                             "pra confirmar a troca; veja o modelo no proprio terminal"))
    cur = after.get("current") or {}
    raise HTTPException(409, detail=erro("erro_pi_recusou_troca",
                                             f"o Pi recusou a troca — segue em "
                                             f"{cur.get('provider')}/{cur.get('id')} (raciocinio "
                                             f"{after.get('thinking')}). Causa mais comum: sem chave configurada "
                                             f"pro provedor pedido (o Pi avisa dentro do TUI)",
                                             provider=cur.get("provider"), id=cur.get("id"),
                                             thinking=after.get("thinking")))


# ── Modelo de uma sessão Kimi ─────────────────────────────────────────────────────────────────
# Quarto mecanismo, diferente dos três vizinhos: sem picker legível (Claude), sem extensão com
# sidecar (Pi), sem app-server (Codex). O catálogo mora no ~/.kimi-code/config.toml e a troca
# dirige a busca do picker + Alt+S, confirmada pela linha "Switched to …" do scrollback — ver
# app/kimi_models.py pro que foi medido na TUI.

class KimiModelBody(_StrictBody):
    model: str | None = None
    effort: str | None = None


def _kimi_catalog() -> dict:
    cat = kimi_models.read_catalog()
    if cat is None:
        # Mesma política do _pi_catalog: falha ALTA com instrução, nunca lista inventada.
        raise HTTPException(409, detail=erro("erro_catalogo_kimi_indisponivel",
                                             "catalogo do Kimi indisponível — ~/.kimi-code/config.toml "
                                             "ausente ou sem seções [models.*]"))
    return cat


async def _kimi_info(name: str):
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if info.provider != "kimi":
        raise HTTPException(400, detail=erro("erro_rota_so_kimi", "esta rota só existe pra sessões Kimi"))
    return info


@app.get("/api/sessions/{name}/kimi/models", dependencies=[Depends(require_auth)])
async def kimi_models_list(name: str):
    await _kimi_info(name)
    cat = _kimi_catalog()
    # "current" ao vivo não tem fonte barata (a TUI não expõe e o marcador da statusline é display
    # name, que repete entre providers): quem mostra o atual é a pill do composer, que já lê a
    # statusline. Aqui vai o default do config como referência da abertura.
    return {"models": cat["models"], "default": cat["default"]}


@app.post("/api/sessions/{name}/kimi/model", dependencies=[Depends(require_auth)])
async def kimi_model_set(name: str, body: KimiModelBody):
    info = await _kimi_info(name)
    _recusa_se_painel_aberto(name)
    # Sessão TRABALHANDO: o `/model` digitado cairia no composer e o Enter o enfileiraria como
    # MENSAGEM — a troca viraria um "/model" pro modelo ler. No Claude o _require_drivable cobre
    # isso pelo spinner; o do Kimi são fases de lua, fora do que ele detecta, então a guarda é o
    # marcador do hook (corrigido: pode ser o idle CONGELADO do turno anterior).
    if info.jsonl:
        m = hook_state.get_state(session_key(info.jsonl))
        if m:
            m = corrige_ocioso_kimi(m, info.jsonl)
        if m and m[0] == "working":
            raise HTTPException(409, detail=erro("erro_sessao_trabalhando",
                                                 "a sessão está trabalhando — espere ela terminar"))
    cat = _kimi_catalog()
    try:
        alvo = kimi_models.check_known(cat, body.model) if body.model else None
        nivel = None
        if body.effort:
            # Com modelo junto, valida contra o support_efforts DELE. Sozinho, quem valida é o
            # picker ao vivo (a linha Thinking mostra os níveis do modelo ATUAL — o backend não
            # sabe o alias vigente sem perguntar à TUI).
            nivel = (kimi_models.check_effort(alvo, body.effort) if alvo
                     else kimi_models.clean_alias(body.effort).lower())
    except kimi_models.KimiModelError as e:
        raise HTTPException(e.status, e.detail)
    if alvo is None and nivel is None:
        raise HTTPException(422, detail=erro("erro_model_effort_faltando",
                                             "informe model e/ou effort"))
    try:
        res = await asyncio.to_thread(terminal.set_kimi_model, name,
                                      alvo and alvo["alias"], alvo and alvo["name"], nivel)
    except terminal_input.DriveError as e:
        raise HTTPException(409, str(e))
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    return {"ok": True,
            "current": {"alias": alvo["alias"], "name": alvo["name"]} if alvo else None,
            "effort": nivel, "result": res.get("result")}


@app.get("/api/fs/roots", dependencies=[Depends(require_auth)])
def fs_roots():
    return list_roots()


@app.get("/api/fs/scan", dependencies=[Depends(require_auth)])
def fs_scan(root: str, path: str | None = None):
    # A seguranca (allowlist + rejeicao de escape) vive em scan_dir; aqui so traduzimos
    # a FsError pro status HTTP correspondente.
    try:
        return scan_dir(root, path)
    except FsError as e:
        raise HTTPException(e.status, e.detail)


# ── Preview: expoe um projeto local (porta) via tailscale serve, pro app ver num iframe ──
# GLOBAL por maquina (nao por sessao): o tunel usa uma porta-slot unica (10000), entao qualquer
# sessao que ligar o preview compartilha o mesmo slot. O backend que atende E o da maquina onde o
# projeto roda -> o preview sai da maquina certa sem config extra.
class PreviewBody(_StrictBody):
    port: int = Field(ge=1, le=65535)


@app.get("/api/preview", dependencies=[Depends(require_auth)])
def preview_status():
    try:
        return tunnel.status()
    except tunnel.TunnelError as e:
        raise HTTPException(e.status, e.detail)


@app.post("/api/preview", dependencies=[Depends(require_auth)])
def preview_start(body: PreviewBody):
    try:
        return tunnel.start(body.port)
    except tunnel.TunnelError as e:
        raise HTTPException(e.status, e.detail)


@app.delete("/api/preview", dependencies=[Depends(require_auth)])
def preview_stop():
    try:
        return tunnel.stop()
    except tunnel.TunnelError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/sessions/{name}/commands", dependencies=[Depends(require_auth)])
def commands(name: str):
    # cwd vem do registry/tmux; se a sessao nao for achada, ainda devolvemos os built-ins
    # + skills globais (lista util mesmo sem cwd casado).
    #
    # `tmux.cwd_de` antes do `registry.list()`: esta rota so quer o cwd de UMA sessao, e a varredura
    # completa cobra tmux de todas as sessoes + /proc + `git` por sessao pra devolver tudo o mais.
    # Medido em 28/08/2026, ela era o companheiro mais caro da abertura de sessao — sozinha levava
    # o `/history` de 0,29s pra 0,53s. A varredura fica como plano B pros dois casos em que ela sabe
    # mais: a sessao Codex (vive num sidecar duravel, pode nao ter pane nenhum) e a sessao com 2+
    # panes (ali quem escolhe o cwd e o `_agent_pane`, que acha o pane do AGENTE — ver `cwd_de`).
    cwd = tmux.cwd_de(name)
    if cwd is None:
        cwd = next((s.cwd for s in registry.list() if s.name == name), None)
    if cwd is None:
        # Nem o tmux nem a varredura acharam a sessao. A lista SAI MESMO ASSIM (built-ins + skills
        # globais), que e util e e o comportamento de sempre — mas nao pode sair calada: sem cwd
        # faltam as skills e comandos DO PROJETO, e do lado de fora isso e indistinguivel de um
        # projeto que nao tem nenhuma.
        _log.warning("commands: sem cwd pra '%s' (tmux e registry nao acharam) — lista sem o que "
                     "e do projeto", name)
    return list_commands(cwd)


_TTS_LIMITE_PADRAO = 5000
# Teto DURO, que nenhuma confirmacao passa — derivado do MODELO EM USO (tts.TETO_CARACTERES), nao
# um numero solto aqui: um teto maior que o do modelo deixaria confirmar um gasto que a ElevenLabs
# ainda ia recusar. Dois numeros diferentes de proposito: o limite configuravel e um AVISO de
# custo (o usuario confirma e passa); este e o que impede um cliente autenticado de mandar
# megabytes numa requisicao.
_TTS_TETO = tts.TETO_CARACTERES


def _tts_limite() -> int:
    """Limite de AVISO em caracteres. Configuravel; 0/ausente cai no padrao."""
    try:
        v = int(runtime_config.get("tts_max_chars") or 0)
    except (TypeError, ValueError):
        v = 0
    return v if v > 0 else _TTS_LIMITE_PADRAO


@app.post("/api/tts", dependencies=[Depends(require_auth)])
async def tts_sintetizar(body: TtsBody):
    texto = tts_preparar(body.text)
    if not texto:
        raise HTTPException(400, detail=erro("erro_tts_sem_texto", "nao sobrou nada pra falar depois de limpar o texto"))
    if len(texto) > _TTS_TETO:
        raise HTTPException(413, detail=erro("erro_tts_teto", f"texto com {len(texto)} caracteres passa do teto de {_TTS_TETO} — selecione um trecho menor", n=len(texto), teto=_TTS_TETO))
    limite = _tts_limite()
    if len(texto) > limite and not body.confirm:
        # 409, nao 413: nao e "grande demais", e "confirme que voce quer gastar isso". O front pede
        # a confirmacao e repete o POST com confirm=true. Checado AQUI e nao so na tela porque a
        # tela evita o susto e o servidor e quem guarda a conta.
        raise HTTPException(409, detail=erro("erro_tts_limite", f"são {len(texto)} caracteres, acima do limite de {limite} — confirme para gerar", n=len(texto), limite=limite))
    try:
        h, veio_do_cache, provedor_final = await asyncio.to_thread(
            tts.sintetizar, texto, body.voice, body.provider, body.instruction)
    except tts.TtsError as e:
        raise HTTPException(e.status, e.detail)
    # provider ecoa o que RESPONDEU de fato (pode ter virado "local" pelo fallback sem chave) — o
    # front usa isso pra avisar na barra, em vez de trocar de voz caladamente.
    return {"url": f"/api/tts/audio/{h}", "chars": len(texto), "cached": veio_do_cache, "provider": provedor_final}


@app.post("/api/tts/narrar", dependencies=[Depends(require_auth)])
async def tts_narrar(body: NarrarBody):
    """Fase 2 (narracao guiada): trata o texto falavel de uma selecao pela Groq ANTES de virar
    audio — o resultado volta pro front pra REVISAO (o usuario confere antes de gastar credito da
    ElevenLabs), nao sintetiza nada aqui."""
    try:
        texto_tratado = await asyncio.to_thread(narrar.narrar, body.text, body.code_blocks, body.instruction)
    except narrar.NarrarError as e:
        raise HTTPException(e.status, e.detail)
    usou_groq = not narrar.eh_instrucao_padrao(body.instruction)
    chars_sent = (len(body.text) + sum(len(b) for b in body.code_blocks) + len(body.instruction)) if usou_groq else 0
    return {"text": texto_tratado, "chars_sent": chars_sent, "used_groq": usou_groq}


@app.get("/api/tts/voices", dependencies=[Depends(require_auth)])
async def tts_vozes():
    try:
        return {"voices": await asyncio.to_thread(tts.listar_vozes)}
    except tts.TtsError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/tts/saldo", dependencies=[Depends(require_auth)])
async def tts_saldo():
    try:
        return await asyncio.to_thread(tts.saldo)
    except tts.TtsError as e:
        raise HTTPException(e.status, e.detail)


@app.get("/api/tts/audio/{h}", dependencies=[Depends(require_auth)])
async def tts_audio(h: str):
    # Hash validado ANTES de tocar no disco: o parametro vem da URL e sem isto viraria path
    # traversal. Mesmo espirito do guard de resolve_upload.
    if not re.fullmatch(r"[0-9a-f]{64}", h):
        raise HTTPException(400, detail=erro("erro_tts_audio_invalido", "identificador de audio invalido"))
    caminho = tts.caminho_do_cache(h)
    if not caminho.exists():
        raise HTTPException(404, detail=erro("erro_tts_sem_cache", "audio nao esta mais em cache"))
    # Extensao real do arquivo em cache, nao suposicao: o motor local pode ter devolvido WAV
    # (ver tts.extensao_de) — servir isso como audio/mpeg quebra o <audio> no WebKit.
    media_type = "audio/wav" if caminho.suffix == ".wav" else "audio/mpeg"
    return FileResponse(caminho, media_type=media_type)


# ── Interface (dist do frontend) ────────────────────────────────────────────────────────────────
# POR ÚLTIMO, depois de TODAS as rotas: o mount na raiz casa qualquer caminho, então registrado
# antes engoliria /api. Serve o build do Vite — arquivos estáticos comuns; o Vite em si não roda
# aqui e não precisa. Com isto o 8765 entrega tela E API num endereço só, e o servidor de
# desenvolvimento deixa de ser infraestrutura: vira ferramenta de quem mexe no layout.
#
# Medido em 05/08/2026: o `tailscale serve` publicava só o 5173, então parar o front (um `npm run
# dev`, que nem servia a tela usada — ela vem da VPS) derrubava a API do celular junto, com 502 em
# /api/sessions/events e o backend vivo o tempo todo.
#
# Ausente = instalação com --no-frontend, ou repo sem build. Sobe igual, só não serve tela.
class _UIStatic(StaticFiles):
    """`index.html` sempre revalida; o resto (nome com hash) segue cacheável à vontade.

    O `StaticFiles` manda só `etag`/`last-modified`, sem `cache-control` — e sem essa diretiva o
    navegador aplica FRESCOR HEURÍSTICO: serve o `index.html` que ele guardou sem nem perguntar ao
    servidor. Como o nome dos bundles tem hash, a página velha continua pedindo o CSS/JS velho: o
    build novo está no disco, servido corretamente, e a tela não muda. Fica parecendo bug de CSS.

    Medido em 10/08/2026: uma aba NOVA em 127.0.0.1:8765 carregou `index-DYyp82gq.css` enquanto
    `curl /` na mesma máquina entregava `index-CDPetMR_.css`; só `reloadIgnoringCache` consertou.
    Custou uma investigação inteira de "costura vertical na sidebar" que já estava consertada no
    código. A janela do Electron carrega deste mesmo endereço, então ela sofria igual.

    `no-cache` NÃO é `no-store`: o arquivo continua guardado, só volta a perguntar antes de usar —
    e com o ETag a resposta vira um 304 de algumas dezenas de bytes. Os assets ficam de fora de
    propósito; o hash no nome já os torna imutáveis, e revalidar cada um seria pagar ida e volta
    por arquivo sem ganhar nada.

    Decide pelo CAMINHO, não pelo `content-type` da resposta pronta: quando o pedido chega com
    `If-None-Match` batendo, o starlette devolve um `NotModifiedResponse`, que copia só uma lista
    fixa de headers e NÃO inclui o `content-type` — olhar o header ali deixaria justamente a
    resposta de revalidação sem diretiva nenhuma. Navegador nenhum regride por isso (o 304 mescla
    com o que ele já guardou), mas um proxy na frente do backend, sim — e tem um: a tela do celular
    passa por Traefik.
    """

    def file_response(self, full_path, *args, **kwargs) -> Response:  # type: ignore[no-untyped-def]
        resp = super().file_response(full_path, *args, **kwargs)
        if str(full_path).endswith(".html"):
            resp.headers["cache-control"] = "no-cache"
        return resp


_DIST = Path(__file__).resolve().parents[2] / "frontend" / "dist"
if _DIST.is_dir():
    app.mount("/", _UIStatic(directory=_DIST, html=True), name="ui")
