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

# Sessões no meio da troca de modo: a fotografia leva ao Rust que (re)subiu no meio dela.
changing_mode: set[str] = set()
# Vida nova que a ponte não aceitou no fim de uma troca de modo: refeita antes da próxima
# fotografia, senão o Rust que subir compara o convite com a vida velha e responde 410.
_unsent_lives: dict[str, str] = {}
_SNAPSHOT_MAX_DELAY = 60.0


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


def _asked(op: str, **args) -> dict | None:
    """A op pedida; falha vira o aviso que a resposta leva (o diário já registrou)."""
    try:
        call(op, **args)
    except ShareBridgeError as e:
        return _warning(e)
    return None


def revoke_session(name: str) -> dict | None:
    try:
        call("revoke_session", name=name)
    except ShareBridgeError as e:
        # Sem Rust (reserva ou subindo) a varredura dele revoga ao voltar: a sessão fechada é
        # ausência confirmada. Avisar a cada fechamento na reserva seria ruído; o diário já tem.
        return None if e.code == "share_bridge_off" else _warning(e)
    return None


def set_life(name: str, life: str | None) -> dict | None:
    warning = _asked("set_life", name=name, life=life)
    if warning is None:
        _unsent_lives.pop(name, None)
    elif life is not None:
        _unsent_lives[name] = life
    return warning


def mode_change(name: str, begin: bool) -> dict | None:
    if begin:
        return _asked("mode_change_begin", name=name)
    # Transferência que segue incompleta (restauração que falhou): a vida de origem continua valendo.
    from app import conversation_transfer, diag
    try:
        source = next((r.source_life for r in conversation_transfer.list_incomplete() if r.name == name), None)
    except Exception as e:                           # noqa: BLE001 — a troca de modo acaba mesmo assim
        _log.warning("transferências ilegíveis no fim da troca de modo de %s", name, exc_info=True)
        diag.registrar("convites.transferencias_ilegiveis", "aviso", **diag.erro_campos(e))
        source = None
    return _asked("mode_change_end", name=name, source_life=source)


def send_snapshot() -> bool:
    """Fotografia do que só o Python sabe, mandada a cada Rust que sobe. Sem ela o Rust responde
    "não sei" (503) e a varredura não revoga nada; com uma fotografia incompleta daria 410."""
    from app import conversation_transfer, diag
    try:
        lives = {r.name: r.source_life for r in conversation_transfer.list_incomplete()}
    except Exception as e:                           # noqa: BLE001 — sem fotografia é o lado seguro
        _log.warning("transferências ilegíveis: fotografia dos convites não enviada", exc_info=True)
        diag.registrar("convites.fotografia_falhou", "aviso", **diag.erro_campos(e))
        return False
    try:
        for name, life in list(_unsent_lives.items()):
            call("set_life", name=name, life=life)
            if _unsent_lives.get(name) == life:
                del _unsent_lives[name]
        call("facts_snapshot", lives=lives, changing=sorted(changing_mode))
    except ShareBridgeError:
        return False                                 # `call` já registrou no diário
    return True


async def send_snapshot_until_sent(stopping) -> None:
    """Repete a fotografia até o Rust aceitar; o teto do intervalo limita o diário. Acaba quando o
    backend para ou o Rust cai (o próximo manda a sua)."""
    delay = 1.0
    while not stopping() and list_bridge.endpoint() is not None:
        await asyncio.sleep(delay)
        if stopping() or list_bridge.endpoint() is None:
            return
        if await asyncio.to_thread(send_snapshot):
            return
        delay = min(delay * 2, _SNAPSHOT_MAX_DELAY)


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
