"""Lista de máquinas do app dentro do peers.json: uma máquina, uma entrada, e os leitores de recado
continuam vendo só peer."""
import json
import subprocess
import sys
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app import peers
from app.api import app
from app.config import settings

TOKEN = "t-app-list"
AUTH = {"Authorization": f"Bearer {TOKEN}"}
ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture(autouse=True)
def _isola(monkeypatch, tmp_path):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setattr(peers, "_PEERS_FILE", tmp_path / "peers.json")


@pytest.fixture
def cli():
    return TestClient(app)


def maquina(id, address, **extra):
    return {"id": id, "label": id.upper(), "address": address, "token": f"tok-{id}", "disabled": False, "invite": False, **extra}


def gravar(cli, servers, rev=None):
    rev = cli.get("/api/peers/app-list", headers=AUTH).json()["rev"] if rev is None else rev
    return cli.put("/api/peers/app-list", headers=AUTH, json={"rev": rev, "servers": servers})


def test_lista_vazia_e_so_para_o_dono(cli):
    assert cli.get("/api/peers/app-list").status_code == 401
    r = cli.get("/api/peers/app-list", headers=AUTH)
    assert r.status_code == 200 and r.json()["servers"] == []


def test_grava_na_ordem_do_app_e_casa_com_o_peer_de_mesmo_host(cli):
    peers._PEERS_FILE.write_text(json.dumps({
        "delphi-02": {"base_url": "https://delphi-02.ts.net", "token": "peer"},
        "casa": {"base_url": "https://vps/casa", "token": "p2", "web_url": "https://casa.ts.net"},
    }))
    lista = [
        maquina("srv-local", "http://127.0.0.1:8765/"),
        maquina("srv-casa", "https://casa.ts.net", lan={"url": "http://192.168.0.2:8765", "id": "casa"}),
        maquina("srv-delphi", "https://delphi-02.ts.net", disabled=True),
        maquina("srv-conv", "https://delphi-02.ts.net:8443", invite=True),
    ]
    r = gravar(cli, lista)
    assert r.status_code == 200
    assert r.json()["servers"] == lista
    dados = json.loads(peers._PEERS_FILE.read_text())
    # O peer ganha a parte do app e o que era dele fica igual.
    assert dados["delphi-02"]["token"] == "peer" and dados["delphi-02"]["app"]["id"] == "srv-delphi"
    assert dados["casa"]["web_url"] == "https://casa.ts.net" and dados["casa"]["app"]["id"] == "srv-casa"
    # Convite e endereço local nunca viram peer de recado.
    assert dados["srv-conv"] == {"app": {**lista[3], "pos": 3}}
    assert "base_url" not in dados["srv-local"]
    assert cli.get("/api/peers/app-list", headers=AUTH).json()["servers"] == lista


def test_regravar_e_idempotente_e_revisao_velha_e_409(cli):
    lista = [maquina("srv-a", "https://a.ts.net")]
    primeira = gravar(cli, lista).json()
    segunda = gravar(cli, lista).json()
    assert primeira == segunda
    assert gravar(cli, [], rev="velha").status_code == 409
    assert cli.get("/api/peers/app-list", headers=AUTH).json()["servers"] == lista


def test_tirar_do_app_mantem_o_peer_e_apaga_so_a_entrada_do_app(cli):
    peers._PEERS_FILE.write_text(json.dumps({"mac": {"base_url": "https://mac.ts.net", "token": "p"}}))
    gravar(cli, [maquina("srv-mac", "https://mac.ts.net"), maquina("srv-x", "https://x.ts.net")])
    gravar(cli, [])
    assert json.loads(peers._PEERS_FILE.read_text()) == {"mac": {"base_url": "https://mac.ts.net", "token": "p"}}


def test_entrada_invalida_nao_grava_nada(cli):
    gravar(cli, [maquina("srv-a", "https://a.ts.net")])
    antes = peers._PEERS_FILE.read_text()
    assert gravar(cli, [maquina("srv-b", "ftp://b")]).status_code == 400
    assert gravar(cli, [maquina("srv-b", "https://b"), maquina("srv-b", "https://c")]).status_code == 400
    assert peers._PEERS_FILE.read_text() == antes


def test_registrar_e_tirar_recados_nao_mexe_na_parte_do_app(cli):
    gravar(cli, [maquina("srv-mac", "https://mac.ts.net")])
    [chave] = json.loads(peers._PEERS_FILE.read_text())
    # O registro dos recados de outra ponta reescreve endereço e token: a parte do app fica.
    peers.gravar_peer(chave, "https://mac.ts.net", "novo")
    assert json.loads(peers._PEERS_FILE.read_text())[chave]["app"]["id"] == "srv-mac"
    peers.remover_peer(chave)
    assert json.loads(peers._PEERS_FILE.read_text())[chave] == {"app": {**maquina("srv-mac", "https://mac.ts.net"), "pos": 0}}
    # Só com a parte do app não é peer: não liga, não remove, não aparece na lista de recados.
    with pytest.raises(ValueError):
        peers.set_peer_enabled(chave, False)
    with pytest.raises(ValueError):
        peers.remover_peer(chave)
    assert cli.get("/api/peers", headers=AUTH).json() == []
    assert peers.peer_cfg(chave) is None


def test_hangar_send_list_ignora_a_maquina_so_do_app(tmp_path):
    arquivo = tmp_path / "peers.json"
    arquivo.write_text(json.dumps({
        "srv-conv": {"app": maquina("srv-conv", "https://h:8443", invite=True)},
        "mac": {"base_url": "https://mac", "token": "p", "app": maquina("srv-mac", "https://mac")},
    }))
    # Mesmo filtro do --list do hangar-send, rodado sobre o arquivo.
    fonte = (ROOT / "scripts" / "hangar-send").read_text()
    trecho = fonte.split("peers_tsv=$(\"$PY\" -c '", 1)[1].split("' \"$PEERS_FILE\")", 1)[0]
    saida = subprocess.run([sys.executable, "-c", trecho, str(arquivo)], capture_output=True, text=True, check=True)
    assert saida.stdout.splitlines() == ["mac\thttps://mac\tp"]
