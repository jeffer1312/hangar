"""A borda HTTP da escolha de modelo."""
import asyncio
import threading
from unittest.mock import AsyncMock, Mock, patch

import pytest
from fastapi.testclient import TestClient

from app.api import app, CreateBody, create_session
from app import api
from app.config import settings
from app.registry import SessionInfo
from app import codex_contas as codex_accounts

TOKEN = "t-modelo"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _auth(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)


def test_model_invalido_devolve_400():
    r = TestClient(app).post("/api/sessions", headers=AUTH, json={
        "name": "x", "cwd": "/tmp", "model": "k3; touch /tmp/x"})
    assert r.status_code == 400


def test_pi_com_til_no_id_chega_intacto_ao_create():
    """B1 da revisão final: o catálogo real do Pi traz `openrouter/~anthropic/claude-opus-latest`
    (11 ids com `~` entre 390). A tela oferece a linha; o backend tem que aceitar o id inteiro — a
    regex é a barreira do shell, e `~` citado por shlex.join não sofre expansão — e repassá-lo
    intacto ao create, sem corromper nada."""
    with patch("app.api.registry.create",
               return_value=SessionInfo(name="pi-til", cwd="/tmp", provider="pi")) as cr:
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "pi-til", "cwd": "/tmp", "provider": "pi",
            "model": "openrouter/~anthropic/claude-opus-latest", "effort": "high"})
    assert r.status_code == 200
    cr.assert_called_once_with("pi-til", "/tmp", None, provider="pi", engine=None,
                               model="openrouter/~anthropic/claude-opus-latest",
                               effort="high", context_window=None)


def test_effort_fora_da_lista_devolve_400():
    r = TestClient(app).post("/api/sessions", headers=AUTH, json={
        "name": "x", "cwd": "/tmp", "provider": "claude", "effort": "turbo"})
    assert r.status_code == 400


def test_codex_sem_modelo_nao_e_barrado_pela_validacao():
    """Regressão: a validação nova não pode transformar criação de Codex em 400. O registry.create
    vai mocado (convenção de tests/test_api.py:790) — sem isso o teste sobe um pane de verdade na
    máquina, e da 2ª rodada em diante passa por 409, sem exercitar nada."""
    with patch("app.api.registry.create",
               return_value=SessionInfo(name="cx-modelo", cwd="/tmp", provider="codex")) as cr:
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "cx-modelo", "cwd": "/tmp", "provider": "codex"})
    assert r.status_code == 200
    cr.assert_called_once()


def test_kimi_sem_modelo_nao_e_barrado_pela_validacao():
    """Irmão do Codex: o Kimi também é provider fora de escopo, e o caminho dele é o
    registry.create (não o create_codex) — sem a guarda de mock, o teste subiria um pane tmux
    de verdade (o mesmo defeito que o teste do Codex tinha)."""
    with patch("app.api.registry.create",
               return_value=SessionInfo(name="km", cwd="/tmp", provider="kimi")) as cr:
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "km", "cwd": "/tmp", "provider": "kimi"})
    assert r.status_code == 200
    cr.assert_called_once_with("km", "/tmp", None, provider="kimi", engine=None,
                               model=None, effort=None, context_window=None)


@pytest.mark.parametrize("provider", ["claude", "pi", "kimi", "omp"])
def test_codex_account_so_vale_para_codex(provider):
    with patch("app.api.registry.create") as cr:
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": provider, "codex_account": "work"})
    assert r.status_code == 400
    cr.assert_not_called()


def test_conta_codex_desconhecida_falha_antes_do_tmux(monkeypatch):
    def fail(_account):
        raise codex_accounts.AccountError(404, "codex_account_not_found", {"account_id": "work"})

    monkeypatch.setattr(codex_accounts, "resolve_account", fail)
    with patch("app.api.registry.create") as cr:
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": "codex", "codex_account": "work"})
    assert r.status_code == 404
    cr.assert_not_called()


def test_preparacao_codex_em_andamento_nao_impede_sessao(monkeypatch, tmp_path):
    account = codex_accounts.Account("work", tmp_path / ".codex-work", False)
    monkeypatch.setattr(codex_accounts, "resolve_account", lambda _: account)

    class Service:
        def __init__(self):
            self.preparou = threading.Event()
        def preparation_status(self, _account):
            return {"status": "running", "issues": []}
        def reserve_creation(self, _account):
            return Mock(mark_live=Mock(), release=Mock())
        async def prepare(self, _account):
            self.preparou.set()
            return {"status": "running", "issues": []}

    service = Service()
    monkeypatch.setattr(app.state, "codex_contas_login", service, raising=False)
    monkeypatch.setattr(api.tmux, "has_session", lambda _name: False)
    with patch("app.api.registry.create", return_value=SessionInfo(
            name="x", cwd="/tmp", provider="codex")) as cr:
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": "codex", "codex_account": "work"})
    assert r.status_code == 200
    assert service.preparou.wait(1)
    cr.assert_called_once()


