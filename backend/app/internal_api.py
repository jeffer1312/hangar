# backend/app/internal_api.py
"""Rotas internas que só o hangar-server, filho deste backend na mesma máquina, consome."""
import asyncio
import secrets
import json
import copy
import re
import time

from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel, ConfigDict
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


@router.post("/accounts/facts")
async def account_facts(request: Request):
    from dataclasses import asdict
    from pathlib import Path
    from app import account_bridge, runtime_coordinator
    from app.account_lifecycle import AccountKey
    coordinator = runtime_coordinator.current()
    instance = request.headers.get("x-hangar-runtime-instance", "")
    if coordinator is None or not coordinator.instance or not secrets.compare_digest(instance, coordinator.instance):
        raise HTTPException(404)
    raw = bytearray()
    async for chunk in request.stream():
        raw.extend(chunk)
        if len(raw) > 64 * 1024:
            raise HTTPException(413)
    try:
        body = json.loads(raw)
        if not isinstance(body, dict) or set(body) != {"keys"} or not isinstance(body["keys"], list) or len(body["keys"]) > 128:
            raise ValueError("pedido de fatos inválido")
        keys = []
        for item in body["keys"]:
            if not isinstance(item, dict) or set(item) != {"provider", "canonical_home"}:
                raise ValueError("chave inválida")
            if not isinstance(item["canonical_home"], str) or not Path(item["canonical_home"]).is_absolute():
                raise ValueError("caminho inválido")
            key = AccountKey.new(item["provider"], Path(item["canonical_home"]))
            keys.append((item, key))
    except (TypeError, ValueError, OSError):
        raise HTTPException(400) from None
    def inspect():
        return [{"key": raw_key, "facts": asdict(account_bridge.inspect_usage(key))} for raw_key, key in keys]
    return await asyncio.to_thread(inspect)

@router.post("/accounts/prepare")
@router.post("/accounts/prepare/wait")
async def account_prepare(request: Request):
    from app import account_bridge, runtime_coordinator
    coordinator = runtime_coordinator.current()
    instance = request.headers.get("x-hangar-runtime-instance", "")
    if coordinator is None or not coordinator.instance or not secrets.compare_digest(instance, coordinator.instance):
        raise HTTPException(404)
    raw = bytearray()
    async for chunk in request.stream():
        raw.extend(chunk)
        if len(raw) > 16 * 1024:
            raise HTTPException(413)
    try:
        body = json.loads(raw)
        if request.url.path.endswith("/wait"):
            if not isinstance(body, dict) or set(body) != {"operation"} or not isinstance(body["operation"], str):
                raise ValueError("operação inválida")
            return await account_bridge.preparation_jobs.wait(body["operation"])
        if not isinstance(body, dict) or body.get("instance") != instance:
            raise ValueError("instância divergente")
        return await account_bridge.preparation_jobs.start(body)
    except (KeyError, TypeError, ValueError, OSError):
        raise HTTPException(409, detail={"code": "account_prepare_rejected"}) from None


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
                    or slot.phase != runtime_coordinator.Phase.Rust
                    or slot.lease is not None and not slot.lease.closed):
                raise RuntimeError("serviço de outra posse ou geração")
        if body["kind"] != "native_message":
            # Os outros serviços não escrevem na sessão (cálculo, sidecar idempotente, log): só a
            # posse importa, e o diário não registra tentativa deles.
            return
        state = json.loads(slot.binding.state_path.read_bytes())
        operation = state["operations"][body["phase_id"]]
        if (state["owner_key"] != body["key"] or state["generation"] != body["generation"]
                or operation["status"] != "dispatching" or operation["payload"].get("kind") != body["kind"]
                or type(operation["payload"].get("request_id")) is not type(body["request_id"])
                or operation["payload"].get("request_id") != body["request_id"]
                or operation["payload"].get("payload") != body["payload"]):
            raise RuntimeError("serviço sem tentativa registrada")

    await asyncio.to_thread(validate)
    key = (instance, body["key"], body["generation"], body["phase_id"])
    if key not in _policy_calls:
        metadata = copy.deepcopy(slot.binding.meta)
        metadata.update(provider=slot.binding.provider, name=slot.binding.name, key=slot.binding.key,
                        generation=slot.binding.generation, jsonl=slot.binding.jsonl,
                        descriptor=slot.binding.descriptor(),
                        state_path=str(slot.binding.state_path),
                        operation_id=body["request_id"] if isinstance(body["request_id"], str) else body["phase_id"], validate=validate)
        async def perform():
            await asyncio.to_thread(validate)
            with slot.guard:
                slot.active += 1
            try:
                return await service.execute(body["kind"], body["payload"], metadata)
            finally:
                with slot.guard:
                    slot.active -= 1
                coordinator._signal(slot)
        task = _policy_calls[key] = asyncio.create_task(perform())
        # Só a chamada em curso fica: a repetida depois do fim é barrada pelo diário (native_message)
        # ou é inofensiva (as demais), e guardar cada resultado fazia o mapa crescer sem fim.
        task.add_done_callback(lambda _done: _policy_calls.pop(key, None))
    return await asyncio.shield(_policy_calls[key])


