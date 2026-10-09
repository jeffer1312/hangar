"""Par externo: convite, aceite e resgate entre máquinas de pessoas diferentes."""
from __future__ import annotations

import asyncio
import html
import logging
import re
import time

from fastapi import APIRouter, Depends, HTTPException, Request
from fastapi.responses import HTMLResponse
from pydantic import BaseModel, Field

from app import external_pairs, groups_bridge, pair, pair_texto, peers, share_api, share_store, share_tunnel
from app.auth import require_auth
from app.external_pairs import ExternalPair
from app.mensagens import erro
from app.share_gate import GUEST_TOKEN_KEY, guest_of
from app.share_guest_api import _PAGE, _REASONS, _owner
from app.share_life import session_life

_log = logging.getLogger(__name__)
router = APIRouter()

_CODE_RE = re.compile(r"[A-Za-z0-9]{1,64}")
_MSG_JA_PAREADA = "a sessão já está em grupo ou pareada — desfaça esse par antes de parear com outra máquina"
_REMOTE_LABEL = "resposta da outra máquina: "
_MSG_PARCIAL = "pareamento desfeito aqui, mas a outra máquina pode continuar pareada: desfaça lá"
# Falhas da ponte depois de o pedido sair (`groups_bridge.call`): o Rust pode ter gravado.
_BRIDGE_UNCERTAIN = frozenset({"groups_bridge_unavailable", "groups_bridge_invalid"})


def _my_owner() -> str:
    # O nome da máquina pode ter espaço e acento; o outro lado só aceita o alfabeto de valid_owner.
    return external_pairs._slug(_owner())[:40]


def _code_error(e: share_store.ShareError) -> HTTPException:
    code, msg = _REASONS[e.reason]
    return HTTPException(404 if e.reason == "unknown" else 410, detail=erro(code, msg, reason=e.reason))


def _invite(name: str) -> dict:
    life = session_life(name)
    if life is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    if share_tunnel.port_clash():
        raise HTTPException(409, detail=erro(
            "erro_compartilhar_porta_do_convite",
            f"o app roda na porta do convite ({share_tunnel.GUEST_PORT}): troque CP_PORT no backend/.env e reinicie",
            port=share_tunnel.GUEST_PORT))
    try:
        base = share_tunnel.ensure_on()
    except share_tunnel.TunnelError as e:
        raise share_api._prereq_error(e)
    s, code = share_store.create(name, life, kind="pair")
    return {"id": s.id, "link": f"{base}/par/{code}", "expires_at": s.code_expires_at}


@router.post("/api/sessions/{name}/pair-invite", dependencies=[Depends(require_auth)])
async def pair_invite(name: str):
    return await asyncio.to_thread(_invite, name)


@router.get("/par/{code}", response_class=HTMLResponse)
def pair_page(code: str):
    # GET nunca gasta o código: prévia de link (WhatsApp) também faz GET.
    try:
        share_store.peek(code, kind="pair")
    except share_store.ShareError as e:
        corpo = f"<h1>Convite indisponível</h1><p>{html.escape(_REASONS[e.reason][1])}.</p>"
    else:
        corpo = (f"<h1>{html.escape(_owner())} quer parear uma sessão com a tua</h1>"
                 "<p>No app desktop do Hangar, clique com o botão direito na sessão que vai "
                 "trabalhar junto e escolha <b>Parear por convite…</b>. Ou cole este link "
                 "na conversa da sessão e peça para ela aceitar.</p>"
                 '<button onclick="navigator.clipboard.writeText(location.href);'
                 "this.textContent='Link copiado'\">Copiar link</button>")
    return HTMLResponse(_PAGE.format(corpo=corpo),
                        headers={"Cache-Control": "no-store", "Referrer-Policy": "no-referrer"})


class PairRedeemBody(BaseModel):
    code: str = Field(max_length=64)
    session: str = Field(max_length=64)
    address: str = Field(max_length=200)
    token: str = Field(max_length=200)
    owner: str = Field(max_length=40)


