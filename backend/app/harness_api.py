import asyncio

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, ConfigDict, StrictBool

from app import codex_integracao
from app.auth import require_auth
from app.mensagens import erro

harness_router = APIRouter(prefix="/api/harness")


class CodexOpcoesBody(BaseModel):
    model_config = ConfigDict(extra="forbid")
    contexto_estendido: StrictBool
    # Ponte: cliente antigo ainda manda o interruptor do beta da voz; aceito e ignorado.
    codex_voice_beta: StrictBool | None = None


# Ponte: o nativo antigo exige o campo na resposta; a voz não depende mais dele.
_VOZ_LEGADO = {"codex_voice_beta": True}


@harness_router.get("/codex/opcoes", dependencies=[Depends(require_auth)])
async def codex_opcoes_ler() -> dict:
    from app.codex_opcoes import ler_opcoes
    return {**await asyncio.to_thread(ler_opcoes, codex_integracao.SERVICO), **_VOZ_LEGADO}


@harness_router.post("/codex/opcoes", dependencies=[Depends(require_auth)])
async def codex_opcoes_salvar(body: CodexOpcoesBody) -> dict:
    from app.codex_opcoes import salvar_opcoes
    try:
        return {**await salvar_opcoes(codex_integracao.SERVICO, body.contexto_estendido), **_VOZ_LEGADO}
    except (OSError, ValueError, RuntimeError):
        raise HTTPException(409, detail=erro("erro_codex_opcoes", "Não foi possível salvar as opções do Codex.")) from None


def _com_interruptor(estado: dict) -> dict:
    from app import runtime_config
    return {**estado, "automatica": bool(runtime_config.get("codex_sync")),
            "memoria": bool(runtime_config.get("codex_memory_import"))}


@harness_router.get("/codex/integracao", dependencies=[Depends(require_auth)])
async def integracao_codex_status() -> dict:
    return _com_interruptor(codex_integracao.SERVICO.status())


@harness_router.post("/codex/integracao", status_code=202, dependencies=[Depends(require_auth)])
async def integracao_codex_reconciliar() -> dict:
    return _com_interruptor(await codex_integracao.SERVICO.iniciar(motivo="manual", forcar=True))


@harness_router.post("/codex/integracao/sessao", status_code=202, dependencies=[Depends(require_auth)])
async def integracao_codex_sessao() -> dict:
    """Gatilho do lançador da TUI: só reconcilia se a fonte mudou; devolve na hora e o lançador consulta."""
    return _com_interruptor(await codex_integracao.SERVICO.sessao())
