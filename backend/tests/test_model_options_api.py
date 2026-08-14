"""Borda HTTP da lista de modelos sem sessão viva (`GET /api/model-options`).

O que esta suíte trava: a chave compartilhada do cache de modelos da conta (_chave_config cola a
grafia da sessão viva — derivada do /proc — com a grafia que o cliente manda — caminho livre); os
cinco ramos da rota; e a invalidação do cache do motor quando o motor é editado ou apagado. Sem
isto, apagar o `.resolve()` do _chave_config deixava a suíte inteira verde e matava o cache
compartilhado calado — `reduced: true` é resposta legítima, e a tela de abertura responderia "lista
reduzida" pra sempre na conta padrão sem ninguém perceber.
"""
import subprocess
import time
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app import api
from app import engines as eng
from app.api import app
from app.config import settings

TOKEN = "t-model-options"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    # Convenção da casa (ver tests/test_engines_api.py:18-33): token próprio por arquivo + fixture
    # de engines.caminho em tmp_path pro teste do PUT/DELETE.
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setattr(eng, "caminho", lambda: tmp_path / "engines.json")
    # Cache vaza de teste pra teste (o TTL é de minutos): limpar antes e depois de cada um, senão
    # a suíte fica dependente de ordem.
    api._claude_models_cache.clear()
    api._engine_models_cache.clear()
    yield
    api._claude_models_cache.clear()
    api._engine_models_cache.clear()


@pytest.fixture
def cli():
    return TestClient(app)


def _resposta_falsa() -> dict:
    """O que a rota da sessão viva grava no cache: kind claude, com o que o picker leu."""
    return {"kind": "claude", "engine": None, "effort": "high",
            "models": [{"id": "opus", "name": "Opus 5", "desc": "", "active": False}]}


def _semeia_cache_claude() -> None:
    api._claude_models_cache[api._chave_config(None)] = (time.monotonic(), _resposta_falsa())


def test_chave_config_colapsa_as_grafias_da_conta_padrao():
    casa = str(Path.home() / ".claude")
    for grafia in ("", None, "  ", "~", "~/.claude", casa, casa + "/", "/home/jefferson/.claude/../.claude"):
        assert api._chave_config(grafia) == casa, grafia


def test_chave_config_nao_mistura_contas():
    assert api._chave_config("~/.claude-jefferson") != api._chave_config("~")


def test_claude_sem_cache_diz_que_a_lista_e_reduzida(cli):
    r = cli.get("/api/model-options", headers=AUTH, params={"provider": "claude"})
    assert r.status_code == 200
    assert r.json()["reduced"] is True
    assert [m["id"] for m in r.json()["models"]] == ["opus", "sonnet", "haiku"]


def test_a_abertura_aproveita_o_cache_da_sessao_viva_em_qualquer_grafia(cli):
    """O ponto inteiro da chave compartilhada: a sessão viva grava com a grafia do /proc (None =
    conta padrão) e a abertura pergunta com a grafia do cliente — têm que casar."""
    _semeia_cache_claude()
    for config_dir in ("", "~", "~/.claude", str(Path.home() / ".claude")):
        r = cli.get("/api/model-options", headers=AUTH,
                    params={"provider": "claude", "config_dir": config_dir})
        assert r.status_code == 200
        assert r.json()["reduced"] is False, config_dir
        assert r.json()["models"] == _resposta_falsa()["models"], config_dir


def test_cache_de_outra_conta_nao_vaza(cli):
    _semeia_cache_claude()
    r = cli.get("/api/model-options", headers=AUTH,
                params={"provider": "claude", "config_dir": "~/.claude-outra"})
    assert r.status_code == 200
    assert r.json()["reduced"] is True


def test_pi_vem_do_catalogo(cli, monkeypatch):
    def fake_listar(fresco=False):
        return [{"provider": "kimi-coding", "id": "k3", "context": "1.0M",
                 "max_out": "131.1K", "thinking": True, "images": True}]
    monkeypatch.setattr(api.pi_catalog, "listar", fake_listar)
    r = cli.get("/api/model-options", headers=AUTH, params={"provider": "pi"})
    assert r.status_code == 200
    assert r.json()["kind"] == "pi"
    assert r.json()["reduced"] is False
    assert r.json()["models"][0]["id"] == "k3"


def test_pi_que_falha_vira_502(cli, monkeypatch):
    for err in (RuntimeError("pi --list-models nao devolveu modelo nenhum"),
                OSError("pi: no such file"),
                subprocess.TimeoutExpired("pi", 30)):
        def quebra(*a, **k):
            raise err
        monkeypatch.setattr(api.pi_catalog, "listar", quebra)
        r = cli.get("/api/model-options", headers=AUTH, params={"provider": "pi"})
        assert r.status_code == 502, err


def test_provider_fora_de_escopo_vira_400(cli):
    for provider in ("codex", "kimi", ""):
        r = cli.get("/api/model-options", headers=AUTH, params={"provider": provider})
        assert r.status_code == 400, provider
        assert "claude" in r.json()["detail"]["msg"]


def test_sem_token_vira_401(cli):
    assert cli.get("/api/model-options").status_code == 401


def test_motor_serve_o_catalogo_do_provedor(cli):
    """Sem rede: o cache curto-circuita o engine_probe."""
    api._engine_models_cache["kimi"] = (time.monotonic(),
                                        [{"id": "k3", "context_length": 262144, "vision": True}])
    r = cli.get("/api/model-options", headers=AUTH,
                params={"provider": "claude", "engine": "kimi"})
    assert r.status_code == 200
    assert r.json()["kind"] == "engine"
    assert r.json()["reduced"] is False
    assert r.json()["models"] == [{"id": "k3", "context_length": 262144, "vision": True}]


def test_put_e_delete_de_motor_esvaziam_o_cache_do_catalogo(cli):
    """A chave do cache é o NOME do motor: trocar base_url/api_key mantendo o nome serviria a lista
    do provedor antigo por até 5 minutos sem a invalidação."""
    motor = {"label": "Kimi", "base_url": "https://api.kimi.com/coding",
             "api_key": "sk-kimi-abcdefgh1234", "model": "k3"}
    api._engine_models_cache["kimi"] = (time.monotonic(), [{"id": "k3"}])
    assert cli.put("/api/engines/kimi", json=motor, headers=AUTH).status_code == 200
    assert "kimi" not in api._engine_models_cache
    api._engine_models_cache["kimi"] = (time.monotonic(), [{"id": "k3"}])
    assert cli.delete("/api/engines/kimi", headers=AUTH).status_code == 200
    assert "kimi" not in api._engine_models_cache
