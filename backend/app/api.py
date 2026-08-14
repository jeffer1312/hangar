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
from concurrent.futures import ThreadPoolExecutor
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Literal, Optional
from fastapi import FastAPI, Depends, HTTPException, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import FileResponse, Response
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, ConfigDict, Field
from sse_starlette.sse import EventSourceResponse
from app.auth import require_auth, require_loopback
from app.commands import list_commands
from app.fs import FsError, list_roots, scan_dir
from app.model_picker import PickerError
from app.mensagens import erro
from app import model_args
from app import pi_catalog
from app import pi_models
from app.pi_inbox import INBOX
from app.registry import KillFailed, SessionRegistry, sanitize_cwd
from app.names import sanitize_session_name
from app.models import (SessionInfo, ChatEvent, CostReport, RunnersResponse, RunBody, RunInfo,
                        ProjectStatus, session_key)
from app.planprog import plan_progress, list_plans, write_pin, is_safe_stem, _plans_dir, PlanPinError, PIN_NONE
from app.pqueue import PromptQueue, _transcript_start_ts, committed_user_lines
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
from app.archive import ArchiveEntry, ArchiveFolder, archive_cwd, archive_jsonl, list_conversations, list_folders
from app.search import SearchHit, search, extract_terms, search_terms, build_ask_prompt
from app.askquestion import clear_pending_askq, read_pending_askq
from app import pair
from app import peers
from app.pair import PairLink, contract_path_for
from app.hook_state import hook_state
from app import push
from app import stall_watch
from app.sync import sync_router
from app.deploy import deploy_router
from app import desktop_palette

_log = logging.getLogger("claude_pocket")


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
    try:
        yield
    finally:
        task.cancel()
        stall_task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        try:
            await stall_task
        except asyncio.CancelledError:
            pass


app = FastAPI(title="claude-pocket", lifespan=_lifespan)
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
registry = SessionRegistry()
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
# recuo, cp-state.ts) virar linha de log — e a mesma enxurrada que o retry em si tenta evitar do
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
        pane = str(primeira.get("pane") or "")
        if not pane:
            await ws.close(code=1008)
            return
        linha = INBOX.registrar(pane, ws.send_json)
        _log.info("pi_inbox: linha aberta pane=%s", pane)
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
            if msg_id:
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
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None:
        raise HTTPException(status_code=404, detail="sessao nao existe")
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
                            detail=f"ja existe uma sessao tmux chamada {alvo!r} sem a marca do "
                                   "painel -- pode ser uma sessao sua de mesmo nome, ou o shell "
                                   "deste painel que perdeu a marca. Encerre ou renomeie essa "
                                   "sessao antes de abrir o shell")
    # O cwd vem do REGISTRY, nunca da query: um `?cwd=/` viraria shell em qualquer lugar do disco.
    novo = tmux.new_hidden_shell(name, info.cwd or str(Path.home()))
    if novo is None:
        raise HTTPException(status_code=500, detail="tmux recusou criar o shell")
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
        raise HTTPException(status_code=404, detail="sessao nao existe")
    nome_bin = os.environ.get("CP_TERMINAL")
    if nome_bin:
        # env checada ANTES do PATH: se o usuario apontou um emulador, e ele que vale -- so falha
        # se esse binario especifico nao existir ou nao for suportado (dicionario fechado; NAO
        # inventa um `-e` generico pra emulador desconhecido).
        if nome_bin not in _EMULADORES or shutil.which(nome_bin) is None:
            raise HTTPException(status_code=503,
                                detail=f"CP_TERMINAL={nome_bin!r} nao encontrado no PATH ou nao "
                                       "suportado")
    else:
        nome_bin = next((n for n in _ORDEM_PROBE if shutil.which(n)), None)
        if nome_bin is None:
            raise HTTPException(status_code=503,
                                detail="nenhum emulador de terminal encontrado no PATH")
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
        raise HTTPException(status_code=503, detail=f"falha ao abrir o emulador de terminal: {e}")
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
        erro = err_file.read().decode(errors="replace").strip()
        err_file.close()
        raise HTTPException(status_code=503,
                            detail=f"emulador de terminal saiu logo apos abrir: "
                                   f"{erro or f'codigo {morreu}'}")
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
_list_snap: dict = {"t": 0.0, "infos": None}


async def _cached_info(name: str) -> SessionInfo | None:
    now = time.monotonic()
    if _list_snap["infos"] is None or now - _list_snap["t"] >= _LIST_TTL:
        _list_snap["infos"] = await asyncio.to_thread(registry.list)
        _list_snap["t"] = time.monotonic()
    info = next((s for s in _list_snap["infos"] if s.name == name), None)
    if info is None:
        _list_snap["infos"] = await asyncio.to_thread(registry.list)
        _list_snap["t"] = time.monotonic()
        info = next((s for s in _list_snap["infos"] if s.name == name), None)
    return info


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


