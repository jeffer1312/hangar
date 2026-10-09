"""Serviços de dados do ator; nenhum cliente CLI ou autoridade paralela de fila."""
from __future__ import annotations

import asyncio
import json
import threading
import time
import uuid
from pathlib import Path

from app import diag

_PATCH = {
    "claude": {"session_id", "cwd", "model", "effort", "permission_mode", "previous_non_plan", "context_window", "problema", "cano"},
    "codex": {"thread_id", "rollout_path", "cwd", "model", "effort", "mode", "service_tier", "skipped_async_questions", "problema", "cano",
              "permission_mode"},
}
_unknown_guard = threading.Lock()
_mutation_locks = {}
_quota_lock = threading.Lock()
_quota_cache = {}


async def execute(kind: str, payload: dict, metadata: dict) -> dict:
    task = asyncio.create_task(asyncio.to_thread(run, kind, payload, metadata))
    try:
        data = await asyncio.shield(task)
        return {"ok": True, "data": data}
    except asyncio.CancelledError:
        try:
            await asyncio.shield(task)
        finally:
            raise
    except Exception as exc:
        diag.registrar("runtime.policy_failed", "erro", codigo=type(exc).__name__)
        return {"ok": False, "error_type": type(exc).__name__}


def _quota(metadata, fresh=False):
    from app import cotas
    root = Path(metadata.get("config_dir") or Path.home() / ".claude").resolve()
    with _quota_lock:
        cached = _quota_cache.get(root)
        if not fresh and cached is not None and time.monotonic() - cached[0] < 300:
            return cached[1]
        try:
            value = next((account.model_dump() for account in cotas.listar_cotas()
                if account.provedor == "claude" and ":" in account.id
                and Path(account.id.split(":", 1)[1]).resolve() == root), None)
        except Exception as exc:
            diag.registrar("runtime.quota_failed", "erro", codigo=type(exc).__name__)
            value = cached[1] if cached else None
        _quota_cache[root] = (time.monotonic(), value)
        return value


def quota_windows(config_dir):
    """Janelas da conta que o ator Rust põe na linha de status (as por modelo ficam fora).
    Sem cache de 300 s aqui: o do ator Rust é o único."""
    quota = _quota({"config_dir": config_dir}, fresh=True)
    return [window for window in (quota or {}).get("janelas", []) if not window.get("por_modelo")]


def native_message(payload, metadata):
    from app import api, uds_messaging
    text = payload.get("text")
    operation_id = metadata.get("operation_id")
    if not isinstance(text, str) or not isinstance(operation_id, str) or not operation_id:
        raise ValueError("recado sem operação registrada")
    sender, _ = uds_messaging.separar_prefixo(text)
    if sender is None:
        return {"outcome": "not_written", "reason": "ordinary_input"}
    endpoint = uds_messaging.socket_da_sessao(metadata.get("session_id", ""), metadata.get("config_dir"))
    if not endpoint:
        return {"outcome": "not_written", "reason": "no_socket"}
    mode = api._classe_modo(sender, metadata["name"], metadata.get("config_dir"), metadata.get("jsonl"))
    mid = str(uuid.uuid5(uuid.NAMESPACE_URL, "hangar:" + metadata["key"] + ":" + operation_id))
    validate = metadata.get("validate")
    if validate is None:
        raise RuntimeError("recado sem confirmação de posse")
    validate()
    try:
        uds_messaging.enviar(endpoint, text, sender, mode, msg_id=mid)
    except Exception:
        return {"outcome": "unknown", "msg_id": mid}
    return {"outcome": "written", "msg_id": mid}


def demote_awaiting(sids: list[str]) -> None:
    """`hooks.demote_awaiting`: o mapa de marcadores e o registro nativo em memória são do Python
    (a prévia e as transições leem deles), então o Rust pede aqui em vez de regravar o sidecar."""
    from app import hook_state
    for sid in sids:
        hook_state.hook_state.demote_awaiting(sid)


def state_service_sync(kind: str, name: str, payload: dict) -> dict:
    """Serviços que o `Monitor` do Rust pede por nome de sessão (não por posse de ator)."""
    if kind == "permission.observe":
        from app import permission_mode
        sid, mode = payload.get("sid"), payload.get("mode")
        if not isinstance(sid, str) or not sid or mode not in permission_mode.ORDEM_CANONICA:
            raise ValueError("observação de permissão inválida")
        # A memória é por session-id; a operação controlada, pelo nome da sessão.
        mode, previous = permission_mode.observar_ou_confirmado(sid, mode, sessao=name)
        return {"mode": mode, "previous_non_plan": previous}
    if kind == "session.dead":
        from app import plugin_bridge
        from app.adapters.claude_headless.sessions import em_troca
        from app.state import forget_frame
        # Conferido na hora: a troca de conta mata o tmux antes de o fato chegar ao Rust.
        if em_troca(name):
            return {"result": "em_troca"}
        plugin_bridge.esquecer(name)
        forget_frame(name)
        return {"result": "ok"}
    raise ValueError("serviço não permitido")