@router.post("/api/pair/redeem")
async def pair_redeem(body: PairRedeemBody):
    from app import api
    address = external_pairs.normalize_address(body.address)
    if (address is None or not external_pairs.valid_token(body.token)
            or not external_pairs.valid_session(body.session)):
        raise HTTPException(400, detail=erro("erro_par_endereco_invalido", "endereço do par inválido"))
    if not external_pairs.valid_owner(body.owner):
        raise HTTPException(400, detail=erro("erro_par_nome_invalido", "nome da máquina do par inválido"))
    try:
        invite = share_store.peek(body.code, kind="pair")
        my_host = await asyncio.to_thread(share_tunnel.host)
    except share_store.ShareError as e:
        raise _code_error(e)
    except share_tunnel.TunnelError as e:
        _log.warning("par externo: túnel indisponível ao resgatar convite: %s", e)
        raise HTTPException(503, detail=erro("erro_sessao_indisponivel", "indisponível por instantes"))
    name = invite.session
    # Antes de gastar o código: sessão já agrupada recusa sem queimar o convite.
    if await asyncio.to_thread(lambda: pair.PairLink(name).get()) is not None:
        raise HTTPException(409, detail=erro("erro_pareamento_mistura_cross", _MSG_JA_PAREADA))
    alias = external_pairs.free_alias(body.owner)
    peer = f"{alias}::{body.session}"
    harness = {s.name: s.provider for s in await asyncio.to_thread(api.registry.list)}
    # Junta antes de gastar o código: grupo recusado não queima o convite.
    try:
        snap = await _join_external(name, peer, harness)
    except (pair.PairMixError, pair.TaskConflito) as e:
        _log.warning("par externo: grupo recusou o resgate de '%s': %s", name, e)
        raise HTTPException(409, detail=erro("erro_pareamento_mistura_cross", _MSG_JA_PAREADA))
    try:
        share, token = await asyncio.to_thread(share_store.redeem, body.code, body.owner, None, None, "pair")
    except Exception as e:  # noqa: BLE001 — o grupo já foi montado: sempre desfaz antes de responder
        if snap is not None:
            await _guarded_async("restaurar o grupo", _restore_external, snap)
        if isinstance(e, share_store.ShareError):
            raise _code_error(e)
        _log.warning("par externo: resgate de '%s' falhou: %r", name, e)
        raise HTTPException(500, detail=erro("erro_pareamento_desfeito", "pareamento desfeito",
                                             avisos=""))
    try:
        await asyncio.to_thread(external_pairs.add, ExternalPair(
            share.id, name, alias, body.owner, body.session, address, body.token, time.time()))
        falha = await api._deliver(name, pair_texto.texto_par_externo(name, peer, body.owner))
    except Exception as e:  # noqa: BLE001 — código já gasto: desfaz tudo antes de propagar
        _log.warning("par externo: resgate de '%s' desfeito: %r", name, e)
        await _undo_local(snap, share.id)
        raise HTTPException(500, detail=erro("erro_pareamento_desfeito", "pareamento desfeito", avisos=""))
    if falha:
        _log.warning("par externo: aviso a '%s' falhou, resgate desfeito: %s", name, api._erro_texto(falha))
        await _undo_local(snap, share.id)
        raise HTTPException(502, detail=erro("erro_pareamento_desfeito", "pareamento desfeito", avisos=""))
    return {"session": name, "owner": _my_owner(), "address": f"https://{my_host}:{share_tunnel.FUNNEL_PORT}",
            "token": token}


async def _join_external(name: str, peer: str, harness: dict[str, str]):
    """Grava o par externo de `name`; devolve o que `_restore_external` desfaz. No modo Rust o
    sidecar é dele: antes do link a sessão está sempre solta, então desfazer é o `unlink`."""
    if groups_bridge.rust_owns_groups():
        try:
            await asyncio.to_thread(groups_bridge.call, "group.external_link", local=name, address=peer,
                                    harness={n: p for n, p in harness.items() if n == name})
        except groups_bridge.GroupsBridgeError as e:
            if e.code == "erro_pareamento_mistura_cross":
                raise pair.PairMixError(e.detail) from None
            if e.code in _BRIDGE_UNCERTAIN:
                # O pedido pode ter chegado e gravado: o unlink só age se o endereço estiver lá.
                await _guarded_async("restaurar o grupo", _restore_external, (name, peer))
            raise
        return (name, peer)
    return (await asyncio.to_thread(pair.join_group, name, [peer], "", substituir_task=True, harness=harness))[1]


def _restore_external(undo) -> None:
    if isinstance(undo, tuple):
        groups_bridge.call("group.external_unlink", local=undo[0], address=undo[1])
    else:
        pair.restore(undo)


