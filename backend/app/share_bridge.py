"""Ponte privada dos convites: o Rust é o único dono de `shares.json` e `external_pairs.json`. O
Python pede em `/__hangar_server/share` o que só ele vê acontecer (fechar, renomear, trocar de modo,
transferência, sessão morta, recado do MCP) e nunca grava os arquivos. As rotas do dono que chegam
pelo Connect (8768) vão às rotas privadas explícitas do Rust.

Falha da ponte nunca some calada: vai ao diário, e quem fecha ou renomeia devolve um aviso.
"""
import asyncio
import http.client
import json
import logging
import urllib.error
import urllib.request
from urllib.parse import quote

from fastapi import APIRouter, Depends, Request
from fastapi.responses import JSONResponse

from app import list_bridge
from app.auth import require_auth
from app.mensagens import erro

_log = logging.getLogger("hangar.share")
# Rename e fim de par esperam a outra máquina (8 s) e o túnel (20 s).
_TIMEOUT = 45
_MAX_RESPONSE = 1024 * 1024
# Criar convite pela internet espera a conferência de fora (até 180 s).
_OWNER_TIMEOUT = 240
_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
_PRIVATE = "/__hangar_server/share/owner"


class ShareBridgeError(RuntimeError):
    """Falha da ponte com código (`share_bridge_off`, `share_bridge_unavailable`, o código do Rust)."""
    def __init__(self, code: str, detail: str = ""):
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code, self.detail = code, detail


def call(op: str, **args) -> dict:
    """Bloqueia (nunca no laço: o Rust chama de volta as rotas internas do Python)."""
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        pass
    else:
        _failed(op, "share_wait_on_loop")
        raise ShareBridgeError("share_wait_on_loop")
    endpoint = list_bridge.endpoint()
    if endpoint is None:
        _failed(op, "share_bridge_off")
        raise ShareBridgeError("share_bridge_off")
    address, secret = endpoint
    data = json.dumps({"op": op, "args": args}, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(f"http://{address}/__hangar_server/share", data=data,
        headers={"content-type": "application/json", "x-hangar-internal": secret}, method="POST")
    try:
        with _opener.open(req, timeout=_TIMEOUT) as response:
            body = response.read(_MAX_RESPONSE + 1)
        if len(body) > _MAX_RESPONSE:
            raise ValueError("resposta grande demais")
        value = json.loads(body)
        if not isinstance(value, dict) or type(value.get("ok")) is not bool:
            raise ValueError("resposta sem ok")
    except (OSError, ValueError, urllib.error.URLError, http.client.HTTPException) as e:
        _failed(op, "share_bridge_unavailable")
        raise ShareBridgeError("share_bridge_unavailable", type(e).__name__) from e
    if not value["ok"]:
        error = value.get("error") if isinstance(value.get("error"), dict) else {}
        code = str(error.get("code") or "share_bridge_invalid")
        _failed(op, code)
        raise ShareBridgeError(code, str(error.get("detail") or ""))
    result = value.get("result")
    if not isinstance(result, dict):
        _failed(op, "share_bridge_invalid")
        raise ShareBridgeError("share_bridge_invalid", "resultado")
    return result


def _failed(op: str, reason: str) -> None:
    from app import diag
    _log.warning("share bridge failed op=%s reason=%s", op, reason)
    diag.registrar("convites.ponte", "aviso", operacao_rust=op, motivo=reason)


def _warning(e: ShareBridgeError) -> dict:
    # O convidado perderia o convite (ou o manteria numa sessão que não existe) sem ninguém saber.
    return erro("erro_convite_nao_acompanhou",
                f"o convite desta sessão não acompanhou a mudança ({e.code})", codigo=e.code)




def revoke_session(name: str) -> dict | None:
    try:
        call("revoke_session", name=name)
    except ShareBridgeError as e:
        # Sem Rust (reserva ou subindo) a varredura dele revoga ao voltar: a sessão fechada é
        # ausência confirmada. Avisar a cada fechamento na reserva seria ruído; o diário já tem.
        return None if e.code == "share_bridge_off" else _warning(e)
    return None


# --- Rotas do dono pelo Connect: repasse fino às rotas privadas explícitas do Rust. ---

router = APIRouter()


def _needs_rust() -> JSONResponse:
    return JSONResponse({"detail": erro("share_needs_rust_server",
                                        "convites e par externo precisam do servidor Rust")}, status_code=404)


async def _owner(request: Request, target: str) -> object:
    from app import voicesock
    query = request.url.query
    return await voicesock.forward_settings(request, f"{_PRIVATE}/{target}" + (f"?{query}" if query else ""),
                                            timeout=_OWNER_TIMEOUT, unavailable=_needs_rust)


def _seg(value: str) -> str:
    return quote(value, safe="")


@router.get("/api/share/prereqs", dependencies=[Depends(require_auth)])
async def prereqs(request: Request):
    return await _owner(request, "prereqs")


@router.post("/api/sessions/{name}/share", dependencies=[Depends(require_auth)])
async def create(name: str, request: Request):
    return await _owner(request, f"create/{_seg(name)}")


@router.get("/api/sessions/{name}/share", dependencies=[Depends(require_auth)])
async def list_shares(name: str, request: Request):
    return await _owner(request, f"list/{_seg(name)}")


@router.delete("/api/sessions/{name}/share/{share_id}", dependencies=[Depends(require_auth)])
async def revoke(name: str, share_id: str, request: Request):
    return await _owner(request, f"revoke/{_seg(name)}/{_seg(share_id)}")


@router.delete("/api/sessions/{name}/share", dependencies=[Depends(require_auth)])
async def revoke_all(name: str, request: Request):
    return await _owner(request, f"revoke_all/{_seg(name)}")


@router.post("/api/sessions/{name}/pair-invite", dependencies=[Depends(require_auth)])
async def pair_invite(name: str, request: Request):
    return await _owner(request, f"pair_invite/{_seg(name)}")


@router.post("/api/sessions/{name}/pair-accept", dependencies=[Depends(require_auth)])
async def pair_accept(name: str, request: Request):
    return await _owner(request, f"pair_accept/{_seg(name)}")


@router.get("/api/external-pairs", dependencies=[Depends(require_auth)])
async def pairs_list(request: Request):
    return await _owner(request, "pairs_list")


@router.post("/api/external-pairs/send", dependencies=[Depends(require_auth)])
async def pairs_send(request: Request):
    return await _owner(request, "pairs_send")


@router.post("/api/sessions/{name}/pair-access", dependencies=[Depends(require_auth)])
async def pair_access(name: str, request: Request):
    return await _owner(request, f"pair_access/{_seg(name)}")


@router.delete("/api/external-pairs/{alias}", dependencies=[Depends(require_auth)])
async def pairs_undo(alias: str, request: Request):
    return await _owner(request, f"pairs_undo/{_seg(alias)}")