async def state_service(kind: str, name: str, payload: dict) -> dict:
    if kind == "session.deliverable":
        # O drain do adapter passa pelo `prepare_session`, que abre o executor se ele não existe.
        from app import api
        from app.adapters import get_adapter
        info = await api._cached_info(name)
        if info is None or info.provider != "claude" or info.headless or not info.jsonl:
            raise ValueError("sessão Claude com terminal não encontrada")
        return {"sent": await get_adapter("claude").drain(name, info.jsonl)}
    return await asyncio.to_thread(state_service_sync, kind, name, payload)


def _sessions(provider):
    if provider == "claude":
        from app.adapters.claude_headless import sessions
    else:
        from app.adapters.codex import sessions
    return sessions


def launch_env(metadata: dict) -> dict:
    """Comando e ambiente do processo que o Rust sobe dentro do cano. O ambiente é segredo: vai na
    resposta e em nenhum outro lugar."""
    if metadata["provider"] != "codex":
        raise ValueError("subida pelo Rust ainda só para o Codex")
    import shutil
    from app.adapters.codex import sem_terminal, sessions
    # O registro em memória não vê o que o Rust gravou depois de abrir (modo trocado): vale o arquivo.
    current = sessions.load(metadata["name"])
    if current and current.get("key") == metadata["key"]:
        metadata = {**metadata, **current}
    env = sem_terminal._ambiente(metadata)
    program = sem_terminal.argv(metadata)
    # O PATH da sessão decide, como no `subir`; no Windows o `which` acha o `.cmd` pelo PATHEXT.
    resolved = shutil.which(program[0], path=env.get("PATH"))
    if resolved is None:
        return {"error": "codex_ausente"}
    return {"program": [resolved, *program[1:]], "env": env, "cano_extra": {}}


def clear_cano(payload: dict, metadata: dict) -> dict:
    """Tira o `cano` do arquivo da sessão só se ele ainda for o processo `pid`; arquivo apagado fica apagado."""
    pid = payload.get("pid")
    if type(pid) is not int or set(payload) != {"pid"}:
        raise ValueError("pid do cano inválido")
    validate = metadata.get("validate")
    if validate is None:
        raise RuntimeError("alteração sem confirmação de posse")
    sessions = _sessions(metadata["provider"])
    with _unknown_guard:
        lock = _mutation_locks.setdefault(metadata["key"], threading.Lock())
    with lock:
        validate()
        current = sessions.load(metadata["name"])
        if not current or current.get("key") != metadata["key"] or (current.get("cano") or {}).get("pid") != pid:
            return {"cleared": False}
        return {"cleared": sessions.update(metadata["name"], cano=None) is not None}


def run(kind: str, payload: dict, metadata: dict) -> dict:
    if not isinstance(payload, dict) or not isinstance(metadata, dict):
        raise ValueError("serviço com dados inválidos")
    provider = metadata.get("provider")
    if provider not in _PATCH:
        raise ValueError("provedor fora do runtime")
    if kind in {"terminal_facts", "terminal_publish", "terminal_plugin_control"}:
        if provider != "claude" or not metadata.get("terminal"):
            raise ValueError("serviço terminal fora do vínculo Claude")
        from app import runtime_terminal
        return {"terminal_facts":runtime_terminal.facts, "terminal_publish":runtime_terminal.publish,
            "terminal_plugin_control":runtime_terminal.plugin_control}[kind](payload, metadata)
    if kind == "native_message":
        return native_message(payload, metadata)
    if kind == "launch_env":
        return launch_env(metadata)
    if kind == "session.clear_cano":
        return clear_cano(payload, metadata)
    if kind == "session.patch_meta":
        if set(payload) - _PATCH[provider]:
            raise ValueError("campo fora do catálogo do sidecar")
        validate = metadata.get("validate")
        if validate is None:
            raise RuntimeError("alteração sem confirmação de posse")
        with _unknown_guard:
            lock = _mutation_locks.setdefault(metadata["key"], threading.Lock())
        with lock:
            validate()
            state_path = metadata.get("state_path")
            view = (json.loads(Path(state_path).read_bytes())["runtime_state"].get("view") or {}) if state_path else {}
            for key, value in payload.items():
                source = "conversation" if key == "session_id" else key
                if source in view and view[source] != value:
                    return {"updated": False, "stale": True}
            sessions = _sessions(provider)
            current = sessions.load(metadata["name"])
            if not current or current.get("key") != metadata["key"]:
                raise RuntimeError("sidecar de outra vida")
            # Fast é da conversa: o sidecar já em outra thread não herda a escolha da anterior.
            if ("service_tier" in payload and "thread_id" not in payload and view.get("thread_id")
                    and current.get("thread_id") != view["thread_id"]):
                return {"updated": False, "stale": True}
            updated = sessions.update(metadata["name"], **payload)
            if updated is None:
                raise RuntimeError("sidecar desapareceu durante a alteração")
        if provider == "claude" and payload.get("session_id"):
            from app import claude_customizations
            try:
                claude_customizations.remember(payload["session_id"], updated.get("claude_settings"))
            except claude_customizations.CustomizationsError as exc:
                diag.registrar("claude.customizations_not_saved", "aviso", codigo=exc.code)
                with lock:
                    validate()
                    current = sessions.load(metadata["name"])
                    if current and current.get("key") == metadata["key"]:
                        sessions.update(metadata["name"], problema=[exc.code, exc.detail])
                return {"updated": True, "customizations_persisted": False, "warning": exc.detail}
        return {"updated": True}
    raise ValueError("serviço não permitido")