@pytest.mark.parametrize("status", ["partial", "error", "idle"])
def test_sync_incompleta_nao_bloqueia_sessao_nem_modelos(monkeypatch, tmp_path, status):
    account = codex_accounts.Account("work", tmp_path / ".codex-work", False)
    monkeypatch.setattr(codex_accounts, "resolve_account", lambda _: account)
    snapshot = {"status": status, "issues": [
        {"code": "codex_account_mcp_runtime_excluded", "params": {}},
        {"code": "codex_account_plugin_marketplace_unavailable", "params": {}},
    ]}
    service = Mock()
    service.preparation_status.return_value = snapshot
    service.prepare = AsyncMock(return_value={"status": "running", "issues": []})
    monkeypatch.setattr(app.state, "codex_contas_login", service, raising=False)
    monkeypatch.setattr(api.tmux, "has_session", lambda _: False)
    with patch("app.api.registry.create", return_value=SessionInfo(
            name="cx-pending", cwd="/tmp", provider="codex")) as create, \
            patch("app.api.codex_models.listar", return_value=[]) as models:
        client = TestClient(app)
        response = client.post("/api/sessions", headers=AUTH, json={
            "name": "cx-pending", "cwd": "/tmp", "provider": "codex", "codex_account": "work"})
        assert response.status_code == 200, response.text
        service.prepare.assert_awaited_once_with(account)
        response = client.get("/api/model-options?provider=codex&codex_account=work", headers=AUTH)
        assert response.status_code == 200, response.text
    assert create.call_args.kwargs["codex_account"] == "work"
    models.assert_called_once_with(codex_home=account.home)
    assert snapshot["status"] == status


def test_criacao_codex_padrao_sem_campo_tambem_reserva(monkeypatch, tmp_path):
    account = codex_accounts.Account("default", tmp_path / ".codex", True)
    monkeypatch.setattr(codex_accounts, "resolve_account", lambda _: account)

    class Lease:
        def __init__(self):
            self.marked = None
        def mark_live(self, name):
            self.marked = name
        def release(self):
            pass

    class Service:
        def __init__(self):
            self.lease = Lease()
        def reserve_creation(self, _account):
            return self.lease

    service = Service()
    monkeypatch.setattr(app.state, "codex_contas_login", service, raising=False)
    monkeypatch.setattr(api.tmux, "has_session", lambda _name: False)
    with patch("app.api.registry.create",
               return_value=SessionInfo(name="cx-default", cwd="/tmp", provider="codex")) as cr:
        r = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "cx-default", "cwd": "/tmp", "provider": "codex"})
    assert r.status_code == 200
    assert service.lease.marked == "cx-default"
    cr.assert_called_once()


def test_reserva_codex_em_voo_devolve_409_traduzivel(monkeypatch, tmp_path):
    account = codex_accounts.Account("work", tmp_path / ".codex-work", False)
    monkeypatch.setattr(codex_accounts, "resolve_account", lambda _: account)

    class Service:
        def preparation_status(self, _account):
            return {"status": "ready", "issues": []}

        def reserve_creation(self, _account):
            raise codex_accounts.AccountError(
                409, "codex_account_login_in_progress", {"account_id": "work"}
            )

    service = Service()
    monkeypatch.setattr(app.state, "codex_contas_login", service, raising=False)
    with patch("app.api.registry.create") as cr:
        response = TestClient(app).post("/api/sessions", headers=AUTH, json={
            "name": "cx-reserva", "cwd": "/tmp", "provider": "codex",
            "codex_account": "work",
        })

    assert response.status_code == 409
    assert response.json()["detail"] == {
        "code": "codex_account_login_in_progress",
        "params": {"account_id": "work"},
        "msg": "criação da conta Codex recusada",
    }
    cr.assert_not_called()