def _drain_session(name: str) -> None:
    """Entrega enfileiradas pendentes desta sessao (best-effort, roda fora do request)."""
    try:
        info = next((s for s in registry.list() if s.name == name), None)
        if info and info.jsonl:
            drain(name, info.jsonl, info.provider)
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
        info = next((s for s in registry.list() if s.name == name), None)
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
        if m and m[0] == "working":
            threading.Timer(_CONFIRM_GRACE + 0.5, _confirm_and_drain, args=(name,)).start()
            return
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
        # Kimi espera MAIS antes de declarar perdida. Com max_attempts=0 nao ha segunda chance: a
        # primeira checagem depois do prazo ja carimba `desistiu`. Subir pra 1 nao serve — no
        # reconcile, attempts < max REDIGITA, que e exatamente a duplicacao que este provider nao
        # pode ter. Entao o que se estica e o PRAZO: 30s cobrem o tempo entre a TUI aceitar o texto
        # e ele aparecer no wire.jsonl, sem nunca digitar duas vezes.
        grace = _CONFIRM_GRACE_KIMI if info.provider == "kimi" else _CONFIRM_GRACE
        requeued = q.reconcile_delivered(
            committed_user_lines(info.jsonl, info.provider), _transcript_start_ts(info.jsonl),
            time.time(),
            grace=grace,
            max_attempts=max_attempts,
        )
        if requeued:
            _log.info("REQUEUE name=%s n=%d (TUI engoliu o send; re-drenando)", name, len(requeued))
            drain(name, info.jsonl, info.provider)
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
                sent = drain(info.name, info.jsonl, info.provider)
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
    return await registry.list_with_state()


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
        raise HTTPException(e.status, e.detail) from None
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
        # Pasta não carimbada (ou conta que sumiu): mesmo 404 do apagar() antigo.
        raise HTTPException(e.status, e.detail) from None
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
    # handler async pra poder `await registry.create_codex` (precisa viver no loop principal —
    # ver docstring de create_codex). O caminho Claude (registry.create) e SINCRONO e spawna um
    # subprocess tmux (bloqueante) -> rodar direto aqui travaria o event loop / o SSE de outras
    # sessoes; vai pro threadpool via asyncio.to_thread, igual aos outros handlers async deste
    # arquivo que chamam registry.list()/save_upload (menor risco de regressao: comportamento e
    # exceções do create() Claude ficam IDENTICOS, so a chamada muda de sync p/ thread).
    # Pi entra pelo MESMO registry.create do Claude (pane tmux + spawn_command do PiAdapter); o que
    # muda la dentro e so o transcript, que nao e pre-semeado (layout proprio, arquivo so no 1o turno).
    # Validar provider, config_dir e engine ANTES de qualquer efeito no disco: um pedido que vai
    # ser rejeitado aqui não pode ter reconciliado a conta (deriva movida, memória criada) à toa.
    if body.provider not in ("claude", "codex", "pi", "kimi"):
        raise HTTPException(400, "provider invalido")
    if body.config_dir is not None and body.config_dir not in {c.path for c in list_config_dirs()}:
        raise HTTPException(400, "config_dir invalido")
    # Mesma guarda do config_dir. Codex nao usa spawn_command/tmux desse jeito, entao motor + codex e
    # pedido incoerente — 400, nao "ignora e segue".
    if body.engine is not None:
        if body.provider != "claude":
            raise HTTPException(400, "motor so vale para provider claude")
        if body.engine not in await asyncio.to_thread(engines.listar):
            raise HTTPException(400, "motor invalido")
    # Mesma regra das linhas acima, pro model/effort: recusa ANTES de qualquer efeito no disco,
    # inclusive pro provedor fora de escopo (codex/kimi) quando alguem pedir escolha — o valor
    # entraria num comando de shell montado por concatenacao.
    try:
        model_args.validar(body.provider, body.model, body.effort)
    except ValueError as e:
        raise HTTPException(400, str(e)) from None

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
    # config dir (Claude/Pi — codex nem recebe ele no create_codex). Sem o ciclo, um DELETE da
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
                        # ContaError já carrega status HTTP (o cp-conta imprime o detail). Deixar
                        # escapar viraria 500 com traceback — o usuário não saberia por que a
                        # abertura falhou (ex: Windows sem Modo Desenvolvedor recusando symlink).
                        raise HTTPException(e.status, e.detail) from None
                    except OSError as e:
                        raise HTTPException(500, f"não consegui reconciliar a conta "
                                                 f"{nome_conta}: {e}") from None
                    for aviso in avisos:
                        _log.warning("conta %s: %s", alvo.name, aviso)
                    try:
                        return await asyncio.to_thread(
                            registry.create, body.name, body.cwd, body.config_dir,
                            provider=body.provider, engine=body.engine,
                            model=body.model, effort=body.effort, context_window=janela)
                    except ValueError as e:
                        raise HTTPException(409, str(e))
                finally:
                    # Solta a trava sempre — inclusive quando o corpo levanta HTTPException.
                    await asyncio.to_thread(cm.__exit__, None, None, None)
            except contas.ContaError as e:
                # Conta sumiu entre a validação e a trava (ex: DELETE concorrente).
                raise HTTPException(e.status, e.detail) from None
    try:
        if body.provider == "codex":
            return await registry.create_codex(body.name, body.cwd, body.initial_prompt)
        return await asyncio.to_thread(registry.create, body.name, body.cwd, body.config_dir,
                                       provider=body.provider, engine=body.engine,
                                       model=body.model, effort=body.effort, context_window=janela)
    except ValueError as e:
        raise HTTPException(409, str(e))


@app.delete("/api/sessions/{name}", dependencies=[Depends(require_auth)])
def kill_session(name: str):
    # 500 quando a sessao SOBREVIVE ao kill — mesmo padrao do /rename logo abaixo, que ja confere e
    # responde 404/500. Antes era {"ok": true} incondicional: o card sumia da UI e a sessao reaparecia
    # na varredura seguinte, sem fila e sem pareamento (ver SessionRegistry.kill).
    try:
        registry.kill(name)
    except KillFailed as e:
        raise HTTPException(500, str(e))
    return {"ok": True}


class RenameBody(_StrictBody):
    new: str


@app.post("/api/sessions/{name}/rename", dependencies=[Depends(require_auth)])
def rename_session(name: str, body: RenameBody):
    from app import tmux
    # tmux nao aceita espaco/./: no nome -> sanitiza. O transcript NAO depende do nome (resolve por
    # /proc), entao renomear nao quebra o historico. Migra so o sidecar da fila (keyed por nome).
    new = sanitize_session_name(body.new)
    if not new:
        raise HTTPException(400, "nome invalido")
    if not tmux.has_session(name):
        raise HTTPException(404, "sessao nao encontrada")
    if new == name:
        return {"ok": True, "name": name}
    if tmux.has_session(new):
        raise HTTPException(409, "ja existe uma sessao com esse nome")
    if not tmux.rename_session(name, new):
        raise HTTPException(500, "falha ao renomear")
    registry.rename(name, new)  # migra o cache name->jsonl (senao serve transcript errado pos-rename)
    from app.pqueue import PromptQueue
    try:
        oq, nq = PromptQueue(name).path, PromptQueue(new).path
        if oq.exists():
            oq.replace(nq)
    except OSError:
        pass
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
        raise HTTPException(400, "sessao nao pode encadear pra si mesma")
    if not tmux.has_session(body.target):
        raise HTTPException(404, "sessao alvo nao encontrada")
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
        raise HTTPException(404, "sessão não encontrada")
    if getattr(info, "provider", "claude") != "claude":
        # Codex e outros nao sao tmux: sem hook de transicao, o tick nunca dispara -> loop ficaria
        # running mudo pra sempre. Recusa cedo em vez de criar um loop-zumbi.
        raise HTTPException(409, "loop runner só suporta sessões claude")
    if not automations_enabled():
        raise HTTPException(409, "automações desligadas (kill-switch)")
    with loop_mod._lock:
        link = loop_mod.LoopLink(name)
        cur = link.get()
        if cur and cur["status"] in loop_mod.ACTIVE:
            raise HTTPException(409, "já existe um loop ativo nesta sessão")
        br = branch_of(info.cwd) if info.cwd else None
        if body.require_branch and br in ("main", "master"):
            raise HTTPException(409, f"sessão está na branch {br} — crie uma branch ou desligue 'exigir branch'")
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
            raise HTTPException(404, "nenhum loop nesta sessão")
        d = loop_mod._end(link, name, "stopped", "parado pelo usuário", push.notify_loop)
    return {"loop": d}


