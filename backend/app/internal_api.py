# backend/app/internal_api.py
"""Rotas internas que só o hangar-server, filho deste backend na mesma máquina, consome."""
import asyncio
import secrets
import json
import logging
import time
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel, ConfigDict

from app import diag
from app.internal_pool import internal_thread
from app.mensagens import erro

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

_LIST_FACTS_MAX = 8 << 20
_list_facts_invalid_at = 0.0
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
