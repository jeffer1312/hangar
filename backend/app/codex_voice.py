"""Sinalização WebRTC; a conversa organiza pedidos antes de entregá-los à sessão."""
import asyncio
import contextlib
import json
import logging

from fastapi import HTTPException, WebSocket, WebSocketDisconnect

from app import runtime_config
from app.auth import require_auth
from app.codex_voice_broker import VoiceBroker
from app.termsock import _origem_aceita

_log = logging.getLogger(__name__)
VOICES = ("alloy", "arbor", "ash", "ballad", "breeze", "cedar", "coral", "cove",
          "echo", "ember", "juniper", "maple", "marin", "sage", "shimmer", "sol",
          "spruce", "vale", "verse")


def forward(sess: dict, notification: dict) -> None:
    queue = sess.get("voice_events")
    method = notification.get("method")
    item = (notification.get("params") or {}).get("item") or {}
    relevante = (method == "turn/completed"
                 or method in {"item/started", "item/completed"} and item.get("type") in {"userMessage", "agentMessage"}
                 or method == "item/tool/requestUserInput" or method.endswith("/requestApproval"))
    if queue is not None and relevante:
        if queue.full():
            _log.warning("codex voice: fila de resultados cheia")
            queue.get_nowait()
        queue.put_nowait(notification)


async def voice_ws(ws: WebSocket, name: str, adapter, provider_of=None) -> None:
    try:
        require_auth(ws)
    except HTTPException:
        await ws.close(code=1008)
        return
    origin = ws.headers.get("origin")
    if origin and not _origem_aceita(origin, ws.headers.get("host")):
        await ws.close(code=1008)
        return
    if runtime_config.get("codex_voice_beta") is not True:
        await ws.close(code=1008)
        return
    if provider_of is not None and await asyncio.to_thread(provider_of, name) != "codex":
        await ws.close(code=1008)
        return
    await ws.accept()
    sess = None
    events = asyncio.Queue(maxsize=32)
    tasks = []
    broker = None
    try:
        client = await adapter.ensure_running(name)
        if client is None:
            await ws.send_json({"type": "error", "code": "unavailable"})
            return
        sess = adapter._sessions[name]
        if sess.get("voice_events") is not None:
            await ws.send_json({"type": "error", "code": "busy"})
            return
        sess["voice_events"] = events
        thread_id = sess["thread_id"]
        ready = asyncio.Event()

        async def keep_events():
            async for state in adapter.state_monitor(name, lambda: thread_id):
                ready.set()
                if state.state == "dead":
                    return

        monitor = asyncio.create_task(keep_events())
        tasks.append(monitor)
        await asyncio.wait_for(ready.wait(), timeout=15)
        if monitor.done():
            await monitor
            raise RuntimeError("session ended")
        await ws.send_json({"type": "ready", "voices": VOICES})
        raw = await asyncio.wait_for(ws.receive_text(), timeout=45)
        if len(raw) > 70000:
            raise ValueError("oversized offer")
        body = json.loads(raw)
        if not isinstance(body, dict) or set(body) - {"type", "sdp", "voice"}:
            raise ValueError("invalid offer")
        sdp, voice = body.get("sdp"), body.get("voice")
        if (body.get("type") != "start" or not isinstance(sdp, str)
                or not sdp.startswith("v=0") or len(sdp) > 65536
                or voice is not None and voice not in VOICES):
            raise ValueError("invalid offer")
        broker = VoiceBroker(adapter, name, sess, ws.send_json)
        await broker.start(sdp, voice)
        tasks.append(broker.reader)

        async def send_results():
            while True:
                await broker.target_event(await events.get())

        async def receive():
            while True:
                message = await asyncio.wait_for(ws.receive_text(), timeout=35)
                if message == "stop":
                    return
                if message != "ping":
                    raise ValueError("invalid control")
                if runtime_config.get("codex_voice_beta") is not True:
                    await ws.send_json({"type": "error", "code": "disabled"})
                    return
                if adapter._sessions.get(name) is not sess or sess["thread_id"] != thread_id:
                    return
                await ws.send_json({"type": "pong"})

        tasks.extend([asyncio.create_task(send_results()), asyncio.create_task(receive())])
        done, _ = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        for task in done:
            await task
    except WebSocketDisconnect:
        pass
    except Exception as exc:
        _log.warning("codex voice: falha name=%s type=%s", name, type(exc).__name__)
        with contextlib.suppress(Exception):
            await ws.send_json({"type": "error", "code": "failed"})
    finally:
        for task in tasks[1:]:
            task.cancel()
        await asyncio.gather(*tasks[1:], return_exceptions=True)
        if broker:
            try:
                await broker.close()
            except Exception:
                _log.warning("codex voice: encerramento do organizador não confirmado name=%s", name)
        if sess is not None and sess.get("voice_events") is events:
            sess.pop("voice_events", None)
        for task in tasks[:1]:
            task.cancel()
        await asyncio.gather(*tasks[:1], return_exceptions=True)
        with contextlib.suppress(Exception):
            await ws.close()
