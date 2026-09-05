"""Modo de permissão na criação — borda HTTP."""
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app.config import settings
from app.models import SessionInfo

TOKEN = "t-permissao"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _auth(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)


@pytest.fixture(autouse=True)
def _pane_vivo_e_limpo():
    """O guard de permissão só olha se a sessão vive e se há menu aberto no pane — nunca o tmux
    real da máquina de quem roda a suíte. Teste que quer menu aberto sobrescreve `is_overlay`."""
    with patch("app.tmux.has_session", return_value=True), patch("app.tmux.capture_pane", return_value=""):
        yield


def _client():
    from app.api import app

    return TestClient(app)


def test_create_com_permissao_claude_ok():
    with patch("app.api.registry.create", return_value=SessionInfo(name="x", cwd="/tmp", provider="claude")) as cr:
        r = _client().post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": "claude", "permission_mode": "plan"})
    assert r.status_code == 200
    assert cr.call_args.kwargs.get("permission_mode") == "plan"


def test_create_com_permissao_kimi_409():
    with patch("app.api.registry.create") as cr:
        r = _client().post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": "kimi", "permission_mode": "plan"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_permissao_so_claude"
    cr.assert_not_called()


def test_create_com_permissao_invalida_409():
    with patch("app.api.registry.create") as cr:
        r = _client().post("/api/sessions", headers=AUTH, json={
            "name": "x", "cwd": "/tmp", "provider": "claude", "permission_mode": "invalido"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_permissao_invalida"
    cr.assert_not_called()


def test_create_sem_permissao_recebe_none():
    with patch("app.api.registry.create", return_value=SessionInfo(name="x", cwd="/tmp", provider="claude")) as cr:
        r = _client().post("/api/sessions", headers=AUTH, json={"name": "x", "cwd": "/tmp", "provider": "claude"})
    assert r.status_code == 200
    assert cr.call_args.kwargs.get("permission_mode") is None


# ── Rotas vivas (Task 5) ────────────────────────────────────────────────────

def _info_claude(name="sess", jsonl="/tmp/x.jsonl"):
    return SessionInfo(name=name, cwd="/tmp", provider="claude", jsonl=jsonl)

def _info_kimi(name="sess"):
    return SessionInfo(name=name, cwd="/tmp", provider="kimi", jsonl="/tmp/k.jsonl")

def test_perm_get_ok():
    # sem sondar (default): não chama listar_modos, devolve current de ler_modo e modes []
    info = _info_claude()
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.tmux.has_session", return_value=True), \
         patch("app.tmux.capture_pane", return_value=""), \
         patch("app.state.is_overlay", return_value=False), \
         patch("app.permission_mode.ler_modo", return_value="plan") as mock_ler, \
         patch("app.permission_mode.listar_modos") as mock_listar:
        r = _client().get("/api/sessions/sess/permission-modes", headers=AUTH)
    assert r.status_code == 200
    assert r.json()["current"] == "plan"
    assert r.json()["modes"] == []
    assert r.json()["sondavel"] is True
    assert mock_ler.call_count == 1
    assert mock_listar.call_count == 0
    # com sondar=1: chama listar_modos uma vez
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api.terminal._require_drivable"), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.permission_mode.ler_modo", return_value="plan"), \
         patch("app.permission_mode.listar_modos", return_value=("plan", ["plan", "auto", "manual", "acceptEdits"])) as mk2:
        r2 = _client().get("/api/sessions/sess/permission-modes?sondar=1", headers=AUTH)
    assert r2.status_code == 200
    assert r2.json()["current"] == "plan"
    assert r2.json()["modes"] == ["plan", "auto", "manual", "acceptEdits"]
    assert mk2.call_count == 1

def test_perm_get_sonda_que_nao_voltou_marca_restaurado_false():
    """A sonda dá voltas de BTab de verdade. Quando não consegue voltar, a sessão FICA noutro
    modo por causa de uma chamada de leitura — sem este campo isso saía calado."""
    info = _info_claude(jsonl="/tmp/nao-restaurou.jsonl")
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api.terminal._require_drivable"), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.permission_mode.ler_modo", return_value="plan"), \
         patch("app.permission_mode.listar_modos", return_value=("acceptEdits", ["plan", "acceptEdits"])):
        r = _client().get("/api/sessions/sess/permission-modes?sondar=1", headers=AUTH)
    assert r.status_code == 200
    assert r.json()["current"] == "acceptEdits"
    assert r.json()["restaurado"] is False


def test_perm_get_sonda_que_voltou_marca_restaurado_true():
    info = _info_claude(jsonl="/tmp/restaurou.jsonl")
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api.terminal._require_drivable"), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.permission_mode.ler_modo", return_value="plan"), \
         patch("app.permission_mode.listar_modos", return_value=("plan", ["plan", "auto"])):
        r = _client().get("/api/sessions/sess/permission-modes?sondar=1", headers=AUTH)
    assert r.json()["restaurado"] is True


def test_perm_get_sessao_trabalhando_200_le_o_modo():
    # leitura (sem sondar) com sessão trabalhando: deve devolver 200 e o current lido, sem 409
    info = _info_claude()
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.tmux.has_session", return_value=True), \
         patch("app.tmux.capture_pane", return_value="pane sem overlay mas com spinner"), \
         patch("app.state.is_overlay", return_value=False), \
         patch("app.permission_mode.ler_modo", return_value="plan") as mock_ler, \
         patch("app.permission_mode.listar_modos") as mock_listar:
        r = _client().get("/api/sessions/sess/permission-modes", headers=AUTH)
    assert r.status_code == 200
    assert r.json()["current"] == "plan"
    assert mock_ler.call_count == 1
    assert mock_listar.call_count == 0

