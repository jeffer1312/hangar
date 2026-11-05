"""Contratos dos modelos Claude/Codex que ainda têm consumidores Python."""
import json
import subprocess
import time
from pathlib import Path
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient


@pytest.fixture(autouse=True)
def _models_cache_isolado(models_cache_em_tmp):
    """Liga o redirecionamento do cache de modelos em disco (ver tests/conftest.py)."""
    yield


from app import api
from app.api import app
from app.models import SessionInfo
from app.config import settings

TOKEN = "t-model-options"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.mark.parametrize("session", [False, True])
def test_claude_parser_failure_is_an_explicit_error_without_alias_fallback(cli, monkeypatch, session):
    async def info(name):
        return SessionInfo(name=name, cwd="/tmp", provider="claude")

    async def list_models(name):
        return [{"value": "modelo-real"}]

    def unavailable(*args):
        raise api.claude_models.ClaudeIndisponivel("Rust indisponível")

    monkeypatch.setattr(api, "_cached_info", info)
    monkeypatch.setattr(api, "_headless", lambda name: True)
    monkeypatch.setattr(api, "get_adapter", lambda provider: SimpleNamespace(list_models=list_models, escolhas=lambda name: (None, None)))
    monkeypatch.setattr(api.headless_sessions, "load", lambda name: {})
    monkeypatch.setattr(api.claude_models, "listar", lambda *args: [{"value": "modelo-real"}])
    monkeypatch.setattr(api.claude_models, "para_tela", unavailable)
    route = "/api/sessions/claude/model/options" if session else "/api/model-options?provider=claude"
    response = cli.get(route, headers=AUTH)
    assert response.status_code == 503
    assert "Rust indisponível" in str(response.json())
    assert "models" not in response.json()


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    # Cache vaza de teste pra teste (o TTL é de minutos): limpar antes e depois de cada um, senão
    # a suíte fica dependente de ordem.
    api._claude_models_cache.clear()
    # Sem isto a rota subiria um `claude` de verdade em todo teste de cache frio: o catálogo
    # efêmero é subprocess. O padrão da suíte é "não veio", que é o ramo do fallback.
    monkeypatch.setattr(api.claude_models, "listar",
                        lambda *a, **k: (_ for _ in ()).throw(
                            api.claude_models.ClaudeAusente("sem claude no teste")))
    yield
    api._claude_models_cache.clear()


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
    # A grafia com `..` sai da PROPRIA casa: com o caminho do Linux escrito na mao ela nunca
    # colapsaria noutro sistema (a casa la e `C:\Users\<voce>`), e o caso passaria a medir o
    # `Path.home()` em vez do colapso de grafias, que e o que ele existe pra provar.
    volta = str(Path.home() / ".claude" / ".." / ".claude")
    for grafia in ("", None, "  ", "~", "~/.claude", casa, casa + "/", volta):
        assert api._chave_config(grafia) == casa, grafia


def test_chave_config_nao_mistura_contas():
    assert api._chave_config("~/.claude-jefferson") != api._chave_config("~")


def test_claude_sem_cache_diz_que_a_lista_e_reduzida(cli):
    r = cli.get("/api/model-options", headers=AUTH, params={"provider": "claude"})
    assert r.status_code == 200
    assert r.json()["reduced"] is True
    assert [m["id"] for m in r.json()["models"]] == ["opus", "fable", "sonnet", "haiku"]


