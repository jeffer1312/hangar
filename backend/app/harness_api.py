"""Rotas do painel de saúde dos harnesses (app/harness_saude.py)."""
import asyncio
import logging
import sqlite3
import subprocess

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, ConfigDict, StrictBool

from app import codex_integracao, contas, harness_install, harness_saude, runtime_config
from app.auth import require_auth
from app.mensagens import erro

_log = logging.getLogger(__name__)

harness_router = APIRouter(prefix="/api/harness")


class CodexOpcoesBody(BaseModel):
    model_config = ConfigDict(extra="forbid")
    contexto_estendido: StrictBool
    codex_voice_beta: StrictBool | None = None


@harness_router.get("/codex/opcoes", dependencies=[Depends(require_auth)])
async def codex_opcoes_ler() -> dict:
    from app.codex_opcoes import ler_opcoes
    return {**await asyncio.to_thread(ler_opcoes, codex_integracao.SERVICO),
            "codex_voice_beta": runtime_config.get("codex_voice_beta") is True}


@harness_router.post("/codex/opcoes", dependencies=[Depends(require_auth)])
async def codex_opcoes_salvar(body: CodexOpcoesBody) -> dict:
    from app.codex_opcoes import salvar_opcoes
    voz_tinha_override, voz_override = runtime_config.override("codex_voice_beta")
    voz_anterior = runtime_config.get("codex_voice_beta") is True
    mudou_voz = body.codex_voice_beta is not None and body.codex_voice_beta != voz_anterior
    try:
        if mudou_voz:
            await asyncio.to_thread(runtime_config.aplicar, {"codex_voice_beta": body.codex_voice_beta})
        resposta = await salvar_opcoes(codex_integracao.SERVICO, body.contexto_estendido)
        return {**resposta, "codex_voice_beta": runtime_config.get("codex_voice_beta") is True}
    except (OSError, ValueError, RuntimeError):
        if mudou_voz:
            try:
                if voz_tinha_override:
                    await asyncio.to_thread(runtime_config.aplicar, {"codex_voice_beta": voz_override})
                else:
                    await asyncio.to_thread(runtime_config.aplicar, {}, remover={"codex_voice_beta"})
            except (OSError, ValueError):
                _log.exception("falha ao restaurar codex_voice_beta após erro nas opções do Codex")
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


@harness_router.get("/instalar", dependencies=[Depends(require_auth)])
async def instalacao_estado() -> dict:
    """Progresso da instalação em curso e, junto, o que dá pra instalar por botão nesta máquina —
    a tela já consulta este endereço, então não precisa de um segundo só pra ler a lista."""
    return harness_install.INSTALADOR.status()


@harness_router.post("/instalar/{cli}", status_code=202, dependencies=[Depends(require_auth)])
async def instalar(cli: str) -> dict:
    try:
        return await harness_install.INSTALADOR.iniciar(cli)
    except ValueError:
        raise HTTPException(400, detail=erro("erro_harness_sem_instalador",
                                             f"nao ha comando conferido pra instalar {cli} aqui",
                                             cli=cli))
    except harness_install.EmCurso as e:
        # 409 e não o estado da outra: devolver o estado dela faria a tela ler `ok` de uma
        # instalação que não é esta e dar o CLI pedido por instalado.
        raise HTTPException(409, detail=erro("erro_harness_instalando",
                                             f"ja ha uma instalacao em curso ({e}) — espere ela terminar",
                                             harness=str(e)))


@harness_router.get("", dependencies=[Depends(require_auth)])
async def listar() -> list[dict]:
    # `--version` de cinco CLIs em série: fora do loop do servidor, que segue servindo o SSE.
    return await asyncio.to_thread(harness_saude.diagnosticar)


@harness_router.post("/conserto/{id_:path}", dependencies=[Depends(require_auth)])
async def consertar(id_: str) -> dict:
    try:
        feito = await asyncio.to_thread(harness_saude.consertar, id_)
    except ValueError as e:
        raise HTTPException(400, detail=erro("erro_harness_conserto", str(e), motivo=str(e)))
    except (OSError, contas.ContaError, sqlite3.Error, subprocess.SubprocessError) as e:
        raise HTTPException(500, detail=erro("erro_harness_conserto", str(e), motivo=str(e)))
    return {"feito": feito, "harnesses": await asyncio.to_thread(harness_saude.diagnosticar)}