def _leave_external(local: str, address: str) -> None:
    """Fim do par externo: o lado de fora já está sendo desfeito aqui, então nada de aviso de volta."""
    if groups_bridge.rust_owns_groups():
        groups_bridge.call("group.external_unlink", local=local, address=address)
    else:
        pair.leave(local)


def _guarded(what: str, fn, *args) -> None:
    # Cada passo do desfazer roda mesmo que o anterior falhe; senão sobra token vivo no disco.
    try:
        fn(*args)
    except Exception as ex:  # noqa: BLE001
        _log.warning("par externo: %s falhou ao desfazer: %r", what, ex)


async def _guarded_async(what: str, fn, *args) -> None:
    await asyncio.to_thread(_guarded, what, fn, *args)


async def _undo_local(snap: dict | None, share_id: str) -> None:
    if snap is not None:
        await _guarded_async("restaurar o grupo", _restore_external, snap)
    await _guarded_async("revogar o convite", share_store.revoke, share_id)
    await _guarded_async("remover o registro", external_pairs.remove, share_id)


class PairAcceptBody(BaseModel):
    link: str


def _refused(e: peers.PeerError) -> HTTPException:
    """O outro lado respondeu com o envelope de erro dele: convite usado/vencido vira a frase própria."""
    d = e.detail
    if isinstance(d, dict):
        for reason, (code, msg) in _REASONS.items():
            if d.get("code") == code:
                return HTTPException(e.status, detail=erro(code, msg, reason=reason))
        texto = d.get("msg") if isinstance(d.get("msg"), str) else str(d)
    else:
        texto = str(d) if d is not None else str(e)
    # Texto que o outro lado escreveu vai rotulado: a tela não deve tomá-lo por mensagem do app.
    texto = _REMOTE_LABEL + texto[:300]
    return HTTPException(e.status, detail=erro("erro_par_recusado", texto, detalhe=texto))


async def _undo_remote(address: str, token: str) -> bool:
    try:
        await asyncio.to_thread(external_pairs.call, address, token, "DELETE", "/api/pair")
    except (peers.PeerError, ValueError) as ex:
        _log.warning("par externo: outro lado não desfeito (%s): %s", address, ex)
        return False
    return True


@router.post("/api/sessions/{name}/pair-accept", dependencies=[Depends(require_auth)])
async def pair_accept(name: str, body: PairAcceptBody):
    from app import api
    parsed = external_pairs.parse_pair_link(body.link)
    if parsed is None or not _CODE_RE.fullmatch(parsed[1]):
        raise HTTPException(400, detail=erro("erro_par_link_invalido", "link de par inválido"))
    address, code = parsed
    life = await asyncio.to_thread(session_life, name)
    if life is None:
        raise HTTPException(404, detail=erro("erro_sessao_inexistente", "sessão não encontrada"))
    # Antes de chamar o outro lado: recusar depois queimaria o código dele à toa.
    if await asyncio.to_thread(lambda: pair.PairLink(name).get()) is not None:
        raise HTTPException(409, detail=erro("erro_pareamento_mistura_cross", _MSG_JA_PAREADA))
    if share_tunnel.port_clash():
        raise HTTPException(409, detail=erro("erro_compartilhar_porta_do_convite", "porta do convite",
                                             port=share_tunnel.GUEST_PORT))
    try:
        my_base = await asyncio.to_thread(share_tunnel.ensure_on)
    except share_tunnel.TunnelError as e:
        raise share_api._prereq_error(e)
    # Vale antes do resgate: recado do outro lado pode chegar antes de este lado terminar.
    mine, my_token = await asyncio.to_thread(share_store.create_redeemed, name, life, "pair")
    try:
        _, resp = await asyncio.to_thread(external_pairs.call, address, None, "POST", "/api/pair/redeem",
                                          {"code": code, "session": name, "address": my_base,
                                           "token": my_token, "owner": _my_owner()})
    except peers.PeerError as e:
        await _guarded_async("revogar o convite", share_store.revoke, mine.id)
        if e.status in (404, 410, 409, 400):
            raise _refused(e)
        if e.transport:
            # A chamada pode ter chegado e gravado o par lá; só a resposta se perdeu.
            raise HTTPException(502, detail=erro("erro_par_incerto", "não deu para confirmar o pareamento"))
        raise _fora_do_ar(str(e), remoto=e.status is not None)
    resp = resp if isinstance(resp, dict) else {}
    owner, session, token = resp.get("owner", ""), resp.get("session", ""), resp.get("token", "")
    if not (isinstance(owner, str) and isinstance(session, str) and isinstance(token, str)
            and external_pairs.valid_owner(owner) and external_pairs.valid_session(session)
            and external_pairs.valid_token(token)):
        await _guarded_async("revogar o convite", share_store.revoke, mine.id)
        # O outro lado já gravou o par com o nosso token: desfaz lá, se o token dele serve pra isso.
        desfeito = True
        if isinstance(token, str) and token and token.isascii() and token.isprintable():
            desfeito = await _undo_remote(address, token)
        if not desfeito:
            raise HTTPException(502, detail=erro("erro_pareamento_desfeito_parcial", _MSG_PARCIAL))
        raise HTTPException(502, detail=erro("erro_par_resposta_invalida", "resposta do par inválida"))
    alias = external_pairs.free_alias(owner)
    peer = f"{alias}::{session}"
    snap = None
    try:
        await asyncio.to_thread(external_pairs.add, ExternalPair(
            mine.id, name, alias, owner, session, address, token, time.time()))
        harness = {s.name: s.provider for s in await asyncio.to_thread(api.registry.list)}
        snap = await _join_external(name, peer, harness)
        falha = await api._deliver(name, pair_texto.texto_par_externo(name, peer, owner))
        if falha:
            raise RuntimeError(api._erro_texto(falha))
    except Exception as e:  # noqa: BLE001 — qualquer falha aqui desfaz os dois lados
        if snap is not None:
            await _guarded_async("restaurar o grupo", _restore_external, snap)
        await _guarded_async("revogar o convite", share_store.revoke, mine.id)
        desfeito = await _undo_remote(address, token)
        await _guarded_async("remover o registro", external_pairs.remove, mine.id)
        if not desfeito:
            raise HTTPException(502, detail=erro("erro_pareamento_desfeito_parcial", _MSG_PARCIAL))
        raise HTTPException(502, detail=erro("erro_pareamento_desfeito",
                                             f"pareamento desfeito: falha ao avisar as sessões ({e})",
                                             avisos=str(e)))
    return {"ok": True, "alias": alias, "owner": owner, "session": session}


