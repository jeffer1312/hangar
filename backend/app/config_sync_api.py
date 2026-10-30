"""Rotas da configuração compartilhada: manifesto (prévia), pacote (origem) e aplicação (destino).

Quem leva o pacote de uma máquina para outra é o navegador, que já tem o token de todas: nenhuma
máquina precisa conhecer a outra pelo peers.json.
"""
import asyncio
import json
import logging

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.responses import Response, StreamingResponse
from pydantic import BaseModel

from app import config_sync
from app.auth import require_auth
from app.config import settings
from app.config_sync_paths import Roots
from app.mensagens import erro

config_sync_router = APIRouter(prefix="/api/config-sync")
# Uma aplicação por vez: duas mexendo no mesmo settings.json trocariam backup e relatório.
_APPLYING = asyncio.Lock()
_RUNNING: set[asyncio.Task] = set()
_log = logging.getLogger("hangar")


def _items(raw: str) -> list[str]:
    items = [i for i in (raw or "").split(",") if i]
    unknown = [i for i in items if i not in config_sync.ITEMS]
    if not items or unknown:
        listed = ", ".join(unknown) or "(nenhum)"
        raise HTTPException(400, detail=erro("config_sync_unknown_item",
                                             f"item desconhecido: {listed}", items=listed))
    return items


def _ndjson(work) -> StreamingResponse:
    """`work(progress)` roda como tarefa própria: se a tela fechar no meio, a aplicação termina
    em vez de parar pela metade. Linhas `progress`, e no fim `done` ou `error`."""
    queue: asyncio.Queue = asyncio.Queue()
    loop = asyncio.get_running_loop()

    def progress(event: dict) -> None:
        try:
            loop.call_soon_threadsafe(queue.put_nowait, {"type": "progress", **event})
        except RuntimeError:   # loop fechando: aviso perdido não derruba a leitura nem a aplicação
            pass

    async def run() -> None:
        # Cancelada (desligando o servidor) também fecha a resposta, senão o cliente espera para sempre.
        final = {"type": "error", "status": 499, "detail": "interrompido"}
        try:
            final = {"type": "done", "result": await work(progress)}
        except HTTPException as exc:
            final = {"type": "error", "status": exc.status_code, "detail": exc.detail}
        except Exception as exc:  # noqa: BLE001 — vira linha de erro em vez de corpo cortado
            _log.exception("config-sync: falha no trabalho com progresso")
            final = {"type": "error", "status": 500, "detail": str(exc)[:300]}
        finally:
            loop.call_soon_threadsafe(queue.put_nowait, final)

    task = asyncio.create_task(run())
    # O loop só guarda referência fraca: sem esta, a tela fechada deixaria a aplicação ser coletada.
    _RUNNING.add(task)
    task.add_done_callback(_RUNNING.discard)

    async def lines():
        while True:
            event = await queue.get()
            yield json.dumps(event, ensure_ascii=False) + "\n"
            if event["type"] != "progress":
                await task
                return

    # Sem o identity o GZipMiddleware seguraria as linhas até juntar o bastante para comprimir.
    return StreamingResponse(lines(), media_type="application/x-ndjson",
                             headers={"Content-Encoding": "identity", "Cache-Control": "no-store"})


@config_sync_router.get("/manifest", dependencies=[Depends(require_auth)])
async def get_manifest(stream: bool = False):
    async def work(progress=None) -> dict:
        data = await asyncio.to_thread(config_sync.manifest, Roots.this_machine(), progress)
        return {**data, "machine": settings.server_id or ""}
    return _ndjson(work) if stream else await work()


def _keep(raw: str) -> dict[str, list[str]] | None:
    """`{"item": ["chave do manifesto", ...]}`; ausente = cada item vai inteiro."""
    if not raw:
        return None
    try:
        keep = json.loads(raw)
    except ValueError:
        keep = None
    if (not isinstance(keep, dict)
            or any(i not in config_sync.ITEMS for i in keep)
            or not all(isinstance(v, list) and all(isinstance(k, str) for k in v)
                       for v in keep.values())):
        raise HTTPException(400, detail=erro("config_sync_invalid_keys",
                                             "escolha de entradas inválida"))
    return keep


@config_sync_router.get("/bundle", dependencies=[Depends(require_auth)])
async def get_bundle(items: str = Query(""), keys: str = Query("")) -> Response:
    chosen = _items(items)
    keep = _keep(keys)
    roots = Roots.this_machine()
    try:
        raw = await asyncio.to_thread(
            lambda: config_sync.pack(config_sync.export_bundle(roots, chosen, keep=keep)))
    except config_sync.BundleTooBig as exc:
        largest = ", ".join(f"{item} ({size // (1024 * 1024)} MB)" for item, size in exc.largest)
        raise HTTPException(413, detail=erro("config_sync_bundle_too_big", str(exc),
                                             largest=largest)) from exc
    # Já é gzip: sem o Content-Encoding o GZipMiddleware comprimiria de novo. Leva segredo, por
    # isso nenhum cache guarda.
    return Response(raw, media_type="application/gzip",
                    headers={"Content-Encoding": "identity", "Cache-Control": "no-store"})






def _busy() -> HTTPException:
    return HTTPException(409, detail=erro("config_sync_busy",
                                          "outra aplicação está em andamento nesta máquina"))


@config_sync_router.post("/apply", dependencies=[Depends(require_auth)])
async def post_apply(request: Request, items: str = Query(""), stream: bool = False):
    chosen = _items(items)
    if _APPLYING.locked():
        raise _busy()
    raw = await request.body()
    try:
        bundle = await asyncio.to_thread(config_sync.unpack, raw)
    except config_sync.BundleError as exc:
        raise HTTPException(400, detail=erro(exc.code, str(exc), **exc.params)) from exc

    async def work(progress=None) -> dict:
        # Conferido de novo: outro pedido pode ter entrado enquanto este lia o corpo.
        if _APPLYING.locked():
            raise _busy()
        async with _APPLYING:
            return await config_sync.apply_bundle(bundle, chosen, Roots.this_machine(),
                                                  progress=progress)
    return _ndjson(work) if stream else await work()
