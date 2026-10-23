# backend/app/internal_api.py
"""Rotas internas que só o hangar-server, filho deste backend na mesma máquina, consome."""
import asyncio
import secrets
import json
import logging
import re
import time
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel, ConfigDict
from sse_starlette.sse import EventSourceResponse

from app import diag
from app.internal_pool import internal_thread
from app.mensagens import erro
from app.sse import merged_events

_LOOPBACK = {"127.0.0.1", "::1"}

# Só na memória: no os.environ ele iria para toda sessão que o backend sobe.
_secret: str | None = None


def set_secret(value: str | None) -> None:
    global _secret
    _secret = value or None


async def require_internal(request: Request) -> None:
    # No laço, sem thread: um pool cheio não pode impedir o Rust de entrar nas rotas que liberam o pool.
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


router = APIRouter(prefix="/internal", dependencies=[Depends(require_internal)], include_in_schema=False)


@router.get("/ping")
async def ping() -> dict:
    """Sem resposta, o laço do Python está parado (vigia do Rust)."""
    return {"ok": True}


async def _instance_body(request: Request, limit: int, *, modes=None):
    """Só a instância viva do coordenador fala por aqui; o corpo tem teto antes do JSON."""
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    instance = request.headers.get("x-hangar-runtime-instance", "")
    if (coordinator is None or not coordinator.instance
            or (modes is not None and getattr(coordinator, "mode", "python") not in modes)
            or not secrets.compare_digest(instance, coordinator.instance)):
        raise HTTPException(404)
    raw = bytearray()
    async for chunk in request.stream():
        raw.extend(chunk)
        if len(raw) > limit:
            raise HTTPException(413)
    return coordinator, instance, raw


@router.post("/accounts/codex-invalidate")
async def codex_invalidate(request: Request):
    from pathlib import Path
    from app.account_lifecycle import AccountKey
    _, _, raw = await _instance_body(request, 16384)
    try:
        body = json.loads(raw)
        item = body["key"]
        if set(body) != {"key"} or set(item) != {"provider", "canonical_home"} or item["provider"] != "codex":
            raise ValueError
        if not isinstance(item["canonical_home"], str) or not Path(item["canonical_home"]).is_absolute():
            raise ValueError
        key = AccountKey.new("codex", Path(item["canonical_home"]))
    except (KeyError, TypeError, ValueError, OSError):
        raise HTTPException(400) from None
    service = getattr(request.app.state, "codex_contas_login", None)
    if service is None:
        raise HTTPException(503)
    service._invalidate_auth(str(key.canonical_home))
    return {"ok": True}


@router.post("/accounts/prepare")
@router.post("/accounts/prepare/wait")
async def account_prepare(request: Request):
    from app import account_bridge
    _, instance, raw = await _instance_body(request, 16 * 1024)
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

@router.post("/accounts/claude-window")
async def claude_window(request: Request):
    from app import account_bridge
    _, _, raw = await _instance_body(request, 16 * 1024)
    try:
        body = json.loads(raw)
        return await internal_thread(account_bridge.claude_windows.run, body)
    except (KeyError, TypeError, ValueError, OSError, RuntimeError):
        raise HTTPException(409, detail={"code": "account_claude_window_rejected"}) from None


_LIST_FACTS_MAX = 8 << 20
_list_facts_invalid_at = 0.0
# Os do envio e os da opção levam o nome que o Python já usava, para o diário não ter dois nomes por falha.
_DIAG_EVENT = re.compile(r"rust\.[a-z_]{1,48}|runtime\.(?:send_failed|send_uncertain|command_deferred)|opcao\.(?:nao_convergiu|envio_falhou)")
_DIAG_WARNING = {"runtime.send_uncertain", "runtime.command_deferred", "rust.groups_sweep_failed", "rust.groups_sweep_recovered"}
# `:` e maiúscula: o Rust anexa o detalhe ao código (`list_facts_status:500`, `…:sessions:Eof`).
_DIAG_CODE = re.compile(r"[A-Za-z0-9_:]{1,64}")


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
    """Estado das linhas Pi/omp/Kimi para o Rust (`list/facts.rs`), com o pid do pane das Pi/omp."""
    global _list_facts_invalid_at
    import pydantic
    from app import list_facts as service
    raw = await request.body()
    try:
        body = json.loads(raw) if len(raw) <= _LIST_FACTS_MAX else None
        if (not isinstance(body, dict) or set(body) != {"rows", "pane_pids"}
                or not isinstance(body["rows"], list) or not all(isinstance(r, dict) for r in body["rows"])
                or not isinstance(body["pane_pids"], dict)
                or not all(isinstance(k, str) and type(v) is int for k, v in body["pane_pids"].items())):
            raise ValueError("fatos inválidos")
    except (ValueError, RecursionError):
        raise HTTPException(400) from None
    try:
        return await service.compute(body["rows"], body["pane_pids"])
    except (pydantic.ValidationError, TypeError) as e:
        # Só o campo: a mensagem do pydantic repete a linha, e ela carrega a última resposta.
        loc = e.errors()[0]["loc"] if isinstance(e, pydantic.ValidationError) else ()
        # A mesma entrada falha a cada tique do Rust: no diário uma vez por minuto.
        if time.monotonic() - _list_facts_invalid_at > 60:
            _list_facts_invalid_at = time.monotonic()
            diag.registrar("lista.fatos_invalidos", "erro", campo=".".join(map(str, loc[-1:])))
        raise HTTPException(400) from None


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
    folders = await asyncio.to_thread(api.list_folders)
    return {
        "roots": [str(r) for r in roots],
        "cwds": cwds,
        "sessions": [{"name": s.name, "cwd": s.cwd, "git_cwd": s.git_dir, "worktree_path": s.worktree_path, "jsonl": s.jsonl}
                     for s in sessions],
        "project_bases": bases,
        "activities": [{"cwd": f.cwd, "mtime": f.mtime} for f in folders if f.cwd],
    }



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