async def teardown(rec: ExternalPair, notify: bool) -> None:
    from app import api
    # A saída do grupo vem primeiro e sem engolir a falha: registro e convite ficam, e o outro lado
    # tenta de novo com o token que ainda vale, em vez de deixar o sidecar com um par sem registro.
    await asyncio.to_thread(_leave_external, rec.local_session, rec.address)
    await _guarded_async("remover o registro", external_pairs.remove, rec.share_id)
    await _guarded_async("revogar o convite", share_store.revoke, rec.share_id)
    if notify:
        falha = await api._deliver(rec.local_session,
                                   f"[painel: par externo encerrado] '{rec.address}' saiu do pareamento. "
                                   "Volte a operar independente.")
        if falha:
            _log.warning("par externo: aviso de encerramento a '%s' falhou: %s", rec.local_session,
                         api._erro_texto(falha))


def _pair_of(request: Request):
    guest = guest_of(request)
    share = guest.pair_share() if guest else None
    if share is None:
        raise HTTPException(403, detail=erro("erro_fora_do_convite", "fora do par"))
    return share


class PairMessageBody(BaseModel):
    text: str = Field(max_length=64000)


@router.post("/api/pair/message")
async def pair_message(body: PairMessageBody, request: Request):
    from app import api
    share = _pair_of(request)
    rec = external_pairs.by_share(share.id)
    if rec is None:
        raise HTTPException(503, detail=erro("erro_sessao_indisponivel", "par ainda abrindo"),
                            headers={"Retry-After": "5"})
    if api._group_estourou(f"ext:{share.id}", time.time()):
        raise HTTPException(429, detail=erro(
            "erro_group_message_tempestade",
            f"mais de {api._GROUP_MAX_NA_JANELA} avisos de grupo em {api._GROUP_JANELA_S}s — parece loop; "
            "espere ou responda 1:1", max=api._GROUP_MAX_NA_JANELA, janela=api._GROUP_JANELA_S))
    # O cabeçalho sai do registro (carimbado pelo token), nunca do texto que o outro lado mandou.
    texto = f"[de fora: {rec.address}] {external_pairs.sanitize_message(body.text)}"
    return await api.input_prompt(share.session, api.InputBody(text=texto, steer=True))