def test_cache_frio_le_o_catalogo_efemero_e_cacheia(cli, monkeypatch):
    """O ponto do `claude_models`: cache vencido não cai mais na lista reduzida enquanto o binário
    responder — e a leitura seguinte já sai do cache, sem subir outro processo."""
    chamadas = []

    def falso(config_dir=None, *a, **k):
        chamadas.append(config_dir)
        return [{"value": "default", "displayName": "Default", "description": "d",
                 "resolvedModel": "claude-opus-5[1m]"},
                {"value": "fable", "displayName": "Fable", "description": "f"}]

    monkeypatch.setattr(api.claude_models, "listar", falso)
    monkeypatch.setattr(api.claude_models, "para_tela", lambda *a, **k: [
        {"id":"default","name":"Default","desc":"d","active":True},
        {"id":"fable","name":"Fable","desc":"f","active":False},
    ])
    r = cli.get("/api/model-options", headers=AUTH, params={"provider": "claude"})
    assert r.status_code == 200
    assert r.json()["reduced"] is False
    assert [m["id"] for m in r.json()["models"]] == ["default", "fable"]
    assert r.json()["models"][0]["active"] is True
    cli.get("/api/model-options", headers=AUTH, params={"provider": "claude"})
    assert len(chamadas) == 1


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


def test_leitura_cortada_do_picker_nao_vira_cache(tmp_path):
    cortada = {"kind": "claude", "engine": None, "effort": None,
               "models": [{"id": "opus", "name": "Opus (1M con…", "desc": "Opus 5", "active": False}]}
    chave = str(tmp_path)
    api._models_cache_put(chave, cortada)
    assert chave not in api._claude_models_cache
    assert not (tmp_path / ".hangar-models.json").exists()
    # A que já foi gravada antes da trava deixa de valer em vez de mandar por 30 dias.
    (tmp_path / ".hangar-models.json").write_text(json.dumps({"ts": time.time(), "resp": cortada}), encoding="utf-8")
    assert api._models_cache_get(chave) is None


def test_provider_fora_de_escopo_vira_400(cli):
    # Os quatro providers ENTRARAM no escopo; o que sobra aqui é o pedido sem provider nenhum.
    r = cli.get("/api/model-options", headers=AUTH, params={"provider": ""})
    assert r.status_code == 400
    assert "claude" in r.json()["detail"]["msg"]


_CODEX_CAT = [{"id": "gpt-5.6-sol", "name": "GPT-5.6-Sol", "desc": "Latest frontier model.",
               "efforts": ["low", "medium", "high", "xhigh", "max", "ultra"],
               "default_effort": "low"},
              {"id": "gpt-5.5", "name": "GPT-5.5", "desc": "",
               "efforts": ["low", "medium", "high", "xhigh"],
               "default_effort": "medium"}]


def test_codex_serve_o_catalogo_do_app_server(cli, monkeypatch):
    monkeypatch.setattr(api.codex_models, "listar", lambda **kw: _CODEX_CAT)
    r = cli.get("/api/model-options", headers=AUTH, params={"provider": "codex"})
    assert r.status_code == 200
    assert r.json()["kind"] == "codex"
    assert r.json()["reduced"] is False
    # Os níveis vêm POR MODELO: é a razão de o catálogo existir em vez de uma lista no código.
    assert r.json()["models"][0]["efforts"][-1] == "ultra"
    assert "ultra" not in r.json()["models"][1]["efforts"]


def test_codex_ausente_tem_codigo_proprio(cli, monkeypatch):
    def some(*a, **k):
        raise api.codex_models.CodexAusente("nao achei o executavel `codex`")
    monkeypatch.setattr(api.codex_models, "listar", some)
    r = cli.get("/api/model-options", headers=AUTH, params={"provider": "codex"})
    assert r.status_code == 502
    assert r.json()["detail"]["code"] == "erro_codex_ausente"


def test_codex_que_falha_vira_502(cli, monkeypatch):
    # Sem `TimeoutExpired` na lista, ao contrário do Pi: o teto de tempo do `codex_models` mata o
    # processo por um Timer, então estouro de tempo chega como RuntimeError. Injetar a outra aqui
    # provaria a captura de algo que o código real não produz.
    for err in (RuntimeError("model/list nao respondeu"), OSError("codex: no such file")):
        def quebra(*a, **k):
            raise err
        monkeypatch.setattr(api.codex_models, "listar", quebra)
        r = cli.get("/api/model-options", headers=AUTH, params={"provider": "codex"})
        assert r.status_code == 502, err


def test_sem_token_vira_401(cli):
    assert cli.get("/api/model-options").status_code == 401
