import asyncio
import io
import json
import urllib.error
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
from fastapi import HTTPException, Request, Response

from app import auth, diag, peers, registry, sse, sync


SECRET = "segredo-que-nao-pode-aparecer@example.invalid"


@pytest.fixture
def diario(tmp_path, monkeypatch):
    monkeypatch.setattr(diag, "_base", lambda: tmp_path / "diario")
    token = diag.req_atual.set("req-diagnostico")
    yield lambda: [json.loads(line) for line in diag.caminho_do_dia().read_text().splitlines()]
    diag.req_atual.reset(token)


def _request(headers=(), query=b"", ip="127.0.0.1"):
    return Request({"type": "http", "method": "GET", "scheme": "http", "path": "/",
                    "headers": list(headers), "query_string": query, "client": (ip, 1234),
                    "server": ("localhost", 80)})


@pytest.mark.parametrize("mecanismo", ["bearer", "query", "cookie", "ausente"])
def test_auth_registra_transicao_e_recuperacao_sem_credenciais(diario, monkeypatch, mecanismo):
    monkeypatch.setattr(auth.settings, "auth_token", "token-correto")
    headers = {
        "bearer": [(b"authorization", f"Bearer {SECRET}".encode())],
        "cookie": [(b"cookie", f"cp_token={SECRET}".encode())],
    }.get(mecanismo, [])
    query = f"token={SECRET}".encode() if mecanismo == "query" else b""
    for _ in range(3):
        with pytest.raises(HTTPException) as exc:
            auth.require_auth(_request(headers, query))
        assert exc.value.status_code == 401
    for _ in range(2):
        auth.require_auth(_request([(b"authorization", b"Bearer token-correto")]))
    rows = diario()
    assert [r["evento"] for r in rows] == ["auth.recusado", "auth.recuperado"]
    assert rows[0]["etapa"] == mecanismo
    assert all(r["req"] == "req-diagnostico" for r in rows)
    assert SECRET not in json.dumps(rows) and "127.0.0.1" not in json.dumps(rows)


def test_auth_bloqueio_e_recuperacao_sem_repetir_poll(diario, monkeypatch):
    monkeypatch.setattr(auth.settings, "auth_token", "token-correto")
    clock = [1000.0]
    monkeypatch.setattr(auth.time, "time", lambda: clock[0])
    bad = _request([(b"authorization", SECRET.encode())], ip="192.0.2.15")
    for _ in range(auth._MAX_FAILS + 3):
        with pytest.raises(HTTPException):
            auth.require_auth(bad)
    clock[0] += auth._WINDOW + 1
    auth.require_auth(_request([(b"authorization", b"Bearer token-correto")], ip="192.0.2.15"))
    rows = diario()
    assert [r["detalhe"] for r in rows] == ["token_ausente", "bloqueio_temporario", "acesso_restabelecido"]
    assert "192.0.2.15" not in json.dumps(rows)


@pytest.mark.parametrize("resultado", ["ok", "http", "transporte", "json"])
def test_peer_liga_requisicao_e_preserva_causa_sem_body(diario, monkeypatch, resultado):
    monkeypatch.setattr(peers, "peer_cfg", lambda _: ("http://peer.invalid", SECRET))
    outgoing = []

    def abrir(request, **kwargs):
        outgoing.append(request)
        if resultado == "http":
            raise urllib.error.HTTPError(request.full_url, 403, SECRET, {}, io.BytesIO(SECRET.encode()))
        if resultado == "transporte":
            raise urllib.error.URLError(TimeoutError(SECRET))
        response = MagicMock()
        response.__enter__.return_value = SimpleNamespace(
            status=200, read=lambda: (SECRET if resultado == "json" else json.dumps({"token": SECRET})).encode())
        return response

    monkeypatch.setattr(peers.urllib.request, "urlopen", abrir)
    if resultado == "ok":
        assert peers.call("outro", "POST", "/api/pair", {"texto": SECRET}) == (200, {"token": SECRET})
    else:
        with pytest.raises(peers.PeerError) as exc:
            peers.call("outro", "POST", "/api/pair", {"texto": SECRET})
        assert exc.value.transport is (resultado != "http")
    assert outgoing[0].get_header("X-hangar-req") == "req-diagnostico"
    rows = diario()
    event = next(r for r in rows if r["evento"] in {"peer.falhou", "peer.respondeu"})
    assert event["ms"] >= 0 and event["etapa"] == "POST"
    if resultado == "transporte":
        assert event["erro_tipo"] == "TimeoutError"
    assert all(r["req"] == "req-diagnostico" for r in rows)
    assert len({r["operacao"] for r in rows}) == 1
    assert SECRET not in json.dumps(rows)