@router.get("/migration/status")
async def migration_status() -> dict:
    from app import migration_status
    return await internal_thread(migration_status.facts)


@router.get("/transcription/config")
def transcription_config():
    """Configuração efetiva exclusiva da ponte privada; chaves nunca vão à resposta pública."""
    from app import runtime_config
    providers = runtime_config.get("transcription_providers") or []
    legacy = None
    key = (runtime_config.get("groq_api_key") or "").strip()
    if not runtime_config.get("transcription_providers") and key:
        legacy = {"id": "legacy", "kind": "openai", "api_key": key,
                  "base_url": runtime_config.get("transcription_base_url") or "",
                  "model": runtime_config.get("transcription_model") or ""}
    organization = {"dictation_include_recent_messages": bool(runtime_config.get("dictation_include_recent_messages")),
                    **{field: runtime_config.get(field) or "" for field in (
                        "dictation_organization_mode", "dictation_claude_model", "dictation_codex_model", "ditado_estilo",
                        "llm_base_url", "llm_api_key", "llm_model", "llm_reasoning_effort",
                        "llm_briefing_base_url", "llm_briefing_api_key", "llm_briefing_model")}}
    if organization["dictation_organization_mode"] != "external_api":
        # A chave da API externa só sai quando é ela que organiza; os campos ficam vazios pro Rust.
        organization.update(llm_base_url="", llm_api_key="", llm_briefing_base_url="", llm_briefing_api_key="")
    return {"providers": providers, "legacy": legacy, "organization": organization,
            "user_vocabulary": runtime_config.get("ditado_vocabulario") or "",
            "dictation_max_minutes": int(runtime_config.get("dictation_max_minutes") or 0),
            "state_path": str(Path.home() / ".hangar" / "transcription-wait.json")}


class _OrqGid(BaseModel):
    model_config = ConfigDict(extra="forbid")
    gid: str


class _OrqPromote(_OrqGid):
    name: str


class _OrqNames(BaseModel):
    model_config = ConfigDict(extra="forbid")
    names: list[str]


@router.post("/orq/group-phase")
async def orq_group_phase(body: _OrqGid) -> dict:
    from app.adapters.orq import runs
    return {"phase": await internal_thread(runs.group_phase, body.gid)}


@router.post("/orq/promote")
async def orq_promote(body: _OrqPromote) -> dict:
    """O Rust promove o time quando um grupo `orq` nasce, com os mesmos erros da rota `/pair`."""
    from app import orq_context, orq_md
    try:
        await internal_thread(orq_context.promote, body.name, body.gid)
    except orq_context.IdentityUnavailable:
        # Sem identidade não houve configuração pela API para promover.
        logging.getLogger(__name__).warning("pair: sem identidade para promover o grupo %s", body.gid)
    except (orq_context.PromotionConflict, orq_md.Conflito) as e:
        raise HTTPException(409, detail=erro("erro_orq_arquivo_mudou", str(e)))
    return {}


@router.post("/orq/is-orchestrator")
async def orq_is_orchestrator(body: _OrqNames) -> dict:
    """Os nomes que são a linha do orquestrador: a recusa `_recusa_orq` das rotas de par."""
    from app.adapters.orq import runs
    return {"names": await internal_thread(lambda: [n for n in body.names if runs.find(n)])}


@router.get("/costs/scopes")
async def costs_scopes() -> dict:
    from app import costs_sources
    return await internal_thread(costs_sources.scopes_for_rust)


@router.get("/sessions/{name}/side-events")
async def side_events(name: str, provider: str, jsonl: str, app: int = 0):
    """Uma conexão por sessão para o hangar-server, que reparte estado, prévia, perguntas e fila
    entre os aparelhos. `provider` e `jsonl` são a conversa que o Rust escolheu; a troca ele faz
    religando a conexão. `app=1`: há ao menos um aparelho do dono no chat (push fica calado)."""
    if provider not in ("claude", "claude-headless", "codex") or not Path(jsonl).is_absolute():
        raise HTTPException(400, detail=erro("erro_conversa_invalida", "provider ou transcript inválido"))
    return EventSourceResponse(
        merged_events(name, jsonl, provider=provider, count_app=bool(app), side=True),
        send_timeout=30)


@router.get("/orq/{exec_id}/conductor-facts")
async def orq_conductor_facts(exec_id: str) -> dict:
    """Uma leitura factual: caminho validado e watchdog compartilhado com a lista."""
    from app import orq, orq_conductor

    def read():
        directory = orq.exec_dir(orq.raiz_padrao(), exec_id)
        if directory is None or not directory.is_dir():
            raise HTTPException(404, detail=erro("erro_nao_encontrado", "execução não encontrada"))
        return {"path": str(directory), "watchdog": orq_conductor.watchdog(directory, orq_conductor.units())}

    return await internal_thread(read)
