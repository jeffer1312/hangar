"""Borda HTTP dos peers — máquinas que este servidor alcança (aba Servidores).

A Task 1 só abre o espaço: a rota de listagem existe e devolve lista vazia, em vez de 404.
Quem preenche o conteúdo é a Task 5 (e a 8, no Lote B), dentro deste módulo. O módulo de ROTA é
`peers_api.py`: `app/peers.py` já existe e é a lógica de pareamento cross-server, não a rota.
"""
import json

import pytest
from fastapi.testclient import TestClient

from app.api import app
from app.config import settings

# Convenção da casa (ver test_engines_api.py): cada arquivo declara o próprio token.
TOKEN = "t-peers"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture(autouse=True)
def _isola(monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    yield


@pytest.fixture
def cli():
    return TestClient(app)


def test_listagem_vazia(arquivo_peers, cli):
    # arquivo_peers isola o peers.json da máquina (tmp_path + monkeypatch): sem a fixture,
    # este teste lia o peers.json REAL do usuário — e a suíte passava na worktree (arquivo
    # ausente) e quebrava na main (arquivo presente).
    r = cli.get("/api/peers", headers=AUTH)
    assert r.status_code == 200
    assert r.json() == []


# ── Listagem: credencial SEMPRE mascarada, e 401 sem credencial ─────────────────────


def test_listagem_sem_credencial_e_401(cli):
    assert cli.get("/api/peers").status_code == 401


@pytest.fixture
def arquivo_peers(tmp_path, monkeypatch):
    """Seam de I/O: aponta o peers.json do backend pra um tmp — a rota grava/remove de verdade
    no arquivo, mas nunca no real da máquina."""
    from app import peers
    p = tmp_path / "peers.json"
    monkeypatch.setattr(peers, "_PEERS_FILE", p)
    return p


def test_listagem_mascara_a_credencial(arquivo_peers, cli):
    arquivo_peers.write_text(json.dumps({
        "notebook": {"base_url": "http://192.168.0.77:8765", "token": "segredo-super-secreto",
                     "web_url": "https://notebook"}},
        ensure_ascii=False))
    r = cli.get("/api/peers", headers=AUTH)
    assert r.status_code == 200
    [peer] = r.json()
    assert peer["id"] == "notebook"
    assert peer["base_url"] == "http://192.168.0.77:8765"
    assert peer["web_url"] == "https://notebook"
    # Máscara tipo runtime_config (gsk_••••••••1234): conferível, não copiável.
    assert peer["token"] != "segredo-super-secreto"
    assert "•" in peer["token"]
    assert peer["token"].endswith("reto")
    # A lista é o único lugar em que o token sai; o arquivo guarda o valor real.
    assert json.loads(arquivo_peers.read_text(encoding="utf-8"))["notebook"]["token"] == "segredo-super-secreto"


def test_listagem_ignora_entrada_malformada(arquivo_peers, cli):
    arquivo_peers.write_text(json.dumps({"pc": "nao-e-dict", "ok": {"base_url": "http://ok:8765", "token": "t"}}),
                             encoding="utf-8")
    r = cli.get("/api/peers", headers=AUTH)
    assert [p["id"] for p in r.json()] == ["ok"]


# ── Gravar e apagar ─────────────────────────────────────────────────────────────────


def test_gravar_peer(arquivo_peers, cli):
    r = cli.post("/api/peers", headers=AUTH, json={
        "id": "notebook", "base_url": "http://192.168.0.77:8765", "token": "segredo"})
    assert r.status_code == 200
    assert [p["id"] for p in r.json()] == ["notebook"]
    # O arquivo guarda o token REAL; só a borda HTTP mascara.
    assert json.loads(arquivo_peers.read_text(encoding="utf-8"))["notebook"]["token"] == "segredo"


def test_gravar_sem_credencial_e_401(cli):
    assert cli.post("/api/peers", json={"id": "x", "base_url": "http://x", "token": "t"}).status_code == 401


def test_gravar_peer_invalido_e_400_sem_escrever(arquivo_peers, cli):
    r = cli.post("/api/peers", headers=AUTH, json={"id": "Notebook\n", "base_url": "x", "token": ""})
    assert r.status_code == 400
    assert not arquivo_peers.exists()          # recusa é recusa: nada escrito


def test_regravar_peer_substitui(arquivo_peers, cli):
    cli.post("/api/peers", headers=AUTH, json={"id": "notebook", "base_url": "http://a:8765", "token": "ta"})
    cli.post("/api/peers", headers=AUTH, json={"id": "notebook", "base_url": "http://b:8765", "token": "tb"})
    r = cli.get("/api/peers", headers=AUTH)
    assert len(r.json()) == 1                    # não duplicou
    assert json.loads(arquivo_peers.read_text(encoding="utf-8"))["notebook"]["base_url"] == "http://b:8765"


def test_apagar_peer(arquivo_peers, cli):
    cli.post("/api/peers", headers=AUTH, json={"id": "notebook", "base_url": "http://n:8765", "token": "t"})
    r = cli.delete("/api/peers/notebook", headers=AUTH)
    assert r.status_code == 200
    assert r.json() == []
    assert json.loads(arquivo_peers.read_text(encoding="utf-8")) == {}


def test_apagar_desconhecido_e_404(arquivo_peers, cli):
    r = cli.delete("/api/peers/nao-existe", headers=AUTH)
    assert r.status_code == 404


def test_apagar_sem_credencial_e_401(cli):
    assert cli.delete("/api/peers/x").status_code == 401


@pytest.mark.parametrize("corpo", [
    {"id": 123, "base_url": "http://x:8765", "token": "t"},
    {"id": [1], "base_url": "http://x:8765", "token": "t"},
    {"id": "ok", "base_url": "http://x:8765", "token": "t", "web_url": 7},
])
def test_gravar_campo_nao_texto_e_400_nomeado_sem_escrever(arquivo_peers, cli, corpo):
    """Tipo errado é recusa nomeada (400), não 500: o front só sabe traduzir o 400."""
    r = cli.post("/api/peers", headers=AUTH, json=corpo)
    assert r.status_code == 400
    assert not arquivo_peers.exists()


# ── Identificador desta máquina: deixou de ser somente-leitura ─────────────────────


# ── Checagem de um peer (Task 8): a rota devolve o estado nomeado ─────────────────


def test_checar_peer_rota_ok(monkeypatch, cli):
    from app import peers, peers_check
    monkeypatch.setattr(peers_check, "_bater", lambda url, path="/api/peers/identificador", token=None: (200, {"identificador": "notebook"}))
    monkeypatch.setattr(peers, "peer_cfg", lambda sid: None)  # peers.json da máquina não entra no teste
    r = cli.get("/api/peers/check?url=http%3A%2F%2Fnotebook%3A8765&id=notebook", headers=AUTH)
    assert r.status_code == 200
    assert r.json()["estado"] == "ok"


def test_checar_peer_rota_sem_credencial_e_401(cli):
    assert cli.get("/api/peers/check?url=http%3A%2F%2Fn%3A8765&id=n").status_code == 401


def test_checar_peer_rota_url_faltando_e_400_nomeado(monkeypatch, cli):
    from app import peers, peers_check
    monkeypatch.setattr(peers_check, "_bater", lambda url, path="/api/peers/identificador", token=None: (200, {"identificador": "n"}))
    monkeypatch.setattr(peers, "peer_cfg", lambda sid: None)
    r = cli.get("/api/peers/check?id=n", headers=AUTH)
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "peers_check_url_obrigatoria"


def test_checar_peer_rota_id_faltando_e_400_nomeado(monkeypatch, cli):
    from app import peers, peers_check
    monkeypatch.setattr(peers_check, "_bater", lambda url, path="/api/peers/identificador", token=None: (200, {"identificador": "n"}))
    monkeypatch.setattr(peers, "peer_cfg", lambda sid: None)
    r = cli.get("/api/peers/check?url=http%3A%2F%2Fn%3A8765", headers=AUTH)
    assert r.status_code == 400
    assert r.json()["detail"]["code"] == "peers_check_id_obrigatorio"



@pytest.fixture
def env_tmp(tmp_path, monkeypatch):
    """Seam de I/O do identificador: o .env da máquina não pode aparecer num teste de rota."""
    from app import peers_api
    p = tmp_path / ".env"
    monkeypatch.setattr(peers_api, "_ENV_ARQUIVO", p)
    monkeypatch.setattr(settings, "server_id", "")
    return p


def test_identificador_leitura(env_tmp, cli):
    r = cli.get("/api/peers/identificador", headers=AUTH)
    assert r.status_code == 200
    assert r.json() == {"identificador": ""}


def test_identificador_leitura_sem_credencial_e_401(cli):
    assert cli.get("/api/peers/identificador").status_code == 401


def test_gravar_identificador(env_tmp, cli):
    r = cli.put("/api/peers/identificador", headers=AUTH, json={"identificador": "casa"})
    assert r.status_code == 200
    assert r.json() == {"identificador": "casa"}
    assert settings.server_id == "casa"           # o processo passa a parear na hora
    assert "CP_SERVER_ID=casa" in env_tmp.read_text(encoding="utf-8")   # o cp-send lê daqui
    assert cli.get("/api/peers/identificador", headers=AUTH).json() == {"identificador": "casa"}


def test_gravar_identificador_sem_credencial_e_401(cli):
    assert cli.put("/api/peers/identificador", json={"identificador": "casa"}).status_code == 401


def test_gravar_identificador_invalido_e_400(env_tmp, cli):
    r = cli.put("/api/peers/identificador", headers=AUTH, json={"identificador": "Casa\n"})
    assert r.status_code == 400
    assert not env_tmp.exists()                   # nada escrito
    assert settings.server_id == ""


def test_gravar_identificador_vazio_remove_a_linha(env_tmp, cli):
    env_tmp.write_text("CP_AUTH_TOKEN=abc\nCP_SERVER_ID=casa\nCP_PORT=8774\n", encoding="utf-8")
    r = cli.put("/api/peers/identificador", headers=AUTH, json={"identificador": ""})
    assert r.status_code == 200
    texto = env_tmp.read_text(encoding="utf-8")
    assert "CP_SERVER_ID" not in texto           # linha some: ausente = pareamento desligado
    assert "CP_AUTH_TOKEN=abc" in texto          # e o resto do .env ficou intacto
    assert settings.server_id == ""


def test_gravar_identificador_preserva_o_resto_do_env(env_tmp, cli):
    env_tmp.write_text("CP_AUTH_TOKEN=abc\n# comentário do dono\nCP_VAPID_PUBLIC=x\n", encoding="utf-8")
    cli.put("/api/peers/identificador", headers=AUTH, json={"identificador": "casa"})
    assert env_tmp.read_text(encoding="utf-8") == (
        "CP_AUTH_TOKEN=abc\n# comentário do dono\nCP_VAPID_PUBLIC=x\nCP_SERVER_ID=casa\n")


@pytest.mark.parametrize("valor", [1, {"a": 1}, [2]])
def test_identificador_nao_texto_e_400_nomeado(env_tmp, cli, valor):
    """Tipo errado é recusa nomeada (400), não 500: o front só sabe traduzir o 400."""
    r = cli.put("/api/peers/identificador", headers=AUTH, json={"identificador": valor})
    assert r.status_code == 400
    assert not env_tmp.exists()
    assert settings.server_id == ""


def test_gravar_identificador_nao_deixa_tmp_para_tras(env_tmp, cli):
    """O tmp é cópia integral do .env (CP_AUTH_TOKEN, VAPID). Órfão de um processo morto não
    pode ficar no disco — nem esperando a próxima gravação."""
    env_tmp.write_text("CP_AUTH_TOKEN=abc\n", encoding="utf-8")
    orfao = env_tmp.with_name(env_tmp.name + ".999.tmp")
    orfao.write_text("CP_AUTH_TOKEN=abc\n", encoding="utf-8")
    cli.put("/api/peers/identificador", headers=AUTH, json={"identificador": "casa"})
    assert not orfao.exists()
    assert list(env_tmp.parent.glob(env_tmp.name + ".*.tmp")) == []


def test_gravar_identificador_espera_a_trava_de_outro_escritor(env_tmp):
    """A trava deste .env tem que ser trava DE VERDADE nos dois sistemas — não no-op num deles.

    Este arquivo não é o peers.json: ele guarda o CP_AUTH_TOKEN (a única credencial do app) e as
    chaves VAPID, e a gravação é read-modify-write do texto inteiro. O módulo tinha `fcntl` com
    `except ImportError: fcntl = None`, o que no Windows virava trava nenhuma — dois escritores
    (a tela e o instalador, ou a tela e o editor do dono) liam o mesmo estado e o último apagava
    a mudança do outro, calado. O teste da concorrência em si é o
    `test_gravacao_concorrente_nao_perde_update` do peers; aqui a pergunta é mais direta e não
    depende de timing pra falhar: com a trava tomada por OUTRO escritor, a gravação espera.

    Vale nos dois: `flock` é por descrição de arquivo aberto e `msvcrt.locking` é por handle, e
    nos dois casos um segundo handle do MESMO processo é barrado (LockFileEx documenta isso
    explicitamente). Por isso duas threads bastam — não precisa de subprocesso.
    """
    import threading

    from app import peers, peers_api

    env_tmp.write_text("CP_AUTH_TOKEN=abc\n", encoding="utf-8")
    terminou = threading.Event()

    def escreve():
        peers_api._escrever_id_env("casa")
        terminou.set()

    with open(env_tmp.with_name(env_tmp.name + ".lock"), "a+", encoding="utf-8") as outro:
        peers._travar(outro)
        t = threading.Thread(target=escreve)
        t.start()
        try:
            assert not terminou.wait(1.0), "gravou com a trava de outro escritor tomada"
        finally:
            peers._destravar(outro)
    t.join(15)
    assert terminou.is_set(), "não destravou: a gravação nunca terminou"
    # E o que ela escreveu continua certo — a trava não pode ter custado o conteúdo.
    texto = env_tmp.read_text(encoding="utf-8")
    assert "CP_SERVER_ID=casa" in texto
    assert "CP_AUTH_TOKEN=abc" in texto