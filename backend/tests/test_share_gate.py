import asyncio
import dataclasses
import socket

import pytest
from fastapi import FastAPI, Request, WebSocket
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse
from fastapi.testclient import TestClient
from starlette.websockets import WebSocketDisconnect

from app import share_api, share_gate, share_store, termsock, tmux
from app.config import settings

SHARED = share_store.Share(
    id="s1", session="cc", life="L1", created_at=0.0, code_expires_at=0.0, code_hash="c",
    token_hash="t", device="Pixel", redeemed_at=1.0, revoked_at=None)
PAR = share_store.Share(
    id="p1", session="yy", life="L2", created_at=0.0, code_expires_at=0.0, code_hash="",
    token_hash="t", device=None, redeemed_at=1.0, revoked_at=None, kind="pair")
GUEST = {"Authorization": "Bearer g"}
# Simula `share_store.revoke(...)` no meio de um stream: o próximo lookup devolve o revogado.
STATE = {"revoked": False, "unsure": False}


def _lookup(t):
    if t != "g":
        return None
    return share_store.Guest([dataclasses.replace(SHARED, revoked_at=2.0) if STATE["revoked"] else SHARED])


def _life(name):
    # `unsure` simula o tmux que parou de responder no meio: a vida some, a ausência não é confirmada.
    if name not in ("cc", "dd", "yy") or STATE["unsure"]:
        return None
    return {"yy": "L2"}.get(name, "L1")