def test_sync_registra_causas_e_resultado_sem_login_senha_ou_vault(diario, tmp_path, monkeypatch):
    monkeypatch.setattr(sync.settings, "sync_bootstrap", "bootstrap-correto")
    monkeypatch.setattr(sync, "_data_path", lambda: tmp_path / "vault.json")
    monkeypatch.setattr(sync, "_FAILS", {})
    body = sync.RegisterBody(user=SECRET, salt=SECRET, auth_hash=SECRET, bootstrap=SECRET)
    with pytest.raises(HTTPException):
        sync.register(body)
    sync.register(body.model_copy(update={"bootstrap": "bootstrap-correto"}))
    with pytest.raises(HTTPException):
        sync.login(sync.LoginBody(user=SECRET, auth_hash="incorreta"), _request(), Response())
    sync.login(sync.LoginBody(user=SECRET, auth_hash=SECRET), _request(), Response())
    with pytest.raises(HTTPException):
        sync.put_vault(sync.VaultPutBody(enc_blob={"texto": SECRET}, base_rev=10), SECRET)
    sync.put_vault(sync.VaultPutBody(enc_blob={"texto": SECRET}, base_rev=0), SECRET)
    rows = diario()
    assert {r.get("detalhe") for r in rows} >= {"bootstrap_invalido", "credenciais_invalidas", "revisao_desatualizada"}
    assert {r["evento"] for r in rows} >= {"sync.cadastrado", "sync.login_concluido", "sync.gravado"}
    assert SECRET not in json.dumps(rows)


@pytest.mark.parametrize("criou", [False, True])
def test_criacao_registra_etapas_sem_caminho_ou_comando(diario, tmp_path, monkeypatch, criou):
    reg = registry.SessionRegistry(projects_dir=tmp_path / "projects")
    monkeypatch.setattr(registry.tmux, "has_session", lambda _: False)
    monkeypatch.setattr(registry.codex_sessions, "exists", lambda _: False)
    monkeypatch.setattr(registry.tmux, "new_session", lambda *args, provider="claude": criou)
    monkeypatch.setattr(registry, "_pretrust_cwd", lambda *args: None)
    monkeypatch.setattr(registry.PromptQueue, "clear", lambda _: None)
    monkeypatch.setattr(registry.ThenLink, "clear", lambda _: None)
    monkeypatch.setattr(reg, "_clear_pair", lambda _: None)
    monkeypatch.setattr(reg, "_jsonl_cache", {})
    if criou:
        assert reg.create("teste", str(tmp_path / SECRET)).name == "teste"
    else:
        with pytest.raises(ValueError, match="falha ao criar"):
            reg.create("teste", str(tmp_path / SECRET))
    rows = diario()
    etapas = [r["etapa"] for r in rows if r["evento"] == "sessao.criar_etapa"]
    assert etapas[:4] == ["validar", "preparar_comando", "confiar_pasta", "criar_terminal"]
    assert ("limpar_estado_anterior" in etapas) is criou
    assert rows[-1]["codigo"] == ("retornou" if criou else "excecao")
    assert len({r["operacao"] for r in rows}) == 1
    assert SECRET not in json.dumps(rows)


@pytest.mark.asyncio
async def test_pump_sse_registra_causa_fechamento_sem_texto(diario, tmp_path, monkeypatch):
    async def vazio(*args):
        return
        yield

    async def falhar(*args):
        raise ValueError(SECRET)
        yield

    adapter = SimpleNamespace(transcript_stream=falhar, state_monitor=lambda *args, **kwargs: vazio())
    monkeypatch.setattr(sse, "get_adapter", lambda _: adapter)
    monkeypatch.setattr(sse._ESTADOS, "ouvir", lambda *args: vazio())
    monkeypatch.setattr(sse.PreviewBroker, "get", lambda *args: SimpleNamespace(subscribe=vazio))
    monkeypatch.setattr(sse.StatsAccumulator, "compartilhado", lambda *args: None)
    monkeypatch.setattr(sse.PromptQueue, "follow", lambda *args, **kwargs: vazio())
    with pytest.raises(ValueError, match=SECRET):
        async with asyncio.timeout(2):
            async for _ in sse.merged_events("teste", str(tmp_path / "nao-existe.jsonl")):
                pass
    rows = diario()
    assert [r["evento"] for r in rows] == ["sse.abriu", "sse.pump_falhou", "sse.fechou"]
    assert rows[1]["etapa"] == "transcript" and rows[1]["erro_tipo"] == "ValueError"
    assert rows[-1]["detalhe"] == "falha_pump" and rows[-1]["nivel"] == "erro"
    assert SECRET not in json.dumps(rows)
