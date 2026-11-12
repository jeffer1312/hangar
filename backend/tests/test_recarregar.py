"""Recarregar uma sessão Claude sem terminal: motivo no `state`, reciclagem do processo e rota."""
import asyncio
import json
import time
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from fastapi.testclient import TestClient

from app.adapters.claude_headless import adapter as A
from app.adapters.claude_headless import sessions as S
from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter, _Sessao
from app.config import settings


@pytest.fixture
def conta(tmp_path, monkeypatch):
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    raiz = tmp_path / "conta"
    raiz.mkdir()
    (raiz / ".claude.json").write_text("{}")
    return raiz


def _sessao(conta, marca: str | None):
    meta = {"name": "s1", "cwd": "/tmp", "session_id": "x", "config_dir": str(conta),
            "cano": {"pid": 4242, "ts": time.time(), "config_marca": marca} if marca else None}
    return _Sessao("s1", meta)


def test_motivo_config_so_quando_mcp_ou_settings_mudam(conta):
    ad = ClaudeHeadlessAdapter()
    (conta / ".claude.json").write_text(json.dumps({"mcpServers": {"a": {"url": "x"}}, "numStartups": 1}))
    marca = A._marca_config(str(conta))

    async def motivo(sess):
        # A leitura roda em segundo plano: a primeira chamada devolve o último valor conhecido e
        # dispara a renovação; o valor de verdade vem na chamada seguinte.
        ad.motivo_recarga(sess)
        await asyncio.gather(*ad._tarefas)
        return ad.motivo_recarga(sess)

    async def fluxo():
        assert await motivo(_sessao(conta, marca)) is None
        # O próprio Claude Code reescreve o resto do .claude.json a toda hora: isso não é motivo.
        (conta / ".claude.json").write_text(json.dumps({"mcpServers": {"a": {"url": "x"}}, "numStartups": 2}))
        assert await motivo(_sessao(conta, marca)) is None
        (conta / ".claude.json").write_text(json.dumps({"mcpServers": {"a": {"url": "y"}}, "numStartups": 2}))
        assert await motivo(_sessao(conta, marca)) == "config"
        (conta / ".claude.json").write_text(json.dumps({"mcpServers": {"a": {"url": "x"}}}))
        (conta / "settings.json").write_text('{"hooks": {}}')
        assert await motivo(_sessao(conta, marca)) == "config"
        assert await motivo(_sessao(conta, None)) is None   # processo de antes da marca
        # Config ilegível: "não li" não vira "mudou" — fica o último valor e o aviso vai pro log.
        (conta / "settings.json").write_bytes(b"\xff\xfe{")
        sess = _sessao(conta, marca)
        assert await motivo(sess) is None
    asyncio.run(fluxo())


def test_marca_ilegivel_levanta_e_ausente_conta_como_vazio(conta):
    (conta / ".claude.json").unlink()
    assert A._marca_config(str(conta)) == A._marca_config(str(conta))
    (conta / ".claude.json").write_text("{nao é json")
    with pytest.raises(ValueError):
        A._marca_config(str(conta))


def test_recarregar_encerra_e_acorda_na_mesma_sessao(conta):
    ad = ClaudeHeadlessAdapter()
    sess = _sessao(conta, time.time())
    sess.proc = MagicMock(returncode=None)
    ad._sessions["s1"] = sess
    ad._encerrar = AsyncMock()
    ad.acordar = MagicMock()
    with patch.object(A, "_esquecer_cano") as esq:
        asyncio.run(ad.recarregar("s1"))
    ad._encerrar.assert_awaited_once_with(sess)
    esq.assert_called_once_with("s1", 4242)
    ad.acordar.assert_called_once_with("s1")


def test_recarregar_sessao_parada_so_acorda(conta):
    ad = ClaudeHeadlessAdapter()
    ad._encerrar = AsyncMock()
    ad.acordar = MagicMock()
    asyncio.run(ad.recarregar("s1"))
    ad._encerrar.assert_not_awaited()
    ad.acordar.assert_called_once_with("s1")


@pytest.fixture
def cliente(monkeypatch):
    import app.api as api_mod
    settings.auth_token = "secret"
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    return TestClient(api_mod.app)


_H = {"Authorization": "Bearer secret"}


@pytest.mark.parametrize("provider", ["claude", "codex", "pi", None])
def test_reload_route_is_rust_only(cliente, provider):
    from app.models import SessionInfo
    info = SessionInfo(name="s1", cwd="/tmp", provider=provider) if provider else None
    with patch("app.api._cached_info", AsyncMock(return_value=info)):
        r = cliente.post("/api/sessions/s1/recarregar", headers=_H)
    code = r.json()["detail"]["code"]
    if provider is None:
        assert r.status_code == 404 and code == "erro_sessao_inexistente"
    elif provider == "pi":
        assert r.status_code == 409 and code == "erro_recarregar_so_sem_terminal"
    else:
        assert r.status_code == 503 and code == "session_open_needs_rust_server"
