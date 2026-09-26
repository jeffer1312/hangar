"""Rotas da configuração compartilhada: manifesto (prévia), pacote (origem) e aplicação (destino).

Quem leva o pacote de uma máquina para outra é o navegador, que já tem o token de todas: nenhuma
máquina precisa conhecer a outra pelo peers.json.
"""
import asyncio
import json
from typing import Annotated, Literal

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.responses import Response
from pydantic import BaseModel, ConfigDict, Field

from app import config_sync, config_sync_translate
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


class TranslateBody(BaseModel):
    model_config = ConfigDict(extra="forbid")
    lang: Literal["pt", "en"]
    texts: list[Annotated[str, Field(max_length=2000)]] = Field(max_length=2000)


@config_sync_router.post("/translate", dependencies=[Depends(require_auth)])
async def post_translate(body: TranslateBody) -> dict:
    """Descrições da prévia no idioma da tela. Sempre 200: falha devolve os originais e o motivo
    em `error`, para a tela mostrar o texto e avisar por que não traduziu."""
    texts, error = await asyncio.to_thread(config_sync_translate.translate, body.texts, body.lang)
    return {"texts": texts, "error": error}


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