def test_perm_sondar_e_trocar_com_sessao_trabalhando_passam():
    # BTab troca o modo no meio do turno (medido): trabalhar não recusa nem sonda nem troca.
    # Só um menu aberto no pane recusa.
    from app.terminal_input import TerminalInput
    from app.api import _perm_modes_cache
    _perm_modes_cache.clear()
    info = _info_claude(name="sess-work", jsonl="/tmp/work.jsonl")
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.api.terminal._require_drivable", side_effect=TerminalInput.NaoDigitou(409, "a sessao esta trabalhando — espere ela terminar")), \
         patch("app.tmux.has_session", return_value=True), \
         patch("app.tmux.capture_pane", return_value="✻ Ebbing… (6s)\n⏵⏵ bypass permissions on"), \
         patch("app.permission_mode.ler_modo", return_value="bypassPermissions"), \
         patch("app.permission_mode.listar_modos", return_value=("bypassPermissions", ["bypassPermissions", "auto", "manual", "acceptEdits", "plan"])), \
         patch("app.permission_mode.trocar_modo", return_value="auto"):
        r = _client().get("/api/sessions/sess-work/permission-modes?sondar=1", headers=AUTH)
        assert r.status_code == 200
        assert len(r.json()["modes"]) == 5
        r2 = _client().post("/api/sessions/sess-work/permission-mode", headers=AUTH, json={"mode": "auto"})
    assert r2.status_code == 200
    assert r2.json()["mode"] == "auto"


def test_perm_menu_aberto_no_pane_409():
    info = _info_claude(name="sess-menu", jsonl="/tmp/menu.jsonl")
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.tmux.has_session", return_value=True), \
         patch("app.tmux.capture_pane", return_value="x"), \
         patch("app.state.is_overlay", return_value=True):
        r = _client().post("/api/sessions/sess-menu/permission-mode", headers=AUTH, json={"mode": "auto"})
    assert r.status_code == 409

def test_perm_get_nao_claude_409():
    info = _info_kimi()
    with patch("app.api._cached_info", return_value=info):
        r = _client().get("/api/sessions/sess/permission-modes", headers=AUTH)
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_permissao_so_claude"

def test_perm_get_cache_usa_mesma_lista():
    from app.api import _perm_modes_cache
    _perm_modes_cache.clear()
    info = _info_claude(name="sess-cache", jsonl="/tmp/cache.jsonl")
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api.terminal._require_drivable"), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.permission_mode.ler_modo", return_value="plan"), \
         patch("app.permission_mode.listar_modos", return_value=("plan", ["plan", "auto", "manual", "acceptEdits"])) as mk:
        r1 = _client().get("/api/sessions/sess-cache/permission-modes?sondar=1", headers=AUTH)
        assert r1.status_code == 200
        r2 = _client().get("/api/sessions/sess-cache/permission-modes?sondar=1", headers=AUTH)
        assert r2.status_code == 200
        # segunda chamada usou cache: listar_modos só uma vez
        assert mk.call_count == 1

def test_perm_post_ok():
    info = _info_claude()
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api.terminal._require_drivable"), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.permission_mode.trocar_modo", return_value="auto"):
        r = _client().post("/api/sessions/sess/permission-mode", headers=AUTH, json={"mode": "auto"})
    assert r.status_code == 200
    assert r.json()["mode"] == "auto"

def test_perm_post_aceita_permission_mode_alias():
    info = _info_claude()
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api.terminal._require_drivable"), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.permission_mode.trocar_modo", return_value="plan"):
        r = _client().post("/api/sessions/sess/permission-mode", headers=AUTH, json={"permission_mode": "plan"})
    assert r.status_code == 200
    assert r.json()["mode"] == "plan"

def test_perm_post_teto_409_devolve_ficou():
    info = _info_claude()
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api.terminal._require_drivable"), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.permission_mode.trocar_modo", return_value="plan"):
        # pede dontAsk mas ficou em plan (fora do ciclo)
        r = _client().post("/api/sessions/sess/permission-mode", headers=AUTH, json={"mode": "dontAsk"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_permissao_teto"
    assert r.json()["detail"]["params"]["ficou"] == "plan"
    assert r.json()["detail"]["params"]["mode"] == "plan"

def test_perm_post_invalida_409():
    info = _info_claude()
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api.terminal._require_drivable"), \
         patch("app.api._recusa_se_painel_aberto"):
        r = _client().post("/api/sessions/sess/permission-mode", headers=AUTH, json={"mode": "invalido"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_permissao_invalida"

def test_perm_post_nao_claude_409():
    info = _info_kimi()
    with patch("app.api._cached_info", return_value=info):
        r = _client().post("/api/sessions/sess/permission-mode", headers=AUTH, json={"mode": "plan"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_permissao_so_claude"

def test_perm_post_painel_aberto_409():
    info = _info_claude()
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api._recusa_se_painel_aberto", side_effect=__import__("fastapi").HTTPException(status_code=409, detail={"code": "erro_terminal_aberto", "params": {}, "msg": "aberto"})):
        r = _client().post("/api/sessions/sess/permission-mode", headers=AUTH, json={"mode": "plan"})
    assert r.status_code == 409

def test_perm_post_sessao_trabalhando_409():
    from app.terminal_input import TerminalInput
    info = _info_claude()
    with patch("app.api._cached_info", return_value=info), \
         patch("app.api._recusa_se_painel_aberto"), \
         patch("app.api.terminal._require_drivable", side_effect=TerminalInput.NaoDigitou(409, "trabalhando")):
        r = _client().post("/api/sessions/sess/permission-mode", headers=AUTH, json={"mode": "plan"})
    assert r.status_code == 409