def test_rename_atualiza_lease_antes_do_tmux(monkeypatch):
    eventos = []
    monkeypatch.setattr(api.tmux, "has_session", lambda name: name == "old")
    monkeypatch.setattr(api.tmux, "rename_session",
                        lambda old, new: eventos.append("tmux") or True)
    monkeypatch.setattr(api.registry, "rename",
                        lambda old, new: eventos.append("registry"))
    monkeypatch.setattr(api.registry_mod, "apos_renomear_codex",
                        lambda old, new: eventos.append("lease"))
    r = TestClient(app).post("/api/sessions/old/rename", headers=AUTH,
                             json={"new": "new"})
    assert r.status_code == 200
    assert eventos == ["lease", "tmux", "registry"]


@pytest.mark.asyncio
async def test_watcher_descarta_resultado_do_nome_anterior_ao_rename(monkeypatch):
    consulta_antiga = asyncio.Event()
    liberar_consulta = asyncio.Event()
    consultas = []

    class Lease:
        released = False

        def release(self):
            self.released = True

    lease = Lease()
    state = {"name": "old", "lease": lease}
    monkeypatch.setattr(api, "_codex_live_leases", {"old": state})

    async def has_session(_fn, name):
        consultas.append(name)
        if name == "old":
            consulta_antiga.set()
            await liberar_consulta.wait()
            return False
        return True

    monkeypatch.setattr(api.asyncio, "to_thread", has_session)
    task = asyncio.create_task(api._watch_codex_lease(state))
    try:
        await consulta_antiga.wait()
        api._codex_lease_renamed("old", "new")
        api._codex_lease_rename_finished("new")
        liberar_consulta.set()
        await asyncio.sleep(0)
        await asyncio.sleep(0)

        assert consultas == ["old", "new"]
        assert lease.released is False
    finally:
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


@pytest.mark.asyncio
async def test_watcher_nao_libera_nome_novo_enquanto_rename_esta_em_voo(monkeypatch):
    consultas = []

    class Lease:
        released = False

        def release(self):
            self.released = True

    lease = Lease()
    state = {"name": "old", "lease": lease}
    monkeypatch.setattr(api, "_codex_live_leases", {"old": state})

    async def has_session(_fn, name):
        consultas.append(name)
        return False

    monkeypatch.setattr(api.asyncio, "to_thread", has_session)
    api._codex_lease_renamed("old", "new")
    task = asyncio.create_task(api._watch_codex_lease(state))
    try:
        await asyncio.sleep(0)
        assert consultas == []
        assert lease.released is False
    finally:
        api._codex_lease_rename_finished("new")
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


def test_model_options_codex_usa_a_conta_solicitada(monkeypatch, tmp_path):
    account = codex_accounts.Account("work", tmp_path / ".codex-work", False)
    monkeypatch.setattr(codex_accounts, "resolve_account", lambda _: account)
    class Service:
        def preparation_status(self, _account):
            return {"status": "ready", "issues": []}
    monkeypatch.setattr(app.state, "codex_contas_login", Service(), raising=False)
    monkeypatch.setattr(api.codex_models, "listar",
                        lambda **kwargs: [{"id": str(kwargs["codex_home"]), "efforts": []}])
    r = TestClient(app).get("/api/model-options", headers=AUTH,
                             params={"provider": "codex", "codex_account": "work"})
    assert r.status_code == 200
    assert r.json()["models"][0]["id"] == str(account.home)


@pytest.mark.asyncio
async def test_cancelamento_espera_worker_de_criacao_antes_de_liberar_lease(monkeypatch, tmp_path):
    account = codex_accounts.Account("work", tmp_path / ".codex-work", False)
    monkeypatch.setattr(codex_accounts, "resolve_account", lambda _: account)
    finished = threading.Event()
    started = threading.Event()

    class Lease:
        def __init__(self):
            self.marked = False
            self.released = False
        def mark_live(self, name):
            self.marked = name
        def release(self):
            self.released = True

    lease = Lease()

    class Service:
        def preparation_status(self, _account):
            return {"status": "ready", "issues": []}
        def reserve_creation(self, _account):
            return lease
        async def prepare(self, _account):
            return {"status": "ready", "issues": []}

    monkeypatch.setattr(app.state, "codex_contas_login", Service(), raising=False)
    monkeypatch.setattr(api.tmux, "has_session", lambda _name: False)

    def slow_create(*args, **kwargs):
        started.set()
        finished.wait(2)
        return SessionInfo(name="cx-cancel", cwd="/tmp", provider="codex")

    monkeypatch.setattr(api.registry, "create", slow_create)
    task = asyncio.create_task(create_session(CreateBody(
        name="cx-cancel", cwd="/tmp", provider="codex", codex_account="work")))
    await asyncio.to_thread(started.wait, 2)
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done()
    finished.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert lease.marked == "cx-cancel"