_LIST_FACTS_MAX = 8 << 20
_list_facts_invalid_at = 0.0
# Os do envio e os da opção levam o nome que o Python já usava, para o diário não ter dois nomes por falha.
_DIAG_EVENT = re.compile(r"rust\.[a-z_]{1,48}|runtime\.(?:send_failed|send_uncertain|command_deferred)|opcao\.(?:nao_convergiu|envio_falhou)")
_DIAG_WARNING = {"runtime.send_uncertain", "runtime.command_deferred"}
_DIAG_CODE = re.compile(r"[a-z0-9_]{1,64}")


@router.post("/diag")
async def rust_diag(request: Request) -> dict:
    """Falha que o Rust atendeu sozinho (histórico, eventos, Git): o log dele não entra no diário."""
    raw = await request.body()
    try:
        body = json.loads(raw) if len(raw) <= 8192 else None
        if (not isinstance(body, dict) or set(body) != {"evento", "sessao", "codigo", "motivo"}
                or not all(isinstance(v, str) for v in body.values())
                or not _DIAG_EVENT.fullmatch(body["evento"]) or not _DIAG_CODE.fullmatch(body["codigo"])
                or len(body["sessao"]) > 128 or len(body["motivo"]) > 300):
            raise ValueError("diário inválido")
    except (ValueError, RecursionError):
        raise HTTPException(400) from None
    diag.registrar(body["evento"], "aviso" if body["evento"] in _DIAG_WARNING else "erro", sessao=body["sessao"], codigo=body["codigo"], detalhe=body["motivo"])
    return {"ok": True}


@router.post("/list/facts")
async def list_facts(request: Request) -> dict:
    """Fatos da lista para o Rust (`list/facts.rs`): ele manda as linhas que descobriu, a contagem
    de listas do dono abertas nele, o pid do pane das linhas Pi/omp e se o pedido é da sombra
    (`CP_LIST_SHADOW=1`), que recebe a assinatura da lista que o Python serviu."""
    global _list_facts_invalid_at
    import pydantic
    from app import list_facts as service
    raw = await request.body()
    try:
        body = json.loads(raw) if len(raw) <= _LIST_FACTS_MAX else None
        if (not isinstance(body, dict) or set(body) != {"rows", "owner_clients", "pane_pids", "shadow"}
                or not isinstance(body["rows"], list) or not all(isinstance(r, dict) for r in body["rows"])
                or type(body["owner_clients"]) is not int or body["owner_clients"] < 0
                or not isinstance(body["pane_pids"], dict) or type(body["shadow"]) is not bool
                or not all(isinstance(k, str) and type(v) is int for k, v in body["pane_pids"].items())):
            raise ValueError("fatos inválidos")
    except (ValueError, RecursionError):
        raise HTTPException(400) from None
    try:
        return await service.compute(body["rows"], body["owner_clients"], body["pane_pids"], body["shadow"])
    except (pydantic.ValidationError, TypeError) as e:
        # Só o campo: a mensagem do pydantic repete a linha, e ela carrega a última resposta.
        loc = e.errors()[0]["loc"] if isinstance(e, pydantic.ValidationError) else ()
        # A mesma entrada falha a cada tique do Rust: no diário uma vez por minuto.
        if time.monotonic() - _list_facts_invalid_at > 60:
            _list_facts_invalid_at = time.monotonic()
            diag.registrar("lista.fatos_invalidos", "erro", campo=".".join(map(str, loc[-1:])))
        raise HTTPException(400) from None


@router.post("/term/origin")
async def term_origin(request: Request) -> dict:
    """Origin do painel de terminal que o Rust abre para o dono: a regra e as fontes ficam aqui."""
    from app import termsock
    raw = await request.body()
    try:
        body = json.loads(raw) if len(raw) <= 8192 else None
        if (not isinstance(body, dict) or set(body) != {"origin", "host"} or not isinstance(body["origin"], str)
                or not isinstance(body["host"], (str, type(None)))):
            raise ValueError("origem inválida")
    except (ValueError, RecursionError):
        raise HTTPException(400) from None
    return {"ok": termsock._origem_aceita(body["origin"], body["host"])}


@router.post("/list/demote")
async def list_demote(request: Request) -> dict:
    """`hooks.demote_awaiting`: o pane que a lista do Rust capturou contradisse o marcador."""
    from app import runtime_policy
    raw = await request.body()
    try:
        body = json.loads(raw) if len(raw) <= 64 * 1024 else None
        if (not isinstance(body, dict) or set(body) != {"sids"} or not isinstance(body["sids"], list)
                or not all(isinstance(s, str) and 0 < len(s) <= 256 for s in body["sids"])):
            raise ValueError("rebaixamento inválido")
    except (ValueError, RecursionError):
        raise HTTPException(400) from None
    # No laço, como o rebaixamento de hoje: o mapa e o registro são mexidos sem trava pelo vigia.
    runtime_policy.demote_awaiting(body["sids"])
    return {"ok": True}


