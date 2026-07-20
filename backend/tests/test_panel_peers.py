"""scripts/cp_panel_common.py — o flag que tira um peer da varredura e a gravação dele.

Testado porque enabled() decide se um servidor SOME da lista (errar o default apagaria peers que
sempre funcionaram, sem barulho nenhum) e porque set_peer_enabled() reescreve o peers.json, que
guarda os TOKENS da malha: gravação torta ali derruba todo servidor remoto de uma vez.
"""
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
from cp_panel_common import PanelError, enabled, set_peer_enabled  # noqa: E402


@pytest.mark.parametrize("cfg,esperado", [
    ({"base_url": "http://x", "token": "t"}, True),          # sem o campo = ligado (peers.json antigo)
    ({"base_url": "http://x", "enabled": True}, True),
    ({"base_url": "http://x", "enabled": False}, False),     # único jeito de desligar
    ({"enabled": 0}, True),        # 0 não é False: só o literal desliga, senão typo apaga peer
    ({"enabled": "false"}, True),  # string idem — "false" de JSON mal escrito não conta
    ("nao-e-dict", True),          # entrada malformada segue pra peer_fields, que reporta o erro
    (None, True),
])
def test_enabled(cfg, esperado):
    assert enabled(cfg) is esperado


@pytest.fixture
def peers_file(tmp_path):
    p = tmp_path / "peers.json"
    p.write_text(json.dumps({
        "pc": {"base_url": "http://pc:8765", "token": "segredo-do-pc", "web_url": "https://pc"},
        "srv": {"base_url": "http://srv:8765", "token": "segredo-do-srv"},
    }), encoding="utf-8")
    p.chmod(0o600)
    return p


def test_set_peer_enabled_preserva_token_e_vizinhos(peers_file):
    """O toggle mexe em UM campo de UM peer. Reescrever o arquivo inteiro é o risco: perder o
    token (malha inteira inalcançável) ou o web_url (link do painel quebrado) seria mudo."""
    set_peer_enabled("pc", False, peers_file)
    d = json.loads(peers_file.read_text(encoding="utf-8"))

    assert d["pc"] == {"base_url": "http://pc:8765", "token": "segredo-do-pc",
                       "web_url": "https://pc", "enabled": False}
    assert d["srv"] == {"base_url": "http://srv:8765", "token": "segredo-do-srv"}


def test_set_peer_enabled_ida_e_volta(peers_file):
    set_peer_enabled("pc", False, peers_file)
    set_peer_enabled("pc", True, peers_file)
    assert enabled(json.loads(peers_file.read_text(encoding="utf-8"))["pc"]) is True


def test_set_peer_enabled_mantem_o_modo(peers_file):
    """Arquivo de credencial: o tmp nasce com o umask do processo, então sem chmod o 600 viraria
    644 na primeira vez que alguém clicasse o toggle."""
    set_peer_enabled("pc", False, peers_file)
    assert peers_file.stat().st_mode & 0o777 == 0o600


def test_set_peer_enabled_servidor_desconhecido(peers_file):
    with pytest.raises(PanelError, match="não está no peers.json"):
        set_peer_enabled("nao-existe", False, peers_file)
    # Arquivo intocado: recusar tem que ser recusa, não gravação parcial.
    assert "nao-existe" not in json.loads(peers_file.read_text(encoding="utf-8"))


def test_set_peer_enabled_json_quebrado_nao_apaga(tmp_path):
    """peers.json corrompido à mão: falhar é certo, mas apagar o que sobrou seria perder os
    tokens de vez — o usuário ainda consegue recuperar editando o arquivo."""
    p = tmp_path / "peers.json"
    p.write_text('{"pc": {"token": "x"', encoding="utf-8")
    with pytest.raises(PanelError, match="ilegível"):
        set_peer_enabled("pc", False, p)
    assert p.read_text(encoding="utf-8") == '{"pc": {"token": "x"'


def test_set_peer_enabled_concorrente_nao_perde_update(peers_file):
    """Painel e CLI podem gravar ao mesmo tempo. Sem lock, é read-modify-write do arquivo inteiro:
    quem grava por último recarrega o estado ANTIGO do outro e apaga a mudança dele — calado."""
    import threading

    inicio = threading.Barrier(2)

    def liga(pid: str):
        inicio.wait()          # maximiza a sobreposição das duas janelas de RMW
        set_peer_enabled(pid, False, peers_file)

    ts = [threading.Thread(target=liga, args=(p,)) for p in ("pc", "srv")]
    for t in ts:
        t.start()
    for t in ts:
        t.join()

    d = json.loads(peers_file.read_text(encoding="utf-8"))
    # As DUAS mudanças precisam sobreviver — este é o teste do lock, não do os.replace.
    assert d["pc"]["enabled"] is False
    assert d["srv"]["enabled"] is False
    assert d["pc"]["token"] == "segredo-do-pc"
    assert d["srv"]["token"] == "segredo-do-srv"


def test_set_peer_enabled_nao_deixa_tmp_para_tras(peers_file):
    """.tmp órfão carrega a malha INTEIRA de tokens. Um morto de processo anterior tem que sumir."""
    orfao = peers_file.with_name(peers_file.name + ".morto-em-2020.tmp")
    orfao.write_text('{"vazou": "token-antigo"}', encoding="utf-8")

    set_peer_enabled("pc", False, peers_file)

    assert not orfao.exists()
    assert list(peers_file.parent.glob(peers_file.name + ".*.tmp")) == []
