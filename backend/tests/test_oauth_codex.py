"""Login OAuth do ChatGPT feito pelo app e espalhado pros CLIs (app/oauth_codex.py).

O que trava: o fluxo de dispositivo termina com o cofre gravado (0600) e os três stores escritos
no formato de cada um; store que já tem login é mantido; CLI ausente é `nao-instalado`, não erro.
"""
import base64
import json
import sqlite3
import time

import pytest

from app import oauth_codex as o


def _jwt(account="acc-1", plano="plus", exp=4102444800):
    corpo = base64.urlsafe_b64encode(json.dumps({
        "exp": exp, "https://api.openai.com/auth": {"chatgpt_account_id": account, "chatgpt_plan_type": plano},
    }).encode()).rstrip(b"=").decode()
    return f"h.{corpo}.s"


@pytest.fixture
def casa(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("PI_CODING_AGENT_DIR", raising=False)
    monkeypatch.setattr(o, "cofre", lambda: tmp_path / ".hangar" / "auth" / "openai-codex.json")
    (tmp_path / ".codex").mkdir()
    (tmp_path / ".pi" / "agent").mkdir(parents=True)
    (tmp_path / ".omp" / "agent").mkdir(parents=True)
    con = sqlite3.connect(tmp_path / ".omp" / "agent" / "agent.db")
    con.execute("create table auth_credentials (id integer primary key autoincrement, provider text not null, "
                "credential_type text not null, data text not null, identity_key text)")
    con.commit(); con.close()
    o._tentativa = None
    return tmp_path


def test_fluxo_de_dispositivo_grava_cofre_e_os_tres_stores(casa, monkeypatch):
    respostas = iter([
        (200, {"device_auth_id": "d1", "user_code": "ABCD-1234", "interval": "0"}),
        (403, {}),
        (200, {"authorization_code": "c", "code_verifier": "v"}),
        (200, {"access_token": _jwt(), "refresh_token": "r1", "id_token": "i1", "expires_in": 10}),
    ])
    monkeypatch.setattr(o, "_http", lambda url, corpo, form: next(respostas))
    passo = o.iniciar(casa)
    assert passo["etapa"] == "aguardando" and passo["user_code"] == "ABCD-1234"
    assert passo["url"] == o.VERIFICATION_URL
    for _ in range(100):
        if o.passo()["etapa"] != "aguardando":
            break
        time.sleep(0.05)
    p = o.passo()
    assert p["etapa"] == "concluido", p
    assert oct(o.cofre().stat().st_mode & 0o777) == "0o600"
    assert {k: v["ok"] for k, v in p["resultado"].items()} == {"codex": True, "pi": True, "omp": True}
    codex = json.loads((casa / ".codex" / "auth.json").read_text())
    assert codex["auth_mode"] == "chatgpt" and codex["tokens"]["account_id"] == "acc-1"
    pi = json.loads((casa / ".pi" / "agent" / "auth.json").read_text())["openai-codex"]
    assert pi == {"type": "oauth", "access": _jwt(), "refresh": "r1", "expires": 4102444800000, "accountId": "acc-1"}
    con = sqlite3.connect(casa / ".omp" / "agent" / "agent.db")
    prov, tipo, dados, ident = con.execute("select provider, credential_type, data, identity_key from auth_credentials").fetchone()
    assert (prov, tipo, ident) == ("openai-codex", "oauth", "acc-1")
    assert json.loads(dados)["refresh"] == "r1" and "type" not in json.loads(dados)
    assert o.estado(casa) == {"cofre": True, "plano": "plus", "expira_em": 4102444800000,
                              "codex": True, "pi": True, "omp": True}


def test_store_com_login_e_mantido_e_cli_ausente_nao_e_erro(casa):
    (casa / ".codex" / "auth.json").write_text(json.dumps({"tokens": {"refresh_token": "dele"}}))
    (casa / ".pi" / "agent" / "auth.json").write_text(json.dumps({"openai-codex": {"type": "oauth", "refresh": "dele"}}))
    (casa / ".omp" / "agent" / "agent.db").unlink()
    t = o.Tokens(access=_jwt(), refresh="novo", id_token="", expires_ms=1, account_id="acc-1")
    r = o.propagar(t, casa)
    assert r["codex"] == {"ok": True, "motivo": "ja-logado"}
    assert r["pi"] == {"ok": True, "motivo": "ja-logado"}
    assert r["omp"] == {"ok": False, "motivo": "nao-instalado"}
    assert json.loads((casa / ".codex" / "auth.json").read_text())["tokens"]["refresh_token"] == "dele"


def test_importar_do_codex_alimenta_o_cofre(casa):
    (casa / ".codex" / "auth.json").write_text(json.dumps({
        "tokens": {"access_token": _jwt("acc-9", "pro"), "refresh_token": "r9", "account_id": "acc-9"}}))
    t = o.importar_do_codex(casa)
    assert t and t.account_id == "acc-9" and t.plano == "pro"
    assert o.ler_cofre().refresh == "r9"