@router.get("/quota")
async def quota(config_dir: str = "") -> dict:
    """Janelas de cota da conta para a linha de status que o ator Rust monta; vazio = `~/.claude`."""
    from pathlib import Path
    from app import runtime_policy
    if len(config_dir) > 4096 or (config_dir and not Path(config_dir).is_absolute()):
        raise HTTPException(400)
    return {"windows": await asyncio.to_thread(runtime_policy.quota_windows, config_dir)}


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
        # `git_cwd`: onde o git da sessão roda (a worktree em que o agente trabalha, ou o cwd).
        "session": {"name": info.name, "cwd": info.cwd, "jsonl": info.jsonl,
                    "git_cwd": info.git_dir} if info else None,
    }


@router.get("/worktrees/context")
async def worktrees_context() -> dict:
    """O que a lista de worktrees do hangar-server não acha sozinho: sessões, pastas e contas."""
    from app import api
    from app.archive import _contas

    sessions, cwds, roots = await api._worktree_inputs()
    bases = await asyncio.to_thread(lambda: [str(base) for _cfg, _rot, base in _contas()])
    return {
        "roots": [str(r) for r in roots],
        "cwds": cwds,
        "sessions": [{"name": s.name, "cwd": s.cwd, "worktree_path": s.worktree_path, "jsonl": s.jsonl}
                     for s in sessions],
        "project_bases": bases,
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
        except runtime_coordinator.TransferInProgress:
            # Na passagem ninguém grava: a projeção em disco é a última dos dois donos. O aviso
            # deixa rastro de uma passagem que não termina.
            diag.registrar("runtime.info_during_transfer", "aviso", sessao=name)
        except Exception as exc:
            from app.runtime_coordinator import failure_reason
            diag.registrar("runtime.history_failed", "erro", sessao=name, **failure_reason(exc))
            raise HTTPException(503) from None
    return info_payload(name, info.provider, info.jsonl)


@router.get("/sessions/{name}/plugin")
async def plugin_pending(name: str) -> dict:
    """A pergunta que o plugin segura, para o Rust decidir `/select` e `/interrupt`. Some com o plugin no Rust."""
    from app import plugin_bridge
    return {"pending": plugin_bridge.pergunta_pendente(name)}


class _PluginInterrupted(BaseModel):
    model_config = ConfigDict(extra="forbid")
    interrupted: str | None


@router.post("/sessions/{name}/plugin")
async def plugin_interrupted(name: str, body: _PluginInterrupted) -> dict:
    """O Esc do app fechou a pergunta `interrupted` no terminal. Some com o plugin no Rust."""
    from app import plugin_bridge
    plugin_bridge.interrompeu(name, body.interrupted)
    return {"ok": True}


@router.get("/sessions/{name}/state-facts")
async def state_facts_snapshot(name: str) -> dict:
    """Retrato dos fatos do estado (`state_facts`); registra o interesse do Rust nesta sessão."""
    from app import state_facts
    return await asyncio.to_thread(state_facts.snapshot, name)


_STATE_SERVICES = {"permission.observe", "session.dead", "session.deliverable"}


@router.post("/sessions/{name}/state-service")
async def state_service(name: str, request: Request) -> dict:
    """Serviços do `Monitor` do Rust; falha volta com o tipo, como `runtime_policy.execute`."""
    from app import runtime_policy
    raw = await request.body()
    try:
        body = json.loads(raw) if len(raw) <= 64 * 1024 else None
        if (not isinstance(body, dict) or set(body) != {"kind", "payload"}
                or body["kind"] not in _STATE_SERVICES or not isinstance(body["payload"], dict)):
            raise ValueError("serviço inválido")
    except (ValueError, RecursionError):
        raise HTTPException(400) from None
    try:
        return {"ok": True, "data": await runtime_policy.state_service(body["kind"], name, body["payload"])}
    except Exception as exc:
        diag.registrar("estado.servico_falhou", "erro", sessao=name, codigo=type(exc).__name__)
        return {"ok": False, "error_type": type(exc).__name__}


@router.get("/migration/status")
async def migration_status() -> dict:
    from app import migration_status
    return await asyncio.to_thread(migration_status.facts)


@router.get("/sessions/{name}/transfer")
async def session_transfer(name: str) -> dict:
    """A troca de agente está em curso nesta sessão? O hangar-server pergunta antes de cada operação de
    mod que atende sozinho numa sessão sem terminal (clique, troca de aba, digitação). É a mesma guarda
    das rotas do Python (`_transfer_check`): a coordenação da troca mora aqui, e o Rust não a copia.

    Perguntada uma vez, na entrada: ao contrário do `_transfer_guard`, o ingresso não fica seguro
    durante a operação do Rust, e uma troca que comece depois desta resposta não é vista (limite
    registrado no `harnesses.md`)."""
    # Import tardio: api.py importa este módulo no topo.
    from app import api
    await api._transfer_check(name)
    return {"ok": True}


@router.get("/costs/scopes")
async def costs_scopes() -> dict:
    from app import costs_sources
    return await asyncio.to_thread(costs_sources.scopes_for_rust)


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