@router.delete("/api/pair")
async def pair_leave(request: Request):
    share = _pair_of(request)
    rec = external_pairs.by_share(share.id)
    if rec is not None:
        await teardown(rec, notify=True)
    else:
        await asyncio.to_thread(share_store.revoke, share.id)
    return {"ok": True}


def _owner_only(request: Request) -> None:
    if guest_of(request) is not None:
        raise HTTPException(403, detail=erro("erro_fora_do_convite", "só o dono"))


@router.get("/api/external-pairs", dependencies=[Depends(require_auth)])
async def list_external_pairs(request: Request):
    _owner_only(request)
    return [{"local_session": r.local_session, "alias": r.alias, "owner": r.peer_owner,
             "session": r.peer_session, "address": r.peer_address, "token": r.peer_token}
            for r in external_pairs.all()]


class ExternalSendBody(BaseModel):
    sender: str
    target: str
    text: str


def _fora_do_ar(texto: str, remoto: bool) -> HTTPException:
    # Texto que o outro lado escreveu vai rotulado: a tela não deve tomá-lo por mensagem do app.
    detalhe = (_REMOTE_LABEL if remoto else "") + texto[:300]
    return HTTPException(502, detail=erro("erro_par_fora_do_ar", detalhe, detalhe=detalhe))


def _remote_failure(e: Exception) -> HTTPException:
    status = e.status if isinstance(e, peers.PeerError) else None
    d = e.detail if isinstance(e, peers.PeerError) else None
    if status == 429:
        # Só os dois números que a mensagem usa: params do outro lado não entram em erro() soltos.
        raw = d.get("params") if isinstance(d, dict) and isinstance(d.get("params"), dict) else {}
        params = {k: raw[k] for k in ("max", "janela")
                  if isinstance(raw.get(k), int) and not isinstance(raw.get(k), bool)}
        return HTTPException(429, detail=erro("erro_group_message_tempestade",
                                              "recados demais em pouco tempo", **params))
    if status == 503:
        return HTTPException(503, detail=erro("erro_sessao_indisponivel", "o par ainda está abrindo"))
    if isinstance(d, dict) and isinstance(d.get("msg"), str):
        return _fora_do_ar(d["msg"], remoto=True)
    return _fora_do_ar(str(d) if d is not None else str(e), remoto=d is not None)


_RESP_KEYS = ("ok", "delivered", "steered", "native", "queued")


async def send_external(rec: ExternalPair, text: str) -> dict:
    try:
        _, resp = await asyncio.to_thread(external_pairs.call, rec.peer_address, rec.peer_token,
                                          "POST", "/api/pair/message", {"text": text})
    except (peers.PeerError, ValueError) as e:
        if isinstance(e, peers.PeerError) and e.status == 410:
            await teardown(rec, notify=True)
            raise HTTPException(410, detail=erro("erro_par_encerrado", "o par foi encerrado do outro lado"))
        raise _remote_failure(e)
    # Só as chaves que o app conhece, com valor primitivo: o dict do outro lado nunca passa adiante.
    resp = resp if isinstance(resp, dict) else {}
    return {k: resp[k] for k in _RESP_KEYS if isinstance(resp.get(k), (bool, int, float))}


@router.post("/api/external-pairs/send", dependencies=[Depends(require_auth)])
async def external_send(body: ExternalSendBody, request: Request):
    _owner_only(request)
    alias = body.target.split("::", 1)[0]
    if external_pairs.ambiguous(alias):
        raise HTTPException(409, detail=erro("erro_par_endereco_ambiguo",
                                             f"'{alias}' é ao mesmo tempo máquina tua e par externo"))
    rec = external_pairs.by_address(body.target)
    if rec is None or rec.local_session != body.sender:
        raise HTTPException(404, detail=erro("erro_par_inexistente", "este par externo não existe"))
    return await send_external(rec, body.text)


class AttachBody(BaseModel):
    token: str


@router.post("/api/guest/attach")
async def guest_attach(body: AttachBody, request: Request):
    if guest_of(request) is None:
        raise HTTPException(403, detail=erro("erro_fora_do_convite", "só convidado"))
    mine = request.scope.get(GUEST_TOKEN_KEY, "")
    other = await asyncio.to_thread(share_store.lookup_token, body.token)
    if other is None or not other.sessions():
        raise HTTPException(409, detail=erro("erro_par_attach_invalido", "token a ligar inválido ou sem sessão viva"))
    return {"attached": await asyncio.to_thread(share_store.attach, mine, body.token)}
