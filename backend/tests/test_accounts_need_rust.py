"""Contas e cotas sem o servidor Rust: não há reserva Python.

No modo python (sem binário, CP_RUST_SERVER=0, supervisor que desistiu), as rotas de conta e cota
que o Rust atende respondem 503 `accounts_need_rust_server`, e os consumidores internos tratam a
ponte ausente como "sem cota, sem login conhecido", sem ler credencial nem abrir CLI pelo Python.
As rotas que só existem no Python (lista de credenciais, apelido, cookie) continuam.
"""
import asyncio
import json
import os
import stat
import time
import urllib.request
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app import codex_contas, conta_estado, contas, cotas, runtime_coordinator
from app.api import app
from app.codex_contas_login import CodexContasLogin
from app.config import list_config_dirs, settings

TOKEN = "t-contas-sem-rust"
AUTH = {"Authorization": f"Bearer {TOKEN}"}

RUST_OWNED = [
    ("GET", "/api/cotas"),
    ("GET", "/api/cotas/sugestao"),
    ("GET", "/api/conta-estado"),
    ("POST", "/api/conta-estado/conta2/login"),
    ("POST", "/api/conta-estado/conta2/login/codigo"),
    ("GET", "/api/conta-estado/conta2/login/passo"),
    ("POST", "/api/conta-estado/conta2/login/cancelar"),
    ("GET", "/api/claude-configs"),
    ("POST", "/api/claude-configs"),
    ("DELETE", "/api/claude-configs/conta2"),
    ("POST", "/api/claude-configs/conta2/logout"),
    ("GET", "/api/codex-contas"),
    ("POST", "/api/codex-contas"),
    ("DELETE", "/api/codex-contas/extra"),
    ("GET", "/api/codex-contas/extra/prepare"),
    ("POST", "/api/codex-contas/extra/prepare"),
    ("GET", "/api/codex-contas/extra/login"),
    ("POST", "/api/codex-contas/extra/login"),
    ("DELETE", "/api/codex-contas/extra/login?attempt_id=a"),
    ("POST", "/api/codex-contas/extra/logout"),
    ("POST", "/api/codex-contas/extra/rate-limit-reset"),
    ("GET", "/api/credenciais/codex"),
    ("GET", "/api/credenciais/codex/login"),
    ("POST", "/api/credenciais/codex/login"),
    ("DELETE", "/api/credenciais/codex/login"),
]


def _fake_cli(folder: Path, name: str, marker: Path) -> None:
    script = folder / name
    script.write_text(f"#!/bin/sh\necho \"$0 $*\" >> '{marker}'\nexit 1\n", encoding="utf-8")
    script.chmod(script.stat().st_mode | stat.S_IEXEC)


@pytest.fixture
def python_owner(tmp_path, monkeypatch):
    """Modo python com uma conta Claude logada e uma Codex: qualquer leitura Python deixa rastro."""
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    monkeypatch.setattr(runtime_coordinator, "current", lambda: None)
    monkeypatch.setattr(contas, "compartilhado", lambda: tmp_path)
    claude = tmp_path / ".claude"
    claude.mkdir()
    future = int((time.time() + 3600) * 1000)
    (claude / ".credentials.json").write_text(json.dumps({"claudeAiOauth": {
        "accessToken": "sk-ant-oat-sintetico", "refreshToken": "r", "expiresAt": future,
        "refreshTokenExpiresAt": future}}), encoding="utf-8")
    (tmp_path / ".codex").mkdir()
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", tmp_path / ".codex")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    marker = tmp_path / "cli-chamado.log"
    _fake_cli(bin_dir, "claude", marker)
    _fake_cli(bin_dir, "codex", marker)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}")
    network = []

    def no_network(request, *args, **kwargs):
        network.append(getattr(request, "full_url", request))
        raise OSError("rede proibida no teste")

    monkeypatch.setattr(urllib.request, "urlopen", no_network)
    yield {"marker": marker, "network": network}
    assert network == [], "o Python leu cota pela rede"
    assert not marker.exists(), f"o Python chamou o CLI: {marker.read_text()}"


@pytest.mark.parametrize("method,path", RUST_OWNED)
def test_account_routes_answer_unavailable_without_rust(python_owner, method, path):
    body = {"name": "extra", "nome": "conta2", "codigo": "x",
            "idempotency_key": "00000000-0000-4000-8000-000000000000"}
    response = TestClient(app).request(method, path, headers=AUTH,
                                       json=body if method in {"POST", "PUT"} else None)
    assert response.status_code == 503
    detail = response.json()["detail"]
    assert detail["code"] == "accounts_need_rust_server"
    assert detail["msg"] == "contas e cotas precisam do servidor Rust"


def test_unauthenticated_account_route_is_refused_before_unavailable(python_owner):
    response = TestClient(app).get("/api/cotas")
    assert response.status_code == 401


def test_python_only_credentials_list_keeps_working_without_reading(python_owner):
    response = TestClient(app).get("/api/credenciais", headers=AUTH)
    assert response.status_code == 200
    [claude] = [row for row in response.json() if row["tipo"] == "claude"]
    assert claude["login"]["estado"] == "indisponivel"
    assert claude["login"]["loggedIn"] is None
    assert claude["cota"] is None


def test_python_only_alias_and_cookie_keep_working(python_owner):
    client = TestClient(app)
    alias = client.put("/api/credenciais/apelido", headers=AUTH,
                       json={"id": "chave:opencode", "apelido": "Meu"})
    assert (alias.status_code, alias.json()) == (200, {"id": "chave:opencode", "apelido": "Meu"})
    cookie = client.put("/api/credenciais/cookie", headers=AUTH,
                        json={"id": "chave:opencode", "workspace_id": "w", "auth_cookie": "c"})
    assert (cookie.status_code, cookie.json()) == (200, {"id": "chave:opencode", "cookie_definido": True})


def test_internal_quota_consumers_get_nothing_without_rust(python_owner):
    assert cotas.listar_cotas() == []
    assert cotas.cotas_claude() == []
    assert cotas.cotas_claude(atualizar=True) == []


def test_internal_login_consumers_get_unavailable_without_rust(python_owner):
    [login] = conta_estado.logins(list_config_dirs())
    assert (login.estado, login.loggedIn) == ("indisponivel", None)


def test_codex_auth_is_unavailable_without_rust(python_owner):
    service = CodexContasLogin()
    account = codex_contas.resolve_account("default")
    for read in (service.read_auth(account), service.read_auth(account, refresh=True),
                 service.read_auth_rapido(account)):
        assert asyncio.run(read)["status"] == "unavailable"
