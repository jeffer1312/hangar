"""Claude SEM terminal: o `claude` como processo filho do backend, falando stream-json.

Sem pane, sem tmux, sem raspar tela. O que muda em relação à sessão Claude comum é só a FONTE
do que é vivo: estado, prévia, permissão, pergunta e statusline vêm do stdout do processo, e a
entrada vai pelo stdin. O histórico continua no `.jsonl` que o próprio Claude grava, lido pelo
mesmo parser (`TranscriptTailer`) das sessões no tmux — bolhas, tool cards, thinking, imagens,
`/history`, estatísticas e o `reset` do `/clear` não sabem que não há pane.

Protocolo (medido contra a CLI, ver docs/research/claude-sem-terminal-monocode.md):
- stdin: `{"type":"user","message":{...}}` manda prompt; `{"type":"control_request",...}` com
  `initialize`, `interrupt`, `set_model`, `set_permission_mode`, `list_models`; e
  `{"type":"control_response",...}` responde permissão/pergunta.
- stdout: `system/init` (session_id, modelo, modo), `stream_event` (deltas), `assistant`,
  `user` (tool_result), `control_request can_use_tool` (permissão e AskUserQuestion), `result`
  (fim de turno, custo, uso), `rate_limit_event`, `conversation_reset` (/clear).

O processo morre com o backend; o próximo prompt sobe outro com `--resume <sid>`. Tudo que
precisa sobreviver está no sidecar (sessions.py).
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import shutil
import signal
import time
from pathlib import Path
from typing import AsyncIterator, Callable, Optional

from app import model_args
from app.adapters.claude_headless import sessions as hl_sessions
from app.adapters.codex.adapter import _fmt_tok, _format_reset
from app.adapters.codex.preview import CodexPreviewSource
from app.config import settings
from app.pqueue import PromptQueue
from app.state import StateEvent
from app.transcript import ChatEvent, TranscriptTailer

_log = logging.getLogger("hangar.claude_headless")

# Mesma regra de app.registry.sanitize_cwd (duplicada pelo mesmo motivo do adapters/claude.py).
_SANITIZE_RE = re.compile(r"[^A-Za-z0-9]")

# Chave interna do adapter no registro de providers. O `provider` da sessão continua "claude"
# (é Claude para o front, comandos, estatísticas e cotas); só o transporte é outro.
CHAVE = "claude-headless"

_TETO_INIT_S = 25.0        # os hooks de SessionStart rodam antes do initialize responder
_TETO_CTRL_S = 15.0
_MARCADOR_PAI = "HANGAR_HEADLESS_PARENT"

OPCOES_PERMISSAO = ["Permitir", "Negar"]


class _Sessao:
    def __init__(self, name: str, meta: dict):
        self.name = name
        self.meta = meta
        self.proc: asyncio.subprocess.Process | None = None
        self.leitor: asyncio.Task | None = None
        self.leitor_err: asyncio.Task | None = None
        self.state = "idle"
        self.label: str | None = None
        self.in_progress = False
        self.model: str | None = meta.get("model")
        self.effort: str | None = meta.get("effort")
        self.permission_mode: str | None = meta.get("permission_mode")
        self.context_window: int | None = meta.get("context_window")
        self.usage: dict | None = None
        self.cost: float | None = None
        self.limited = False
        self.limit_reset: str | None = None
        # Pedidos de permissão em aberto, na ordem em que chegaram: request_id -> request.
        self.pending: dict[str, dict] = {}
        self.question: dict | None = None      # AskUserQuestion pendente (payload pro front)
        self.previa = ""
        self.version = 0
        self.cond = asyncio.Condition()
        self.waiters: dict[str, asyncio.Future] = {}
        self.n_req = 0
        self.initialized = asyncio.Event()

    @property
    def sid(self) -> str:
        return self.meta["session_id"]

    @property
    def vivo(self) -> bool:
        return self.proc is not None and self.proc.returncode is None


class ClaudeHeadlessAdapter:
    provider = "claude"

    def __init__(self) -> None:
        self._sessions: dict[str, _Sessao] = {}
        self._delivery_locks: dict[str, asyncio.Lock] = {}

    # ── contrato Adapter ────────────────────────────────────────────────────────────────────

    def transcript_stream(self, path: str, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        Path(path).parent.mkdir(parents=True, exist_ok=True)
        return TranscriptTailer(path).follow(start_offset)

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        return self._state_stream(name)

    def spawn_command(self, cwd: str, session_id: str,
                      model: str | None = None, effort: str | None = None,
                      permission_mode: str | None = None) -> list[str]:
        return self._argv(session_id, resume=False, model=model, effort=effort, permission_mode=permission_mode)

    def transcript_path(self, cwd: str, session_id: str, config_dir: str | None = None) -> str:
        base = (Path(config_dir) / "projects") if config_dir else Path(settings.projects_dir)
        return str(base / _SANITIZE_RE.sub("-", cwd) / f"{session_id}.jsonl")

    def transcript_path_de(self, meta: dict) -> str:
        return self.transcript_path(meta["cwd"], meta["session_id"], meta.get("config_dir"))

    def delivery_lock(self, name: str) -> asyncio.Lock:
        return self._delivery_locks.setdefault(name, asyncio.Lock())

    async def deliverable(self, name: str) -> bool:
        sess = self._sessions.get(name)
        if sess is None:
            return hl_sessions.exists(name)
        return not (sess.in_progress or sess.pending or sess.question)

    async def send_prompt(self, name: str, text: str) -> str:
        sess = await self.ensure_running(name)
        if sess is None or not await self.deliverable(name):
            return "deferred"
        try:
            await self._write(sess, {
                "type": "user", "session_id": "", "parent_tool_use_id": None,
                "message": {"role": "user", "content": [{"type": "text", "text": text}]},
            })
        except Exception:
            _log.exception("claude headless: escrita no stdin falhou name=%s", name)
            return "deferred"
        sess.in_progress = True
        sess.state = "working"
        sess.label = None
        await self._notify(sess)
        return "sent"

    async def drain(self, name: str, path: str) -> int:
        async with self.delivery_lock(name):
            q = PromptQueue(name)
            if not any(e.get("delivered") is False for e in await asyncio.to_thread(q.load)):
                return 0
            sent = 0
            while True:
                claimed = await asyncio.to_thread(q.claim_undelivered, limit=1)
                if not claimed:
                    return sent
                entry = claimed[0]
                try:
                    result = await self.send_prompt(name, entry["text"])
                except Exception:
                    _log.exception("claude headless drain: falha entry=%s name=%s", entry.get("id"), name)
                    result = "deferred"
                if result != "sent":
                    # claim_undelivered marcou entregue de forma otimista; nada saiu, reverte.
                    try:
                        await asyncio.to_thread(q.set_delivered, entry["id"], False)
                    except OSError:
                        pass
                    return sent
                sent += 1

    # ── controles ───────────────────────────────────────────────────────────────────────────

    async def interrupt(self, name: str) -> bool:
        sess = self._sessions.get(name)
        if sess is None or not sess.vivo or not (sess.in_progress or sess.pending or sess.question):
            return False
        # Pedido pendente some junto com o turno: negar antes evita a tool rodar depois do Esc.
        for rid in list(sess.pending):
            await self._responder(sess, rid, {"behavior": "deny", "message": "Interrompido pelo usuário."})
        if sess.question:
            await self._responder(sess, sess.question["request_id"],
                                  {"behavior": "deny", "message": "Interrompido pelo usuário."})
            sess.question = None
        try:
            await self._ctrl(sess, "interrupt", esperar=False)
        except Exception:
            _log.exception("claude headless: interrupt falhou name=%s", name)
            return False
        return True

    async def select(self, name: str, option: int) -> bool:
        """Resposta ao pedido de permissão em aberto: 1 = permitir, 2 = negar."""
        sess = self._sessions.get(name)
        if sess is None or not sess.pending:
            return False
        rid, req = next(iter(sess.pending.items()))
        if option == 1:
            resposta = {"behavior": "allow", "updatedInput": req.get("input") or {}}
        else:
            resposta = {"behavior": "deny", "message": "Usuário recusou."}
        await self._responder(sess, rid, resposta)
        sess.pending.pop(rid, None)
        self._recalcular_estado(sess)
        await self._notify(sess)
        return True

    async def answer_questions(self, name: str, request_id: int | str | None, answers: list[dict]) -> None:
        sess = self._sessions.get(name)
        if sess is None or not sess.question:
            raise ValueError("nenhuma pergunta pendente")
        q = sess.question
        if request_id is not None and str(request_id) != str(q["request_id"]):
            raise ValueError("a pergunta mudou")
        perguntas = q["questions"]
        respostas: dict[str, str] = {}
        for i, item in enumerate(perguntas):
            a = answers[i] if i < len(answers) else None
            if not a:
                raise ValueError("responda a todas as perguntas")
            if a.get("kind") == "text":
                texto = (a.get("value") or "").strip()
            else:
                opcoes = item.get("options") or []
                idx = a.get("indices") or []
                rotulos = a.get("labels") or [opcoes[j]["label"] for j in idx if 0 <= j < len(opcoes)]
                texto = ", ".join(rotulos)
            if not texto:
                raise ValueError("responda a todas as perguntas")
            respostas[item["question"]] = texto
        await self._responder(sess, q["request_id"], {
            "behavior": "allow",
            "updatedInput": {"questions": perguntas, "answers": respostas},
        })
        sess.question = None
        self._recalcular_estado(sess)
        await self._notify(sess)

    async def set_permission_mode(self, name: str, mode: str) -> str:
        sess = await self.ensure_running(name)
        if sess is None:
            raise ValueError("sessão indisponível")
        r = await self._ctrl(sess, "set_permission_mode", mode=mode)
        sess.permission_mode = (r or {}).get("mode") or mode
        hl_sessions.update(name, permission_mode=sess.permission_mode)
        await self._notify(sess)
        return sess.permission_mode

    async def set_model(self, name: str, model: str | None, effort: str | None) -> None:
        sess = await self.ensure_running(name)
        if sess is None:
            raise ValueError("sessão indisponível")
        if model:
            await self._ctrl(sess, "set_model", model=model)
            sess.model = model
        if effort:
            # A CLI não tem controle de esforço em voo: fica no sidecar e vale no próximo processo.
            sess.effort = effort
        hl_sessions.update(name, model=sess.model, effort=sess.effort)
        await self._notify(sess)

    async def list_models(self, name: str) -> list[dict]:
        sess = await self.ensure_running(name)
        if sess is None:
            return []
        r = await self._ctrl(sess, "list_models")
        return list((r or {}).get("models") or [])

    # ── processo ────────────────────────────────────────────────────────────────────────────

    async def ensure_running(self, name: str) -> _Sessao | None:
        sess = self._sessions.get(name)
        if sess is not None and sess.vivo:
            return sess
        meta = hl_sessions.load(name)
        if meta is None:
            return None
        sess = _Sessao(name, meta)
        self._sessions[name] = sess
        try:
            await self._spawn(sess)
        except Exception:
            _log.exception("claude headless: não subiu name=%s", name)
            self._sessions.pop(name, None)
            raise
        return sess

    def _argv(self, sid: str, *, resume: bool, model=None, effort=None, permission_mode=None) -> list[str]:
        base = ["claude", "-p", "--output-format", "stream-json", "--input-format", "stream-json",
                "--verbose", "--include-partial-messages", "--permission-prompt-tool", "stdio",
                "--setting-sources", "user,project,local"]
        base += ["--resume", sid] if resume else ["--session-id", sid]
        # A CLI nasce no `permissions.defaultMode` do settings.json da conta, não num padrão dela;
        # passar o modo explícito é o que faz a sessão nascer no modo que o Hangar mostra.
        return base + model_args.args_de("claude", model, effort, permission_mode)

    async def _spawn(self, sess: _Sessao) -> None:
        meta = sess.meta
        transcript = self.transcript_path_de(meta)
        resume = Path(transcript).exists()
        argv = self._argv(sess.sid, resume=resume, model=sess.model, effort=sess.effort,
                          permission_mode=None if resume else sess.permission_mode)
        if meta.get("engine"):
            pre = ["hangar-engine", "--exec", meta["engine"]]
            if sess.model:
                pre += ["--model", sess.model]
                if sess.context_window:
                    pre += ["--context", str(sess.context_window)]
            argv = pre + ["--"] + argv
        env = dict(os.environ)
        env["CP_SESSION_NAME"] = sess.name
        env[_MARCADOR_PAI] = str(os.getpid())
        if meta.get("config_dir"):
            env["CLAUDE_CONFIG_DIR"] = meta["config_dir"]
        if shutil.which(argv[0]) is None:
            raise RuntimeError(f"binário não encontrado: {argv[0]}")
        sess.proc = await asyncio.create_subprocess_exec(
            *argv, cwd=meta["cwd"], env=env,
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            start_new_session=True, limit=16 << 20)
        sess.leitor = asyncio.create_task(self._ler(sess))
        sess.leitor_err = asyncio.create_task(self._ler_err(sess))
        _log.info("claude headless: subiu name=%s pid=%s resume=%s", sess.name, sess.proc.pid, resume)
        try:
            await asyncio.wait_for(self._ctrl(sess, "initialize"), _TETO_INIT_S)
        except asyncio.TimeoutError:
            _log.warning("claude headless: initialize sem resposta em %.0fs name=%s", _TETO_INIT_S, sess.name)
        sess.initialized.set()

    async def _ler(self, sess: _Sessao) -> None:
        assert sess.proc and sess.proc.stdout
        try:
            while True:
                linha = await sess.proc.stdout.readline()
                if not linha:
                    break
                try:
                    ev = json.loads(linha)
                except ValueError:
                    continue
                try:
                    await self._on_event(sess, ev)
                except Exception:
                    _log.exception("claude headless: evento mal digerido name=%s tipo=%s", sess.name, ev.get("type"))
        finally:
            rc = await sess.proc.wait() if sess.proc else None
            _log.info("claude headless: processo saiu name=%s rc=%s", sess.name, rc)
            for fut in sess.waiters.values():
                if not fut.done():
                    fut.set_exception(RuntimeError("processo encerrou"))
            sess.waiters.clear()
            sess.in_progress = False
            sess.pending.clear()
            sess.question = None
            sess.state = "dead"
            await CodexPreviewSource.get(sess.name).push("")
            await self._notify(sess)

    async def _ler_err(self, sess: _Sessao) -> None:
        assert sess.proc and sess.proc.stderr
        while True:
            linha = await sess.proc.stderr.readline()
            if not linha:
                return
            _log.debug("claude headless stderr name=%s: %s", sess.name, linha.decode(errors="replace").rstrip())

    async def _write(self, sess: _Sessao, obj: dict) -> None:
        if not sess.vivo or sess.proc is None or sess.proc.stdin is None:
            raise RuntimeError("processo não está vivo")
        sess.proc.stdin.write((json.dumps(obj) + "\n").encode())
        await sess.proc.stdin.drain()

    async def _ctrl(self, sess: _Sessao, subtype: str, *, esperar: bool = True, **req) -> dict | None:
        sess.n_req += 1
        rid = f"hangar_{sess.n_req}"
        fut: asyncio.Future | None = None
        if esperar:
            fut = asyncio.get_running_loop().create_future()
            sess.waiters[rid] = fut
        await self._write(sess, {"type": "control_request", "request_id": rid,
                                 "request": {"subtype": subtype, **req}})
        if fut is None:
            return None
        try:
            return await asyncio.wait_for(fut, _TETO_CTRL_S if subtype != "initialize" else _TETO_INIT_S)
        finally:
            sess.waiters.pop(rid, None)

    async def _responder(self, sess: _Sessao, rid: str, resposta: dict) -> None:
        await self._write(sess, {"type": "control_response",
                                 "response": {"subtype": "success", "request_id": rid, "response": resposta}})

    # ── eventos do stdout ──────────────────────────────────────────────────────────────────

    async def _on_event(self, sess: _Sessao, ev: dict) -> None:
        t = ev.get("type")
        if t == "control_response":
            r = ev.get("response") or {}
            fut = sess.waiters.get(r.get("request_id"))
            if fut is not None and not fut.done():
                if r.get("subtype") == "error":
                    fut.set_exception(RuntimeError(r.get("error") or "control_request recusado"))
                else:
                    fut.set_result(r.get("response") or {})
            return
        if t == "system":
            await self._on_system(sess, ev)
            return
        if t == "stream_event":
            await self._on_stream(sess, ev.get("event") or {})
            return
        if t == "assistant":
            blocos = (ev.get("message") or {}).get("content") or []
            tools = [b.get("name") for b in blocos if isinstance(b, dict) and b.get("type") == "tool_use"]
            if tools:
                sess.label = f"{tools[-1]}…"
            if any(isinstance(b, dict) and b.get("type") == "text" for b in blocos):
                # O bloco fechou: o .jsonl já tem a mensagem, a prévia sai de cena.
                sess.previa = ""
                await CodexPreviewSource.get(sess.name).push("")
            await self._notify(sess)
            return
        if t == "user":
            sess.label = None
            await self._notify(sess)
            return
        if t in ("control_request", "sdk_control_request"):
            await self._on_control_request(sess, ev)
            return
        if t == "control_cancel_request":
            rid = str(ev.get("request_id"))
            sess.pending.pop(rid, None)
            if sess.question and str(sess.question["request_id"]) == rid:
                sess.question = None
            self._recalcular_estado(sess)
            await self._notify(sess)
            return
        if t == "result":
            sess.in_progress = False
            sess.pending.clear()
            sess.question = None
            sess.label = None
            sess.previa = ""
            if isinstance(ev.get("total_cost_usd"), (int, float)):
                sess.cost = float(ev["total_cost_usd"])
            u = ev.get("usage")
            # Turno interrompido vem com uso zerado: o contexto anterior continua valendo.
            if isinstance(u, dict) and any(u.get(k) for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")):
                sess.usage = u
                mu = ev.get("modelUsage") or {}
                for m, dados in mu.items():
                    if isinstance(dados, dict) and dados.get("contextWindow"):
                        sess.model = sess.model or m
                        sess.context_window = int(dados["contextWindow"])
            self._recalcular_estado(sess)
            await CodexPreviewSource.get(sess.name).push("")
            await self._notify(sess)
            return
        if t == "rate_limit_event":
            info = ev.get("rate_limit_info") or {}
            sess.limited = info.get("status") not in (None, "allowed")
            sess.limit_reset = _hora_local(info.get("resetsAt")) if sess.limited else None
            await self._notify(sess)
            return

    async def _on_system(self, sess: _Sessao, ev: dict) -> None:
        sub = ev.get("subtype")
        if sub == "init":
            sid = ev.get("session_id")
            if sid and sid != sess.sid:
                # /clear (ou resume que trocou de id): o transcript agora é outro arquivo. O
                # sidecar é a fonte da lista, e o jsonl_watcher do SSE faz o reset a partir dela.
                sess.meta = hl_sessions.update(sess.name, session_id=sid) or {**sess.meta, "session_id": sid}
            if ev.get("model"):
                sess.model = ev["model"]
            if ev.get("permissionMode"):
                sess.permission_mode = ev["permissionMode"]
            sess.initialized.set()
        elif sub == "status":
            if ev.get("permissionMode"):
                sess.permission_mode = ev["permissionMode"]
            if ev.get("status") == "requesting" and sess.in_progress:
                sess.label = "Pensando…"
        elif sub == "thinking_tokens":
            if sess.in_progress:
                sess.label = "Pensando…"
        elif sub and sub.startswith("compact"):
            sess.label = "Compactando…"
        else:
            return
        await self._notify(sess)

    async def _on_stream(self, sess: _Sessao, e: dict) -> None:
        tipo = e.get("type")
        if tipo == "content_block_start":
            bloco = e.get("content_block") or {}
            if bloco.get("type") == "text":
                sess.previa = ""
                sess.label = None
            elif bloco.get("type") in ("tool_use", "server_tool_use", "mcp_tool_use"):
                sess.label = f"{bloco.get('name') or 'tool'}…"
            elif bloco.get("type") == "thinking":
                sess.label = "Pensando…"
            await self._notify(sess)
        elif tipo == "content_block_delta":
            d = e.get("delta") or {}
            if d.get("type") == "text_delta" and d.get("text"):
                sess.previa += d["text"]
                await CodexPreviewSource.get(sess.name).push(sess.previa)
        elif tipo == "message_start":
            if not sess.in_progress:
                # Turno iniciado por outro caminho (steer, hook): o estado acompanha o stream.
                sess.in_progress = True
                sess.state = "working"
                await self._notify(sess)

    async def _on_control_request(self, sess: _Sessao, ev: dict) -> None:
        rid = str(ev.get("request_id"))
        req = ev.get("request") or {}
        sub = req.get("subtype")
        if sub != "can_use_tool":
            # Subtype que não tratamos: responder vazio destrava a CLI (mesma escolha do MonoCode).
            await self._responder(sess, rid, {})
            return
        if req.get("tool_name") == "AskUserQuestion":
            perguntas = (req.get("input") or {}).get("questions") or []
            sess.question = {"provider": "claude", "request_id": rid, "questions": perguntas}
        else:
            sess.pending[rid] = req
        self._recalcular_estado(sess)
        await self._notify(sess)

    def _recalcular_estado(self, sess: _Sessao) -> None:
        if not sess.vivo:
            sess.state = "dead"
        elif sess.pending or sess.question:
            sess.state = "awaiting_input"
        elif sess.in_progress:
            sess.state = "working"
        else:
            sess.state = "idle"

    async def _notify(self, sess: _Sessao) -> None:
        async with sess.cond:
            sess.version += 1
            sess.cond.notify_all()

    # ── estado pro SSE ─────────────────────────────────────────────────────────────────────

    def _permissao_texto(self, req: dict) -> str:
        tool = req.get("tool_name") or "ferramenta"
        inp = req.get("input") or {}
        detalhe = req.get("description") or inp.get("command") or inp.get("file_path") or inp.get("path") or ""
        detalhe = str(detalhe)
        if len(detalhe) > 200:
            detalhe = detalhe[:200] + "…"
        return f"Permitir {tool}? {detalhe}".strip()

    def status_line(self, sess: _Sessao) -> str | None:
        parts: list[str] = []
        if sess.model:
            seg = f"🤖 {sess.model}"
            if sess.effort:
                seg += f" ({sess.effort})"
            parts.append(seg)
        u = sess.usage or {}
        usado = (u.get("input_tokens") or 0) + (u.get("cache_creation_input_tokens") or 0) + (u.get("cache_read_input_tokens") or 0)
        if usado and sess.context_window:
            parts.append(f"💬 {_fmt_tok(usado)}/{_fmt_tok(u.get('output_tokens') or 0)} "
                         f"{_fmt_tok(usado)}/{_fmt_tok(sess.context_window)}")
        if sess.cost is not None:
            parts.append(f"💵 ${sess.cost:.2f}")
        return " │ ".join(parts) or None

    def _evento(self, sess: _Sessao) -> StateEvent:
        question = options = None
        if sess.pending and not sess.question:
            question = self._permissao_texto(next(iter(sess.pending.values())))
            options = list(OPCOES_PERMISSAO)
        return StateEvent(session=sess.name, state=sess.state, label=sess.label,
                          question=question, options=options,
                          status_line=self.status_line(sess),
                          claude_permission_mode=sess.permission_mode,
                          limited=sess.limited, limit_reset=sess.limit_reset,
                          codex_question=sess.question)

    def snapshot(self, name: str) -> StateEvent | None:
        """Estado atual sem abrir stream (lista/board). None = sem processo vivo (sessão parada)."""
        sess = self._sessions.get(name)
        if sess is None or not sess.vivo:
            return None
        return self._evento(sess)

    async def _state_stream(self, name: str) -> AsyncIterator[StateEvent]:
        while True:
            if not hl_sessions.exists(name):
                yield StateEvent(session=name, state="dead")
                return
            sess = self._sessions.get(name)
            if sess is None or not sess.vivo:
                # Sessão parada (o processo morre com o backend): ociosa até o próximo prompt
                # subir outro. Não sobe aqui — abrir o chat não deve custar um processo.
                meta = hl_sessions.load(name) or {}
                yield StateEvent(session=name, state="idle",
                                 claude_permission_mode=meta.get("permission_mode"),
                                 status_line=(f"🤖 {meta['model']}" if meta.get("model") else None))
                while True:
                    await asyncio.sleep(1.0)
                    sess = self._sessions.get(name)
                    if sess is not None and sess.vivo:
                        break
                    if not hl_sessions.exists(name):
                        yield StateEvent(session=name, state="dead")
                        return
            last = -1
            while True:
                async with sess.cond:
                    await sess.cond.wait_for(lambda: sess.version != last)
                    last = sess.version
                    ev = self._evento(sess)
                if ev.state == "dead":
                    # Processo caiu (ou o backend o matou). Com sidecar, a sessão segue existindo
                    # e volta ociosa (laço de fora); sem sidecar (kill), morreu de vez.
                    if self._sessions.get(name) is sess:
                        self._sessions.pop(name, None)
                    break
                yield ev

    # ── encerramento ───────────────────────────────────────────────────────────────────────

    def close_sync(self, name: str) -> None:
        """SIGTERM no grupo do processo (chamado do registry.kill, sync). O leitor vê o EOF e
        fecha o resto; o sidecar é apagado por quem chamou."""
        sess = self._sessions.pop(name, None)
        CodexPreviewSource._sources.pop(name, None)
        if sess is None or sess.proc is None or sess.proc.returncode is not None:
            return
        try:
            os.killpg(os.getpgid(sess.proc.pid), signal.SIGTERM)
        except (ProcessLookupError, PermissionError, OSError):
            pass

    def rename(self, old: str, new: str) -> None:
        sess = self._sessions.pop(old, None)
        if sess is not None:
            sess.name = new
            sess.meta["name"] = new
            self._sessions[new] = sess
        lock = self._delivery_locks.pop(old, None)
        if lock is not None:
            self._delivery_locks[new] = lock


def _hora_local(epoch) -> str | None:
    if not isinstance(epoch, (int, float)):
        return None
    return time.strftime("%H:%M", time.localtime(epoch))


def matar_orfaos() -> int:
    """Processos `claude` de um backend anterior (marcador de env com pid que não existe mais).
    Chamado na subida: sem isto, reiniciar o serviço deixava um `claude` por sessão pendurado."""
    mortos = 0
    proc = Path("/proc")
    if not proc.exists():
        return 0
    for p in proc.iterdir():
        if not p.name.isdigit():
            continue
        try:
            env = (p / "environ").read_bytes()
        except OSError:
            continue
        marca = f"{_MARCADOR_PAI}=".encode()
        for item in env.split(b"\0"):
            if item.startswith(marca):
                pai = item[len(marca):].decode(errors="replace")
                if pai.isdigit() and not (proc / pai).exists():
                    try:
                        os.kill(int(p.name), signal.SIGTERM)
                        mortos += 1
                    except OSError:
                        pass
                break
    return mortos