@pytest.fixture(autouse=True)
def _fakes(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", "secret")
    STATE["revoked"] = False
    STATE["unsure"] = False
    share_gate._life_cache.clear()
    share_api.changing_mode.discard("cc")
    monkeypatch.setattr(share_store, "lookup_token", _lookup)
    monkeypatch.setattr(share_gate, "session_life", _life)
    monkeypatch.setattr(share_api, "confirmed_absent", lambda name: False)
    yield
    share_api.changing_mode.discard("cc")


def _app():
    a = FastAPI()

    @a.get("/api/sessions/events")
    async def lista():
        async def corpo():
            # Teto de 500 pedaços (~5 s): se o vigia falhar, o teste termina e acusa pela contagem.
            for i in range(500):
                yield f"data: {i}\n\n"
                if i == 0:
                    STATE["revoked"] = True
                await asyncio.sleep(0.01)
        return StreamingResponse(corpo(), media_type="text/event-stream")

    @a.get("/api/sessions/{name}/events")
    async def eventos(name: str):
        async def corpo():
            for i in range(30):
                yield f"data: {i}\n\n"
                if i == 0:
                    STATE["unsure"] = True
                await asyncio.sleep(0.02)
        return StreamingResponse(corpo(), media_type="text/event-stream")

    # Todo handler de WebSocket tem teto de 3 s: se o vigia falhar, o teste acusa em vez de travar.
    @a.websocket("/api/sessions/{name}/term-hold")
    async def terminal_aberto(ws: WebSocket, name: str):
        # Terminal que fica aberto: só o vigia do porteiro fecha.
        await ws.accept()
        await ws.send_text("aberto")
        STATE["revoked"] = True
        await asyncio.wait_for(ws.receive_text(), 3)

    @a.websocket("/api/sessions/{name}/term-hold-finally")
    async def terminal_com_finally(ws: WebSocket, name: str):
        # Como termsock/navsock: o cancelamento cai num `finally` que fecha com 1000.
        await ws.accept()
        await ws.send_text("aberto")
        STATE["revoked"] = True
        try:
            await asyncio.wait_for(ws.receive_text(), 3)
        finally:
            await ws.close()

    @a.websocket("/api/sessions/{name}/term")
    async def term(ws: WebSocket, name: str):
        prep = await termsock._porta_de_entrada(ws, name)
        if prep:
            await ws.accept()
            await ws.send_text(f"ok {prep[2]}")
            await ws.close()

    @a.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE"])
    def qualquer(path: str, request: Request):
        g = share_gate.guest_of(request)
        return {"path": path, "guest": ",".join(sorted(g.sessions())) if g else None}

    a.add_middleware(share_gate.ShareGate)
    a.add_middleware(CORSMiddleware, allow_origins=["*"], allow_methods=["*"], allow_headers=["*"])
    return a


def _guest_client():
    return TestClient(_app(), base_url="http://127.0.0.1:8766", client=("203.0.113.9", 1))


def test_porta_do_dono_nao_passa_pelo_porteiro():
    c = TestClient(_app(), base_url="http://127.0.0.1:8765")
    r = c.get("/api/config")
    assert r.status_code == 200 and r.json()["guest"] is None


def test_rota_da_sessao_marca_o_convidado():
    r = _guest_client().get("/api/sessions/cc/history", headers=GUEST)
    assert r.status_code == 200 and r.json()["guest"] == "cc"


def test_token_por_query_vale():
    assert _guest_client().get("/api/sessions/cc/events?token=g").status_code == 200


def test_token_do_dono_e_cookie_recusados():
    c = _guest_client()
    assert c.get("/api/sessions/cc/history", headers={"Authorization": "Bearer secret"}).status_code == 401
    c.cookies.set("cp_token", "g")
    assert c.get("/api/sessions/cc/history").status_code == 401


def test_outra_sessao_e_rota_global_fora_da_lista_dao_403():
    c = _guest_client()
    for path in ("/api/sessions/outra/history", "/api/config", "/api/fs/roots", "/"):
        r = c.get(path, headers=GUEST)
        assert r.status_code == 403, path
        assert r.json()["detail"]["code"] == "erro_fora_do_convite"


@pytest.mark.parametrize("path", [
    "/api/sessions/cc/pair", "/api/sessions/cc/pair-remote", "/api/sessions/cc/group-message",
    "/api/sessions/cc/orq/papel", "/api/sessions/cc/bastao", "/api/sessions/cc/open-terminal",
    "/api/sessions/cc/open-editor", "/api/sessions/cc/nav", "/api/sessions/cc/share"])
def test_lista_de_bloqueio(path):
    assert _guest_client().post(path, headers=GUEST).status_code == 403


def test_rotas_globais_do_chat_passam():
    c = _guest_client()
    assert c.get("/api/model-options", headers=GUEST).status_code == 200
    assert c.post("/api/pensamento/pt", headers=GUEST).status_code == 200
    assert c.get("/api/tts/audio/abc", headers=GUEST).status_code == 200


def test_token_com_duas_sessoes_alcanca_as_duas(monkeypatch):
    outra = dataclasses.replace(SHARED, id="s2", session="dd")
    monkeypatch.setattr(share_store, "lookup_token",
                        lambda t: share_store.Guest([SHARED, outra]) if t == "g" else None)
    c = _guest_client()
    assert c.get("/api/sessions/cc/history", headers=GUEST).status_code == 200
    assert c.get("/api/sessions/dd/history", headers=GUEST).status_code == 200
    assert c.get("/api/sessions/ee/history", headers=GUEST).status_code == 403


def test_sessao_de_par_so_le(monkeypatch):
    monkeypatch.setattr(share_store, "lookup_token",
                        lambda t: share_store.Guest([SHARED, PAR]) if t == "g" else None)
    c = _guest_client()
    assert c.get("/api/sessions/yy/history", headers=GUEST).status_code == 200
    assert c.post("/api/sessions/yy/input", headers=GUEST, json={"text": "x"}).status_code == 403
    assert c.get("/api/sessions/yy/file?path=/etc/passwd", headers=GUEST).status_code == 403
    assert c.post("/api/sessions/cc/input", headers=GUEST, json={"text": "x"}).status_code == 200


@pytest.mark.parametrize("acao", ["pair-invite", "pair-accept"])
def test_convidado_nao_cria_nem_aceita_par(acao):
    c = _guest_client()
    assert c.post(f"/api/sessions/cc/{acao}", headers=GUEST, json={}).status_code == 403


def test_convidado_nao_fecha_a_sessao_do_dono():
    # Fechar mata a sessão do dono; o convidado só para de acompanhar do lado dele.
    assert _guest_client().delete("/api/sessions/cc", headers=GUEST).status_code == 403


def test_revogado_da_410():
    STATE["revoked"] = True
    r = _guest_client().get("/api/sessions/cc/history", headers=GUEST)
    assert r.status_code == 410
    assert r.json()["detail"]["code"] == "erro_convite_encerrado"


def test_token_desconhecido_da_401():
    r = _guest_client().get("/api/sessions/cc/history", headers={"Authorization": "Bearer x"})
    assert r.status_code == 401


def test_stream_aberto_termina_quando_o_acesso_e_revogado(monkeypatch):
    monkeypatch.setattr(share_gate, "WATCH_INTERVAL", 0.05)
    r = _guest_client().get("/api/sessions/events", headers=GUEST)
    pedacos = [l for l in r.text.splitlines() if l.startswith("data:")]
    assert 1 <= len(pedacos) < 500


# websocket_connect junta a URL com `ws://testserver` e ignora o base_url do client: só URL
# absoluta faz o scope chegar com a porta 8766 e passar pelo porteiro.
GUEST_WS = "ws://127.0.0.1:8766"


@pytest.mark.parametrize("rota", ["term-hold", "term-hold-finally"])
def test_websocket_aberto_fecha_com_4410_quando_o_acesso_e_revogado(monkeypatch, rota):
    monkeypatch.setattr(share_gate, "WATCH_INTERVAL", 0.05)
    with _guest_client().websocket_connect(f"{GUEST_WS}/api/sessions/cc/{rota}?token=g") as ws:
        assert ws.receive_text() == "aberto"
        with pytest.raises(WebSocketDisconnect) as e:
            ws.receive_text()
    assert e.value.code == 4410


def test_troca_de_modo_da_503_e_nao_410():
    share_api.changing_mode.add("cc")
    r = _guest_client().get("/api/sessions/cc/history", headers=GUEST)
    assert r.status_code == 503
    assert r.headers["retry-after"] == "5"


def test_vida_sem_resposta_do_tmux_da_503_e_ausencia_confirmada_da_410(monkeypatch):
    STATE["unsure"] = True
    c = _guest_client()
    r = c.get("/api/sessions/cc/history", headers=GUEST)
    assert r.status_code == 503 and r.headers["retry-after"] == "5"
    monkeypatch.setattr(share_api, "confirmed_absent", lambda name: True)
    assert c.get("/api/sessions/cc/history", headers=GUEST).status_code == 410


def test_stream_aberto_sobrevive_a_incerteza(monkeypatch):
    monkeypatch.setattr(share_gate, "WATCH_INTERVAL", 0.05)
    monkeypatch.setattr(share_gate, "_LIFE_TTL", 0)
    r = _guest_client().get("/api/sessions/cc/events", headers=GUEST)
    assert len([l for l in r.text.splitlines() if l.startswith("data:")]) == 30


def test_stream_aberto_termina_quando_a_ausencia_e_confirmada(monkeypatch):
    monkeypatch.setattr(share_gate, "WATCH_INTERVAL", 0.05)
    monkeypatch.setattr(share_gate, "_LIFE_TTL", 0)
    monkeypatch.setattr(share_api, "confirmed_absent", lambda name: True)
    r = _guest_client().get("/api/sessions/cc/events", headers=GUEST)
    assert 1 <= len([l for l in r.text.splitlines() if l.startswith("data:")]) < 30


def test_sessao_recriada_com_o_mesmo_nome_da_410(monkeypatch):
    monkeypatch.setattr(share_gate, "session_life", lambda name: "OUTRA-VIDA")
    assert _guest_client().get("/api/sessions/cc/history", headers=GUEST).status_code == 410


def test_rotas_abertas_sem_token():
    c = _guest_client()
    assert c.get("/convite/ABC").status_code == 200
    assert c.post("/api/guest/redeem").status_code == 200


def test_403_sai_com_cabecalho_cors_e_preflight_passa():
    c = _guest_client()
    r = c.get("/api/config", headers={**GUEST, "Origin": "https://app-do-convidado.example"})
    assert r.status_code == 403
    assert r.headers["access-control-allow-origin"] == "*"
    pre = c.options("/api/sessions/cc/input", headers={
        "Origin": "https://app-do-convidado.example", "Access-Control-Request-Method": "POST",
        "Access-Control-Request-Headers": "authorization"})
    assert pre.status_code == 200


def test_terminal_do_convidado_com_origem_estrangeira(monkeypatch):
    monkeypatch.setattr(tmux, "has_session", lambda name: name in ("cc", "term-cc"))
    c = _guest_client()
    for alvo in ("cc", "term-cc"):
        with c.websocket_connect(f"{GUEST_WS}/api/sessions/{alvo}/term?token=g",
                                 headers={"origin": "https://app-do-convidado.example"}) as ws:
            assert ws.receive_text() == f"ok {alvo}"


def test_terminal_de_outra_sessao_recusado(monkeypatch):
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    with pytest.raises(WebSocketDisconnect):
        with _guest_client().websocket_connect(f"{GUEST_WS}/api/sessions/outra/term?token=g") as ws:
            ws.receive_text()


def test_app_na_porta_do_convite_nao_passa_pelo_porteiro(monkeypatch):
    # CP_PORT=8766: sem essa saída, todo pedido do dono levava 401, até a sonda de saúde.
    from app import main
    monkeypatch.setattr(settings, "port", 8766)
    c = _guest_client()
    assert c.get("/api/peers/ping").json() == {"path": "api/peers/ping", "guest": None}
    r = c.get("/api/sessions", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200 and r.json()["guest"] is None
    assert main._guest_socket() is None


def test_porta_do_convite_ocupada_nao_derruba_o_boot(monkeypatch):
    from app import main
    ocupada = socket.socket()
    ocupada.bind(("127.0.0.1", 0))
    ocupada.listen()
    monkeypatch.setattr(main, "GUEST_PORT", ocupada.getsockname()[1])
    try:
        assert main._guest_socket() is None
    finally:
        ocupada.close()


def test_vida_em_cache_de_antes_da_troca_de_modo_nao_da_410():
    # O pedido do próprio convidado deixou a vida ANTIGA no cache; a troca terminou e o convite
    # já aponta para a nova. Comparar cache velho com registro novo não pode encerrar o convite.
    import time
    share_gate._life_cache["cc"] = (time.monotonic(), "L0")
    r = _guest_client().get("/api/sessions/cc/history", headers=GUEST)
    assert r.status_code == 200


def test_vida_diferente_de_verdade_continua_dando_410(monkeypatch):
    import time
    share_gate._life_cache["cc"] = (time.monotonic(), "L0")
    monkeypatch.setattr(share_gate, "session_life", lambda name: "OUTRA-VIDA")
    assert _guest_client().get("/api/sessions/cc/history", headers=GUEST).status_code == 410


@pytest.mark.parametrize("nome,reuse", [("nt", False), ("posix", True)])
def test_socket_principal_so_reusa_endereco_fora_do_windows(monkeypatch, nome, reuse):
    # Windows: SO_REUSEADDR deixaria um segundo backend abrir a 8765 ao lado do primeiro.
    # `main.os` trocado inteiro: patchar `os.name` levaria o pathlib junto.
    import types
    from app import main
    opts = []

    class Fake:
        def __init__(self, *a):
            pass

        def setsockopt(self, *a):
            opts.append(a)

        def bind(self, addr):
            pass

        def set_inheritable(self, v):
            pass

    monkeypatch.setattr(main, "os", types.SimpleNamespace(name=nome))
    monkeypatch.setattr(main, "socket", types.SimpleNamespace(
        socket=Fake, AF_INET=2, AF_INET6=10, SOCK_STREAM=1, SOL_SOCKET=1, SO_REUSEADDR=2))
    main._tcp_socket("127.0.0.1", 8765)
    assert bool(opts) is reuse


def test_socket_principal_ocupado_levanta_oserror():
    from app import main
    ocupada = socket.socket()
    ocupada.bind(("127.0.0.1", 0))
    ocupada.listen()
    try:
        with pytest.raises(OSError):
            main._tcp_socket("127.0.0.1", ocupada.getsockname()[1])
    finally:
        ocupada.close()


def test_socket_principal_nao_e_herdado_por_processos_filhos():
    from app import main
    s = main._tcp_socket("127.0.0.1", 0)
    try:
        assert s.get_inheritable() is False
    finally:
        s.close()


def test_guest_of_sem_scope_devolve_none():
    assert share_gate.guest_of(object()) is None


@pytest.mark.parametrize("path,esperado", [
    ("/api/sessions/yy/uploads/foto.png", 200), ("/api/sessions/yy/uploads", 403),
    ("/api/sessions/yy/runners", 403), ("/api/sessions/yy/project-shortcuts", 403),
    ("/api/sessions/yy/shortcut-terminals", 403), ("/api/sessions/yy/transcript-image", 200)])
def test_par_le_so_o_que_esta_na_lista(monkeypatch, path, esperado):
    monkeypatch.setattr(share_store, "lookup_token",
                        lambda t: share_store.Guest([PAR]) if t == "g" else None)
    assert _guest_client().get(path, headers=GUEST).status_code == esperado


def test_path_session_so_tira_term_do_terminal():
    assert share_gate.path_session("/api/sessions/term-cc/term") == "cc"
    assert share_gate.path_session("/api/sessions/term-cc/history") == "term-cc"
    assert share_gate.path_session("/api/sessions/term-cc") == "term-cc"
    assert share_gate.path_session("/api/sessions/cc/history") == "cc"


def test_sessao_compartilhada_e_pareada_no_mesmo_token_passa_como_convite(monkeypatch):
    copia_do_par = dataclasses.replace(PAR, id="p2", session="cc", life="L1", created_at=5.0, parent_id="p1")
    monkeypatch.setattr(share_store, "lookup_token",
                        lambda t: share_store.Guest([SHARED, copia_do_par]) if t == "g" else None)
    r = _guest_client().post("/api/sessions/cc/input", headers=GUEST, json={"text": "x"})
    assert r.status_code == 200


@pytest.mark.parametrize("path", ["/api/engines", "/api/model-options", "/api/harness/codex/opcoes",
                                  "/api/tts/audio/abc"])
def test_token_so_de_par_nao_alcanca_rotas_globais(monkeypatch, path):
    monkeypatch.setattr(share_store, "lookup_token",
                        lambda t: share_store.Guest([PAR]) if t == "g" else None)
    assert _guest_client().get(path, headers=GUEST).status_code == 403