@app.post("/api/sessions/{name}/loop/refine", dependencies=[Depends(require_auth)])
def loop_refine(name: str, body: LoopRefine):
    """Refina o objetivo do loop via claude -p efemero (sonnet). Stateless — nao toca a sessao nem o
    sidecar; o {name} da rota so mantem a familia de URLs consistente. Falha do CLI -> 502.
    Sob o kill-switch mestre: refine dispara um agente autonomo, entao respeita automations_enabled."""
    if not automations_enabled():
        raise HTTPException(409, "automações desligadas (kill-switch)")
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
            raise HTTPException(409, "loop não está aguardando confirmação")
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
            raise HTTPException(404, "nenhum transcript pra retomar neste diretorio")
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
        raise HTTPException(404, "session or transcript not found")
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


@app.get("/api/sessions/{name}/workflows", dependencies=[Depends(require_auth)])
async def workflows_list(name: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import list_workflows
    return await asyncio.to_thread(list_workflows, info.jsonl)


@app.get("/api/sessions/{name}/workflows/{run_id}", dependencies=[Depends(require_auth)])
async def workflow_detail(name: str, run_id: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import get_workflow
    wf = await asyncio.to_thread(get_workflow, info.jsonl, run_id)
    if wf is None:
        raise HTTPException(404, "workflow run not found")
    return wf


@app.get("/api/sessions/{name}/workflows/{run_id}/agents/{agent_id}", dependencies=[Depends(require_auth)])
async def workflow_agent_detail(name: str, run_id: str, agent_id: str):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.workflows import get_agent
    a = await asyncio.to_thread(get_agent, info.jsonl, run_id, agent_id)
    if a is None:
        raise HTTPException(404, "agent not found")
    return a


@app.get("/api/sessions/{name}/peer-address", dependencies=[Depends(require_auth)])
async def peer_address(name: str):
    """Endereço do inbox nativo desta sessão (cross-session messaging), ou `null`.

    Existe pro `cp-send` decidir com FATO se o caminho nativo alcança este alvo, em vez de supor
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
        raise HTTPException(404, "session or transcript not found")
    from app.subagents import list_subagents
    return await asyncio.to_thread(list_subagents, info.jsonl)


@app.get("/api/sessions/{name}/subagents/{agent_id}", dependencies=[Depends(require_auth)])
async def subagent_detail(name: str, agent_id: str, events: int = 0):
    info = await _cached_info(name)
    if not info or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.subagents import get_subagent
    # events=N -> devolve tambem o transcript do subagente nos MESMOS ChatEvent do chat, pra a UI
    # reusar a lista de mensagens em vez de desenhar um formato proprio.
    a = await asyncio.to_thread(get_subagent, info.jsonl, agent_id, 40, events)
    if a is None:
        raise HTTPException(404, "subagent not found")
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
        raise HTTPException(404, "session or transcript not found")
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
_send_executor = ThreadPoolExecutor(max_workers=4, thread_name_prefix="cp-send")


def _send_thread(fn, *args):
    """Roda `fn(*args)` no pool DEDICADO de envio (nao no executor default, saturavel pela decoracao)."""
    return asyncio.get_running_loop().run_in_executor(_send_executor, fn, *args)


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
    is_pi = provider == "pi" and pane_id and INBOX.tem_linha(pane_id) and not stripped.startswith("/")
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
        # feedback. Agora vira 400 limpo (o frontend mostra). (Multi-linha de verdade: backlog.)
        return {"ok": False, "error": str(e)}
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
            erro = ("envio incompleto: o composer foi limpo e a mensagem NAO foi enviada — pode "
                     "reenviar sem risco de duplicar.")
        else:
            erro = ("envio incompleto: parte do texto ficou no composer da sessao e nada foi "
                     "submetido. Confira o terminal antes de reenviar.")
        return {"ok": False, "error": erro}
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
                return {"ok": False, "error": f"fila indisponivel e prompt nao foi digitado: {e}"}
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
            return {"ok": False, "error": f"fila indisponivel e prompt nao foi entregue: {e}"}
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
        return {"ok": False, "error": str(e)}
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
        return {"ok": False, "error": "fila indisponivel e o turno nao aceitou o prompt: nao foi entregue"}
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
        raise HTTPException(404, "sessão não encontrada — recado NÃO enfileirado")
    if _provider_of(name) == "codex":
        res = await _send_one_codex(name, body.text)
    else:
        res = await _send_thread(_send_one, name, body.text)
    if not res["ok"]:
        raise HTTPException(400, res["error"])
    # delivered: True = digitou agora na TUI; False = na fila durável (entrega no próximo idle).
    return {"ok": True, "delivered": res.get("delivered", False)}


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
            results[name] = {"ok": False, "error": "sessão não encontrada", "delivered": False}
            continue
        if _provider_of(name) == "codex":
            results[name] = await _send_one_codex(name, body.text)
        else:
            results[name] = await _send_thread(_send_one, name, body.text)
    return {"results": results}


class PairBody(_StrictBody):
    # peer (1) OU peers (N) — peers vence; peer fica por compat (cp-send --pair manda um só).
    peer: str = ""
    peers: list[str] = []
    task: str = ""


def _group_text(me: str, others: list[str], task: str) -> str:
    t = f" na tarefa: {task.strip()}" if task.strip() else ""
    quem = ", ".join(f"'{o}'" for o in others)
    exemplo = others[0]
    # Par remoto (srv::sessao): contrato compartilhado não sincroniza cross-server no MVP — some a
    # linha do arquivo (cada máquina teria o seu, com gid diferente). cp-send já roteia srv::sessao.
    cross = any(peers.is_remote(o) for o in others)
    contrato = "" if cross else (
        f"Contrato/decisões que o grupo precisa consultar: registrar no arquivo compartilhado "
        f"{contract_path_for(me)} (markdown; criar se não existir, manter curto e atual). ")
    return (
        f"[de: claude-pocket] GRUPO DE TRABALHO ATIVO: você ('{me}') trabalha junto com {quem}{t}. "
        f"Cada sessão mexe SÓ no próprio repo; quando precisar de algo de outro membro (contrato, "
        f"endpoint, tipo, dúvida), mande 1:1 por iniciativa própria. COMO mandar, nesta ordem: "
        f"se você TEM a ferramenta SendMessage e o membro aparece no seu ListAgents (sessão Claude "
        f"desta máquina), use SendMessage — a entrega é por socket, sem digitar no terminal, então "
        f"nada de texto cortado ou colado pela metade. Não tem a ferramenta, ou o membro não está "
        f"na lista (sessão de outra máquina 'servidor::sessao', Codex, Pi)? Aí é o Bash: "
        f'cp-send {exemplo} "sua mensagem". Os dois chegam do mesmo jeito, como [de: <membro>]. '
        f'AVISO pro grupo TODO (marco: "terminei minha parte", "contrato atualizado"): '
        f'cp-send --group "sua mensagem" (uma vez, chega como [grupo: <membro>]). '
        f"REGRA ANTI-LOOP: NUNCA responda um [grupo: ...] com --group (vira tempestade). Aviso de "
        f"grupo é unidirecional; se precisar responder, faça 1:1 (cp-send <membro>) e só se necessário. "
        f"{contrato}"
        f"BRANCH: antes de trabalhar, rode git branch --show-current no SEU repo e alinhe pra "
        f"branch do ticket da tarefa (fetch+checkout) — re-verifique após restart/resume da sessão. "
        f"Exceção única: o usuário pedir explicitamente outra branch. Checkout DUPLICADO do repo "
        f"na máquina → alerte o usuário e pergunte qual é o canônico antes de mexer. "
        f"Commit/push e decisões de rumo continuam com o usuário. Confirme em uma linha."
    )


async def _deliver(name: str, text: str) -> str | None:
    # Mesma esteira do /input (fila durável se ocupada), ramificada por provider.
    # Devolve o erro (str) ou None — _send_one/_send_one_codex NUNCA levantam, reportam no dict;
    # engolir isso fazia o pareamento dizer "ok" com o aviso jamais entregue.
    if _provider_of(name) == "codex":
        res = await _send_one_codex(name, text)
    else:
        res = await _send_thread(_send_one, name, text)
    return None if res.get("ok") else (res.get("error") or "falha desconhecida no envio")


@app.post("/api/sessions/{name}/pair", dependencies=[Depends(require_auth)])
async def pair_session(name: str, body: PairBody):
    """Junta `name` e peer(s) num GRUPO de trabalho (une os grupos existentes de todos) e injeta
    em CADA membro o prompt do grupo atualizado — a partir daí trocam recados via cp-send por
    iniciativa própria, dentro do escopo da tarefa. Badge `pair_peers` aparece na lista."""
    others = [p for p in dict.fromkeys(body.peers or ([body.peer] if body.peer else [])) if p]
    if not others:
        raise HTTPException(400, "informe peer ou peers")
    if name in others:
        raise HTTPException(400, "não dá pra parear uma sessão com ela mesma")
    if any(peers.is_remote(o) for o in others):
        # Cross-server é 1:1 puro (um peer remoto, sem misturar grupo local) — grupo cross-server de
        # N fica pra fase 2. ponytail: 1:1 cobre "trabalhar junto entre máquinas"; N quando doer.
        if len(others) != 1:
            raise HTTPException(400, "pareamento cross-server é 1:1 por enquanto: um peer remoto, "
                                     "sem misturar com grupo local")
        if not settings.server_id:
            raise HTTPException(400, "CP_SERVER_ID ausente no backend/.env — obrigatório pra "
                                     "pareamento cross-server (é o endereço de resposta srv::sessao)")
        return await _pair_cross_server(name, others[0], body.task)
    names = {s.name for s in await asyncio.to_thread(registry.list)}
    missing = [p for p in [name, *others] if p not in names]
    if missing:
        raise HTTPException(404, f"sessão não encontrada: {', '.join(missing)}")
    # join_group: snapshot + join na MESMA seção crítica (em seções separadas, um join concorrente
    # na janela entre elas entrava no grupo fora do snapshot e um rollback posterior não o
    # reverteria). O snapshot volta pra cá pra desfazer se o aviso não chegar em ninguém.
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, others, body.task)
    except pair.PairMixError as e:
        # Uma das sessões locais já está pareada cross-server (1:1) — não dá pra fundir em grupo local.
        raise HTTPException(400, str(e))
    link = await asyncio.to_thread(lambda: PairLink(name).get() or {})
    task = link.get("task", body.task)
    errs = []
    for m in members:
        e = await _deliver(m, _group_text(m, [x for x in members if x != m], task))
        if e:
            errs.append(f"{m}: {e}")
    if len(errs) == len(members):
        # NINGUÉM foi avisado -> grupo fantasma; restaura o estado anterior e reporta.
        await asyncio.to_thread(pair.restore, snap)
        raise HTTPException(502, f"pareamento desfeito: falha ao avisar as sessões ({'; '.join(errs)})")
    # Falha parcial: grupo vale (AO MENOS 1 membro sabe), e o warning nomeia quem ficou sem aviso
    # — o front mostra em vez de fingir sucesso total.
    return {"ok": True, "members": members,
            "warning": ("aviso falhou em: " + "; ".join(errs)) if errs else None}


async def _pair_cross_server(name: str, peer: str, task: str) -> dict:
    """Pareamento 1:1 entre máquinas. Registra o vínculo LOCAL (name.json peers=[srv::sessao];
    sidecar do remoto vive na máquina dele) e chama o /pair-remote do backend peer pra registrar o
    reverso + injetar o protocolo lá. Falha na chamada remota desfaz o vínculo local (mesmo racional
    do 'grupo fantasma' do pair local). Transporte já provado pelo cp-send cross-server (peers.json)."""
    local_names = {s.name for s in await asyncio.to_thread(registry.list)}
    if name not in local_names:
        raise HTTPException(404, f"sessão não encontrada: {name}")
    srv, sess = peers.split_addr(peer)
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, [peer], task)
    except pair.PairMixError as e:
        # `name` já está num grupo local (ou já pareada cross-server): não dá pra cross-parear.
        raise HTTPException(400, str(e))
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
            raise HTTPException(502, f"pareamento NÃO confirmado (falha de rede com '{srv}'): desfeito "
                                     f"deste lado; se o peer tiver ficado pareado, rode unpair lá. ({e})")
        raise HTTPException(502, f"pareamento desfeito (peer rejeitou): {e}")
    # Reverso registrado. Injeta o protocolo NESTE lado; se este falhar (sessão morreu na janela), o
    # vínculo já vale dos dois lados — só avisa, não desfaz (o par remoto já sabe).
    warn = None
    e = await _deliver(name, _group_text(name, [peer], task))
    if e:
        warn = f"vínculo criado, mas o aviso local falhou ({name}: {e}) — refaça o pair se precisar"
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
        raise HTTPException(400, "initiator precisa ser qualificado (srv::nome)")
    local_names = {s.name for s in await asyncio.to_thread(registry.list)}
    if name not in local_names:
        raise HTTPException(404, f"sessão não encontrada: {name}")
    try:
        members, snap = await asyncio.to_thread(pair.join_group, name, [body.initiator], body.task)
    except pair.PairMixError as e:
        # `name` já está num grupo local aqui — não pode virar par cross-server de outra máquina.
        raise HTTPException(409, str(e))
    e = await _deliver(name, _group_text(name, [body.initiator], body.task))
    if e:
        await asyncio.to_thread(pair.restore, snap)
        raise HTTPException(502, f"pareamento desfeito: falha ao avisar '{name}': {e}")
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
        e = await _deliver(name, f"[de: claude-pocket] '{body.peer}' saiu do pareamento. "
                                 "Volte a operar independente; use cp-send só quando o usuário pedir.")
        if e:
            warn = f"{name}: {e}"
    return {"ok": True, "warning": warn}


class GroupMsgBody(_StrictBody):
    text: str


@app.post("/api/sessions/{name}/group-message", dependencies=[Depends(require_auth)])
async def group_message(name: str, body: GroupMsgBody):
    """Aviso pro GRUPO todo (cp-send --group): entrega o texto a CADA companheiro de `name` numa
    tacada, como `[grupo: <name>]`. Unidirecional por contrato (o prompt instrui a NUNCA responder
    um [grupo:] com --group) — é o que impede o loop de N sessões se avisando em cascata.
    Slash-command fora (mesmo racional do /broadcast)."""
    if body.text.lstrip().startswith("/"):
        raise HTTPException(400, "group-message não suporta slash-commands")
    link = await asyncio.to_thread(lambda: PairLink(name).get())
    peers = link.get("peers") if link else None
    if not peers:
        raise HTTPException(404, "sessão não está num grupo")
    text = f"[grupo: {name}] {body.text}"
    results: dict[str, dict] = {}
    for p in peers:
        if not await _send_thread(_session_exists, p):
            results[p] = {"ok": False, "error": "sessão não encontrada", "delivered": False}
            continue
        if _provider_of(p) == "codex":
            results[p] = await _send_one_codex(p, text)
        else:
            results[p] = await _send_thread(_send_one, p, text)
    failed = [f"{n}: {r.get('error')}" for n, r in results.items() if not r.get("ok")]
    return {"ok": True, "peers": peers,
            "warning": ("falha em: " + "; ".join(failed)) if failed else None}


@app.get("/api/sessions/{name}/pair/contract", dependencies=[Depends(require_auth)])
def pair_contract(name: str):
    """Contrato compartilhado do GRUPO (markdown que os membros editam via fs; keyed pelo gid —
    estável quando membro entra/sai). 404 sem grupo; content vazio se ainda não existe."""
    p = contract_path_for(name)
    if p is None:
        raise HTTPException(404, "sessão não está pareada")
    link = PairLink(name).get() or {}
    try:
        content = p.read_text(encoding="utf-8")
    except OSError:
        content = ""
    return {"peers": link.get("peers", []), "path": str(p), "content": content}


@app.delete("/api/sessions/{name}/pair", dependencies=[Depends(require_auth)])
async def unpair_session(name: str):
    """`name` SAI do grupo (os demais membros continuam entre si; grupo restante de 1 dissolve).
    Avisa quem saiu e quem ficou. Idempotente. Aviso que falhar NÃO refaz o vínculo (fora do grupo
    é o estado desejado) — só reporta no result."""
    expeers = await asyncio.to_thread(pair.leave, name)   # nome próprio: 'peers' é o módulo importado
    if not expeers:
        return {"ok": True, "warning": None}
    errs = []
    # Pares REMOTOS (srv::sessao): avisa o backend deles pra limpar o reverso — não dá pra _deliver
    # local numa sessão de outra máquina. Os locais são avisados no loop abaixo.
    for p in expeers:
        if not peers.is_remote(p):
            continue
        if not settings.server_id:
            errs.append(f"{p}: CP_SERVER_ID ausente — par remoto não avisado")
            continue
        srv, sess = peers.split_addr(p)
        try:
            await asyncio.to_thread(peers.call, srv, "POST",
                                    f"/api/sessions/{sess}/unpair-remote",
                                    {"peer": f"{settings.server_id}::{name}"})
        except peers.PeerError as ex:
            # `name` já saiu localmente (pair.leave acima); o peer não pôde ser avisado -> sidecar
            # remoto fica órfão até alguém desparear lá. Loga pra rastreabilidade (journalctl) além
            # do warning no result. ponytail: sem fila de retry durável — single-user, recuperável na
            # mão; se virar comum, enfileirar via pqueue como o /input faz.
            _log.warning("unpair: peer remoto '%s' não avisado (sidecar de lá fica órfão): %s", p, ex)
            errs.append(f"{p}: {ex}")
    e = await _deliver(name, "[de: claude-pocket] Você saiu do grupo de trabalho "
                             f"({', '.join(expeers)}). Volte a operar independente; use cp-send só "
                             "quando o usuário pedir.")
    if e:
        errs.append(f"{name}: {e}")
    resto = [p for p in expeers if not peers.is_remote(p)]
    for p in resto:
        ainda = [x for x in resto if x != p]
        msg = (f"[de: claude-pocket] '{name}' saiu do grupo de trabalho. "
               + (f"O grupo continua entre você e {', '.join(ainda)}."
                  if ainda else "O grupo foi dissolvido (só restava você); volte a operar independente."))
        e = await _deliver(p, msg)
        if e:
            errs.append(f"{p}: {e}")
    return {"ok": True, "warning": ("aviso de saída falhou: " + "; ".join(errs)) if errs else None}


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
                            detail="Terminal aberto nesta sessao. Feche o painel pra responder "
                                   "por aqui.")


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
        raise HTTPException(404, "sessão não encontrada — opção NÃO enviada")
    terminal.select(name, body.option)
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
        raise HTTPException(400, "limits so existe pra sessoes Codex")
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
async def codex_models(name: str):
    # Task C: modelo + reasoning effort so pra Codex (via model/list) -- o /model do Claude e o
    # picker interativo dedicado (/model-effort), sem esta rota.
    if _provider_of(name) != "codex":
        raise HTTPException(400, "models so existe pra sessoes Codex")
    adapter = get_adapter("codex")
    return {"models": await adapter.list_models(name), "current": adapter.current_model(name)}


@app.post("/api/sessions/{name}/model", dependencies=[Depends(require_auth)])
async def set_codex_model(name: str, body: CodexModelBody):
    # Grava a escolha e reabre/configura a TUI; se ha turno em voo, aplica ao terminar.
    if _provider_of(name) != "codex":
        raise HTTPException(400, "model so existe pra sessoes Codex")
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
        raise HTTPException(404, "sessao nao encontrada")
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
            "scan_roots": settings.scan_roots,
            "terminal_panel": os.name == "posix",   # `pty` e POSIX-only; sem ele o painel nao existe
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


@app.post("/api/sessions/{name}/upload", dependencies=[Depends(require_auth)])
async def upload(name: str, request: Request):
    # Resolve o cwd da sessao (registry.list() ja traz cwd via tmux #{pane_current_path}).
    # handler async -> registry.list() (subprocess tmux) no threadpool pra nao bloquear o loop.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if info is None:
        raise HTTPException(404, "sessao nao encontrada")
    if not info.cwd:
        raise HTTPException(409, "cwd da sessao indisponivel")
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 100 * 1024 * 1024:
        raise HTTPException(413, "arquivo maior que 100 MiB")
    data = await request.body()
    # Filename do cliente (X-Filename, percent-encoded) ou ?name= -> so a EXTENSAO e usada
    # (o nome final e gerado pelo servidor). Qualquer tipo de arquivo.
    filename = request.headers.get("x-filename") or request.query_params.get("name")
    try:
        # write_bytes (ate 100 MiB) no threadpool pra nao bloquear o loop durante o disco.
        path = await asyncio.to_thread(save_upload, info.cwd, data, filename)
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
async def transcribe_audio(name: str, request: Request, limpar: bool = False):
    # Salva o audio (pra anexar o path no chat) E transcreve via Groq num round-trip. Mesmo padrao
    # de upload (raw body + X-Filename). Devolve {path, text} -> o front monta "texto — 📎 audio: path".
    # `limpar` so o microfone manda: audio ANEXADO (arquivo de ate 10min) nao pode pagar a limpeza.
    # Desligado (default), a resposta e byte a byte a de sempre -> quem ja consome nao muda.
    sessions = await asyncio.to_thread(registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if info is None:
        raise HTTPException(404, "sessao nao encontrada")
    if not info.cwd:
        raise HTTPException(409, "cwd da sessao indisponivel")
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 100 * 1024 * 1024:
        raise HTTPException(413, "arquivo maior que 100 MiB")
    data = await request.body()
    filename = request.headers.get("x-filename") or request.query_params.get("name")
    try:
        path = await asyncio.to_thread(save_upload, info.cwd, data, filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)
    # Transcricao (chamada de rede bloqueante) no threadpool pra nao travar o loop.
    try:
        text = await asyncio.to_thread(transcribe, data, filename)
    except TranscribeError as e:
        raise HTTPException(e.status, e.detail)
    if not limpar:
        return {"path": path, "text": text}
    texto_limpo, aviso = await asyncio.to_thread(narrar.limpar_ditado, text)
    return {"path": path, "text": texto_limpo, "raw": text, "aviso": aviso}


@app.get("/api/sessions/{name}/uploads/{filename}", dependencies=[Depends(require_auth)])
def serve_upload(name: str, filename: str):
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None or not info.cwd:
        raise HTTPException(404, "sessao nao encontrada")
    try:
        path = resolve_upload(info.cwd, filename)
    except UploadError as e:
        raise HTTPException(e.status, e.detail)
    return FileResponse(path)


@app.get("/api/sessions/{name}/uploads", dependencies=[Depends(require_auth)])
def list_session_uploads(name: str):
    # Galeria de anexos: a retencao vive no servidor, entao o prazo sai daqui pronto (o front so
    # desenha). Le do runtime_config, nao do env cru — senao a galeria mostraria um prazo e o
    # prune usaria outro.
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None or not info.cwd:
        raise HTTPException(404, "sessao nao encontrada")
    return {"files": list_uploads(info.cwd, runtime_config.get("upload_retention_days"))}


class CheckoutBody(_StrictBody):
    branch: str


class GitActionBody(_StrictBody):
    # allowlist declarativa no schema (alem do git_ops)
    action: Literal["status", "pull", "fetch", "stash", "stash-pop", "log",
                    "revert-abort", "cherry-pick-abort"]


class GitPathBody(_StrictBody):
    path: str   # validado em git_ops contra a lista real de arquivos alterados (anti-traversal)


class GitCommitBody(_StrictBody):
    message: str = Field(min_length=1)
    paths: list[str] = []        # sem min_length: amend=True aceita [] (reword); git_ops barra [] sem amend
    amend: bool = False
    new_branch: str | None = None


def _session_cwd(name: str) -> str:
    # cwd da sessao tmux (mesmo lookup do upload). 404 se a sessao/cwd nao existe.
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None or not info.cwd:
        raise HTTPException(404, "sessao nao encontrada")
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
        raise HTTPException(404, "sem plano ativo")
    try:
        markdown = await asyncio.to_thread(
            lambda: Path(p.path).read_text(encoding="utf-8", errors="replace"))
    except OSError:
        # plan_progress vem de cache; o arquivo pode ter sumido/perdido permissao entre a leitura
        # cacheada e esta segunda leitura. Degrada pra markdown vazio, mas NAO engole em silencio.
        _log.warning("falha lendo markdown do plano path=%s", p.path, exc_info=True)
        markdown = ""
    return {
        "name": p.name, "path": p.path,
        "task": p.task_idx, "task_total": p.task_total,
        "done": p.done, "total": p.total, "complete": p.complete,
        "tasks": [{"title": t.title, "done": t.done, "total": t.total,
                   "steps": [{"title": s.title, "done": s.done, "manual": s.manual}
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
        raise HTTPException(404, "repo sem pasta de planos")
    return r


class PlanPinBody(_StrictBody):
    stem: str | None = None   # None = solta o pin e volta pra eleicao automatica


@app.post("/api/sessions/{name}/plan-pin", dependencies=[Depends(require_auth)])
async def session_plan_pin(name: str, body: PlanPinBody):
    """Fixa qual plano o painel mostra. Vale ate o plano fechar: em 100% o pin e ignorado e a
    eleicao automatica volta (ver planprog.plan_progress)."""
    cwd = await asyncio.to_thread(_session_cwd, name)
    root = await asyncio.to_thread(_plans_dir, cwd)
    if root is None:
        raise HTTPException(404, "repo sem pasta de planos")
    if body.stem is not None and body.stem != PIN_NONE:
        # So um plano que existe DE VERDADE nesta raiz. Sem isto, o stem viraria nome de arquivo
        # vindo do cliente — e a checagem de traversal do read_pin nao cobriria um nome valido
        # apontando pra plano de outro repo. A guarda de separador vem ANTES do isfile: com um
        # `../..` o proprio isfile ja responderia se existe .md fora da pasta de planos.
        if not is_safe_stem(body.stem):
            raise HTTPException(400, f"nome de plano invalido: {body.stem}")
        alvo = os.path.join(root, body.stem + ".md")
        if not await asyncio.to_thread(os.path.isfile, alvo):
            raise HTTPException(404, f"plano nao encontrado: {body.stem}")
    try:
        await asyncio.to_thread(write_pin, root, body.stem)
    except PlanPinError as e:
        raise HTTPException(500, f"nao deu pra gravar o pin: {e}")
    return {"pinned": body.stem}


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
    return runner.start_run(_session_cwd(name), body.command)


@app.post("/api/sessions/{name}/run/stop", dependencies=[Depends(require_auth)])
def stop_runner(name: str):
    runner.stop_run(_session_cwd(name))
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
        raise HTTPException(500, f"editor '{binario}' falhou: {e}")
    return {"ok": True}


@app.get("/api/sessions/{name}/transcript-image/{uuid}/{idx}", dependencies=[Depends(require_auth)])
def transcript_image(name: str, uuid: str, idx: int):
    # Serve uma imagem colada no TERMINAL (base64 no .jsonl) sob demanda. Decodifica por uuid+idx.
    jsonl = next((s.jsonl for s in registry.list() if s.name == name), None)
    if not jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.transcript import get_transcript_image
    got = get_transcript_image(jsonl, uuid, idx)
    if got is None:
        raise HTTPException(404, "image not found")
    raw, media = got
    # immutable: o conteudo de um uuid+idx nunca muda -> cache agressivo no cliente.
    return Response(content=raw, media_type=media, headers={"Cache-Control": "max-age=31536000, immutable"})


# ── Arquivo: conversas mortas (transcripts sem sessao tmux viva) ──────────────
@app.get("/api/archive", dependencies=[Depends(require_auth)], response_model=list[ArchiveFolder])
def archive_index():
    # Nivel 1: so as PASTAS (agregado barato). As conversas vem por pasta, no endpoint abaixo.
    return list_folders()


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
def archive_history(project: str, session_id: str):
    try:
        p = archive_jsonl(project, session_id)
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


@app.post("/api/archive/{project}/{session_id}/resume", dependencies=[Depends(require_auth)],
          response_model=SessionInfo)
def resume_archived(project: str, session_id: str, body: ResumeArchivedBody = ResumeArchivedBody()):
    # "Retomar conversa" do Arquivo: sobe uma sessao tmux NOVA no cwd original com `claude --resume
    # <uuid>` -- reusa registry.create (nome/config_dir/spawn tmux ja tratados), so troca o comando pro
    # uuid EXISTENTE (nao um novo transcript). Nome derivado do basename do cwd, igual ao
    # CreateSessionSheet do front; colisao suffixa -2/-3... (mesmo esquema, do lado do backend pq aqui
    # nao ha form pro usuario escolher nome).
    from app import tmux
    try:
        cwd = archive_cwd(project, session_id)
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
    while tmux.has_session(name):
        name = f"{base}-{i}"
        i += 1
    try:
        return registry.create(name, cwd, resume_session_id=session_id, engine=body.engine)
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


@app.get("/api/sessions/{name}/file", dependencies=[Depends(require_auth)])
def serve_file(name: str, path: str):
    # Serve QUALQUER arquivo referenciado na conversa (video/html/codigo/pdf/...). TRAVA de seguranca:
    # so serve se o `path` aparece no transcript desta sessao (citado por voce ou pelo Claude =
    # consentido) E existe E e arquivo regular -> bloqueia leitura arbitraria de disco / path-traversal.
    # FileResponse trata Range -> <video> faz seek/streaming.
    # Path RELATIVO (ex "./mock.png", "sub/x.png") resolve contra o CWD DA SESSAO (onde o Claude criou
    # o arquivo), nao o cwd do processo backend; guard extra: o resolvido nao pode ESCAPAR do cwd.
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None or not info.jsonl:
        raise HTTPException(404, "session or transcript not found")
    from app.transcript import path_in_transcript
    if not path_in_transcript(info.jsonl, path):
        raise HTTPException(403, "file not referenced in this conversation")
    expanded = os.path.expanduser(path)
    if os.path.isabs(expanded):
        real = os.path.realpath(expanded)
    else:
        if not info.cwd:
            raise HTTPException(409, "cwd da sessao indisponivel")
        base = os.path.realpath(info.cwd)
        real = os.path.realpath(os.path.join(base, expanded))
        if real != base and not real.startswith(base + os.sep):
            raise HTTPException(403, "path escapes session cwd")
    if not os.path.isfile(real):
        raise HTTPException(404, "file not found")
    media = mimetypes.guess_type(real)[0] or "application/octet-stream"
    return FileResponse(real, media_type=media)


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
    info = next((s for s in registry.list() if s.name == name), None)
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
            raise HTTPException(409, "nenhuma pergunta do Pi pendente (ja respondida no terminal?)")
        if not answers:
            raise HTTPException(409, "sem resposta")
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
                raise HTTPException(409, f"drive falhou ({e}) e nao ha texto de fallback — responda no terminal")
            terminal.interrupt(name)  # Escape unico: fecha o picker do Pi (sem clear — input vazio)
            _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
            res = _send_one(name, text)
            if not res["ok"]:
                raise HTTPException(409, f"drive falhou e fallback por texto tambem: {res['error']}")
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
            raise HTTPException(409, "nenhuma pergunta do Kimi pendente (ja respondida no terminal?)")
        if not answers:
            raise HTTPException(409, "sem resposta")
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
                raise HTTPException(409, "resposta enviada, mas nao deu pra confirmar a tempo — "
                                         "confira na sessao antes de responder de novo")
        except ValueError as e:
            raise HTTPException(409, str(e))
        except terminal_input.DriveError as e:
            text = _pi_answer_fallback_text(answers[0])
            _log.warning("KIMI-QUESTION fallback name=%s reason=%s text=%r", name, e, text[:120])
            if not text:
                # Sem texto de fallback, NAO manda o Escape: picker aberto = o usuario ainda pode
                # responder no terminal. Fechar e devolver ok sem entregar nada seria a pior saida.
                raise HTTPException(409, f"drive falhou ({e}) e nao ha texto de fallback — responda no terminal")
            terminal.interrupt(name)  # Escape unico: fecha o picker do Kimi (sem clear — input vazio)
            _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
            res = _send_one(name, text)
            if not res["ok"]:
                raise HTTPException(409, f"drive falhou e fallback por texto tambem: {res['error']}")
            return {"ok": True, "fallback": True}
        return {"ok": True, "fallback": False}
    try:
        terminal_input.answer_questions(name, answers)
    except ValueError as e:
        raise HTTPException(409, str(e))
    except terminal_input.DriveError as e:
        text = _askq_fallback_text(answers, jsonl)
        _log.warning("ASKQ fallback name=%s reason=%s text=%r", name, e, text[:120])
        terminal.interrupt(name)  # Escape unico: fecha o picker (sem clear — input vazio)
        if text:
            _espera_picker_fechar(name)   # sem isto o texto sai junto do Escape e a TUI o engole
            res = _send_one(name, text)
            if not res["ok"]:
                raise HTTPException(409, f"drive falhou e fallback por texto tambem: {res['error']}")
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
# Uma hora, e nao dez minutos, porque a lista muda quando a Anthropic lanca modelo ou o plano do
# usuario muda — nao de minuto em minuto. A chave e o config dir, nao a sessao: a lista vem da
# CONTA, e a mesma pra todas as sessoes dela.
_CLAUDE_MODELS_TTL = 3600.0
_claude_models_cache: dict[str, tuple[float, dict]] = {}


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
        raise HTTPException(409, f"motor {nome!r} nao esta mais no engines.json")
    try:
        modelos = await asyncio.to_thread(engine_probe.listar_modelos, cfg["base_url"], cfg["api_key"])
    except RuntimeError as e:
        # A mensagem do provedor E a informacao util (key invalida, host fora do ar).
        raise HTTPException(502, f"o provedor do motor {nome!r} nao respondeu: {e}")
    _engine_models_cache[nome] = (time.monotonic(), modelos)
    return modelos


@app.get("/api/sessions/{name}/model/options", dependencies=[Depends(require_auth)])
async def model_options(name: str):
    """Modelos que ESTA sessao pode escolher. `kind` diz de onde vieram e como aplicar."""
    info = await _cached_info(name)
    if not info:
        raise HTTPException(404, "sessao nao encontrada")
    if info.provider not in (None, "claude"):
        raise HTTPException(400, "esta rota so existe pra sessoes Claude Code")
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
    hit = _claude_models_cache.get(chave)
    if hit and time.monotonic() - hit[0] < _CLAUDE_MODELS_TTL:
        return hit[1]
    try:
        lido = await asyncio.to_thread(terminal.list_model_options, name)
    except PickerError as e:
        raise HTTPException(e.status, e.detail)
    resp = {"kind": "claude", "engine": None, "effort": lido["effort"],
            # `id` (único por linha), não `keyword`: duas linhas do picker compartilham a keyword
            # `opus` ("Opus" e "Opus (1M context)"), e id repetido derrubava a lista na tela.
            "models": [{"id": r["id"], "name": r["name"], "desc": r["desc"],
                        "active": r["active"]} for r in lido["models"]]}
    _claude_models_cache[chave] = (time.monotonic(), resp)
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
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            raise HTTPException(502, detail=erro("erro_pi_list_models", f"pi --list-models falhou: {e}", erro=str(e)))
    if provider != "claude":
        raise HTTPException(400, detail=erro("erro_provider_invalido", "provider deve ser 'claude' ou 'pi'"))
    if engine:
        modelos = await _engine_models(engine)
        return {"kind": "engine", "reduced": False,
                "models": [{"id": m["id"], "context_length": m.get("context_length"),
                            "vision": m.get("vision")} for m in modelos]}
    hit = _claude_models_cache.get(_chave_config(config_dir))
    if hit and time.monotonic() - hit[0] < _CLAUDE_MODELS_TTL:
        return {**hit[1], "reduced": False}
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
        raise HTTPException(404, "sessao nao encontrada")
    if not info.engine:
        raise HTTPException(400, "esta rota so existe pra sessoes que rodam num motor")
    # fresco=True: a validacao promete "recusa aqui em vez de deixar a falha aparecer so no proximo
    # turno", e essa promessa nao sobrevive ao cache de 5 min (ver _engine_models).
    modelos = await _engine_models(info.engine, fresco=True)
    if not any(m["id"] == body.model for m in modelos):
        # Recusar aqui em vez de digitar: o CC aceitaria o id, a sessao passaria a mandar request
        # pra um modelo que o provedor nao tem, e a falha apareceria so no proximo turno.
        raise HTTPException(422, f"modelo fora do catalogo do motor {info.engine!r}: {body.model}")

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
# porque o mecanismo e um terceiro: a extensao cp-state.ts publica o catalogo num sidecar e expoe
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
        raise HTTPException(404, "sessao ou transcript nao encontrado")
    if info.provider != "pi":
        raise HTTPException(400, "esta rota so existe pra sessoes Pi")
    cat = await asyncio.to_thread(pi_models.read_catalog, info.jsonl, _session_config_dir(name))
    if cat is None:
        # Falha ALTA: sem o sidecar nao ha catalogo real, e inventar um faria o app oferecer
        # modelos que o `/cp-model` nao encontraria. Instrucao junto porque a causa e sempre a
        # mesma (extensao velha/ausente) e o conserto e um comando.
        raise HTTPException(409, "catalogo do Pi indisponivel — rode ./scripts/install-claude-wrapper.sh "
                                 "e reinicie a sessao (extensao cp-state.ts desatualizada)")
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
        raise HTTPException(422, "informe model (com provider) e/ou effort")
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
        raise HTTPException(409, "comandos digitados, mas o Pi nao republicou o catalogo — nao da "
                                 "pra confirmar a troca; veja o modelo no proprio terminal")
    cur = after.get("current") or {}
    raise HTTPException(409, f"o Pi recusou a troca — segue em "
                             f"{cur.get('provider')}/{cur.get('id')} (raciocinio "
                             f"{after.get('thinking')}). Causa mais comum: sem chave configurada "
                             f"pro provedor pedido (o Pi avisa dentro do TUI)")


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
    cwd = next((s.cwd for s in registry.list() if s.name == name), None)
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
