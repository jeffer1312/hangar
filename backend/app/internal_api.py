# backend/app/internal_api.py
"""Rotas internas que só o hangar-server, filho deste backend na mesma máquina, consome."""
import asyncio
import secrets
import json
import copy

from fastapi import APIRouter, Depends, HTTPException, Request
from sse_starlette.sse import EventSourceResponse

from app import diag
from app.mensagens import erro
from app.models import session_key
from app.sse import merged_events

_LOOPBACK = {"127.0.0.1", "::1"}

# Só na memória: no os.environ ele iria para toda sessão que o backend sobe.
_secret: str | None = None


def set_secret(value: str | None) -> None:
    global _secret
    _secret = value or None


def require_internal(request: Request) -> None:
    # 404 em toda recusa: quem não é o hangar-server não descobre que a rota existe. O segredo nasce
    # a cada subida e o repasse do hangar-server manda o IP real no X-Forwarded-For, então quem
    # chega de fora pela porta pública cai no IP antes do segredo.
    secret = _secret
    ip = request.client.host if request.client else None
    given = request.headers.get("x-hangar-internal", "")
    if secret is None or ip not in _LOOPBACK or not secrets.compare_digest(given.encode(), secret.encode()):
        # Pedido local COM o cabeçalho é o hangar-server: a recusa desliga o atalho dele sem
        # erro visível, então vai ao diário (nunca o valor do segredo).
        if ip in _LOOPBACK and "x-hangar-internal" in request.headers:
            diag.registrar("internal.recusado", "aviso",
                           codigo="sem_segredo" if secret is None else "segredo_errado")
        raise HTTPException(status_code=404)


def info_payload(name: str, provider: str, jsonl: str | None) -> dict:
    """O `InternalInfo` do Rust: a rota `info` e o evento `info` do side-events usam este mesmo."""
    from app.adapters import chave_de
    from app.pqueue import PromptQueue

    return {
        # Chave do adapter: o Claude sem terminal vem como "claude-headless".
        "provider": chave_de(name, provider),
        "jsonl": jsonl,
        "session_key": session_key(jsonl) if jsonl else "",
        # Tudo que o merged_history do Rust precisa além do transcript.
        "history": {"queue": str(PromptQueue(name).path)},
    }


router = APIRouter(prefix="/internal", dependencies=[Depends(require_internal)], include_in_schema=False)
_policy_calls = {}


@router.post("/runtime/policy")
async def runtime_policy(request: Request):
    from app import runtime_coordinator, runtime_policy as service
    coordinator = runtime_coordinator.current()
    instance = request.headers.get("x-hangar-runtime-instance", "")
    if coordinator is None or not coordinator.instance or not secrets.compare_digest(instance, coordinator.instance):
        raise HTTPException(404)
    raw = bytearray()
    async for chunk in request.stream():
        raw.extend(chunk)
        if len(raw) > (32 << 20) + 1024:
            raise HTTPException(413)
    try:
        body = json.loads(raw)
        if (not isinstance(body, dict) or set(body) != {"key", "generation", "request_id", "phase_id", "kind", "payload"}
                or not isinstance(body["key"], str) or type(body["generation"]) is not int
                or type(body["request_id"]) not in (int, str) or not isinstance(body["phase_id"], str)
                or not isinstance(body["kind"], str) or not isinstance(body["payload"], dict)):
            raise ValueError("serviço inválido")
        slot = coordinator.slots[body["key"]]
    except (KeyError, TypeError, ValueError):
        raise HTTPException(400) from None

    def validate():
        with slot.guard:
            if (coordinator.instance != instance or slot.binding.key != body["key"]
                    or slot.binding.generation != body["generation"]
                    or slot.phase not in {runtime_coordinator.Phase.Rust, runtime_coordinator.Phase.PreparingRust}
                    or slot.lease is not None and not slot.lease.closed):
                raise RuntimeError("serviço de outra posse ou geração")
        state = json.loads(slot.binding.state_path.read_bytes())
        operation = state["operations"][body["phase_id"]]
        if (state["owner_key"] != body["key"] or state["generation"] != body["generation"]
                or operation["status"] != "dispatching" or operation["payload"].get("kind") != body["kind"]
                or type(operation["payload"].get("request_id")) is not type(body["request_id"])
                or operation["payload"].get("request_id") != body["request_id"]
                or operation["payload"].get("payload") != body["payload"]):
            raise RuntimeError("serviço sem tentativa registrada")

    validate()
    key = (instance, body["key"], body["generation"], body["phase_id"])
    for old in tuple(_policy_calls):
        if old[0] != instance:
            _policy_calls.pop(old, None)
    if key not in _policy_calls:
        metadata = copy.deepcopy(slot.binding.meta)
        metadata.update(provider=slot.binding.provider, name=slot.binding.name, key=slot.binding.key,
                        generation=slot.binding.generation, jsonl=slot.binding.jsonl,
                        state_path=str(slot.binding.state_path),
                        operation_id=body["request_id"] if isinstance(body["request_id"], str) else body["phase_id"], validate=validate)
        async def perform():
            validate()
            with slot.guard:
                slot.active += 1
            try:
                return await service.execute(body["kind"], body["payload"], metadata)
            finally:
                with slot.guard:
                    slot.active -= 1
                coordinator._signal(slot)
        _policy_calls[key] = asyncio.create_task(perform())
    return await asyncio.shield(_policy_calls[key])


@router.get("/workspace/context")
async def workspace_context(name: str | None = None) -> dict:
    """Só metadados; o consumidor privado não consulta novamente este registro."""
    from app import api
    from app.fs import allowed_roots

    infos = await asyncio.to_thread(api._guardar_snap)
    info = next((s for s in infos if s.name == name), None) if name else None
    if name and (info is None or not info.cwd):
        info = await api._cached_info(name)
        if info is None or not info.cwd:
            raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    return {
        "roots": [str(root) for root in allowed_roots()],
        "sessions": [{"name": s.name, "cwd": s.cwd} for s in infos if s.cwd],
        "session": {"name": info.name, "cwd": info.cwd, "jsonl": info.jsonl} if info else None,
    }


@router.get("/sessions/{name}/info")
async def session_info(name: str) -> dict:
    # Import tardio: api.py importa este módulo no topo.
    from app import api

    info = await api._cached_info(name)
    if info is None:
        raise HTTPException(status_code=404)
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is not None and coordinator.managed_queue(name):
        import uuid
        try:
            await coordinator.op(name, {"kind":"ensure_projection"}, uuid.uuid4().hex)
        except Exception as exc:
            diag.registrar("runtime.history_failed", "erro", sessao=name, codigo=type(exc).__name__)
            raise HTTPException(503) from None
    return info_payload(name, info.provider, info.jsonl)


@router.get("/sessions/{name}/side-events")
async def side_events(name: str, app: int = 0):
    """Uma conexão por sessão para o hangar-server, que reparte estado, prévia, perguntas e fila
    entre os aparelhos. `app=1`: há ao menos um aparelho do dono no chat (push fica calado)."""
    from app import api as _api   # api monta este roteador
    sessions = await asyncio.to_thread(_api.registry.list)
    info = next((s for s in sessions if s.name == name), None)
    if not info or not info.jsonl:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente",
                                             "session or transcript not found"))
    return EventSourceResponse(
        merged_events(name, info.jsonl, provider=info.provider, count_app=bool(app), side=True),
        send_timeout=30)
