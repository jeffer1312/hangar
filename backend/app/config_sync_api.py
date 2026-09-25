"""Rotas da configuração compartilhada: manifesto (prévia), pacote (origem) e aplicação (destino).

Quem leva o pacote de uma máquina para outra é o navegador, que já tem o token de todas: nenhuma
máquina precisa conhecer a outra pelo peers.json.
"""
import asyncio

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.responses import Response

from app import config_sync
from app.auth import require_auth
from app.config import settings
from app.config_sync_paths import Roots
from app.mensagens import erro

config_sync_router = APIRouter(prefix="/api/config-sync")
# Uma aplicação por vez: duas mexendo no mesmo settings.json trocariam backup e relatório.
_APPLYING = asyncio.Lock()


def _items(raw: str) -> list[str]:
    items = [i for i in (raw or "").split(",") if i]
    unknown = [i for i in items if i not in config_sync.ITEMS]
    if not items or unknown:
        listed = ", ".join(unknown) or "(nenhum)"
        raise HTTPException(400, detail=erro("config_sync_unknown_item",
                                             f"item desconhecido: {listed}", items=listed))
    return items


@config_sync_router.get("/manifest", dependencies=[Depends(require_auth)])
async def get_manifest() -> dict:
    data = await asyncio.to_thread(config_sync.manifest, Roots.this_machine())
    return {**data, "machine": settings.server_id or ""}


@config_sync_router.get("/bundle", dependencies=[Depends(require_auth)])
async def get_bundle(items: str = Query("")) -> Response:
    chosen = _items(items)
    roots = Roots.this_machine()
    try:
        raw = await asyncio.to_thread(
            lambda: config_sync.pack(config_sync.export_bundle(roots, chosen)))
    except config_sync.BundleTooBig as exc:
        largest = ", ".join(f"{item} ({size // (1024 * 1024)} MB)" for item, size in exc.largest)
        raise HTTPException(413, detail=erro("config_sync_bundle_too_big", str(exc),
                                             largest=largest)) from exc
    # Já é gzip: sem o Content-Encoding o GZipMiddleware comprimiria de novo. Leva segredo, por
    # isso nenhum cache guarda.
    return Response(raw, media_type="application/gzip",
                    headers={"Content-Encoding": "identity", "Cache-Control": "no-store"})


@config_sync_router.post("/apply", dependencies=[Depends(require_auth)])
async def post_apply(request: Request, items: str = Query("")) -> dict:
    chosen = _items(items)
    if _APPLYING.locked():
        raise HTTPException(409, detail=erro("config_sync_busy",
                                             "outra aplicação está em andamento nesta máquina"))
    async with _APPLYING:
        raw = await request.body()
        try:
            bundle = await asyncio.to_thread(config_sync.unpack, raw)
        except config_sync.BundleError as exc:
            raise HTTPException(400, detail=erro(exc.code, str(exc), **exc.params)) from exc
        return await config_sync.apply_bundle(bundle, chosen, Roots.this_machine())
