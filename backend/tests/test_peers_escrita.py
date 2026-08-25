"""Escrita do peers.json via peers.py — gravar/regravar/apagar o arquivo que guarda os TOKENS
da malha cross-server. O peers.json é lido pelo hangar-send, pelas rotas de pareamento e pela lista
de origens permitidas do terminal: gravação torta ali derruba todo servidor remoto de uma vez.
"""
import json
import os

import pytest

from app import peers


@pytest.fixture
def arquivo(tmp_path, monkeypatch):
    """Seam de I/O do módulo: o teste aponta o caminho do arquivo pra um tmp, nunca o real."""
    p = tmp_path / "peers.json"
    monkeypatch.setattr(peers, "_PEERS_FILE", p)
    return p


def test_gravar_cria_a_entrada(arquivo):
    peers.gravar_peer("notebook", "http://192.168.0.77:8765", "segredo-do-notebook")
    d = json.loads(arquivo.read_text(encoding="utf-8"))
    assert d["notebook"] == {"base_url": "http://192.168.0.77:8765", "token": "segredo-do-notebook"}


def test_regravar_substitui_nao_duplica(arquivo):
    peers.gravar_peer("notebook", "http://antigo:8765", "token-antigo")
    peers.gravar_peer("notebook", "http://novo:8765", "token-novo")
    d = json.loads(arquivo.read_text(encoding="utf-8"))
    assert list(d) == ["notebook"]                      # não duplicou
    assert d["notebook"]["base_url"] == "http://novo:8765"
    assert d["notebook"]["token"] == "token-novo"


def test_gravar_preserva_enabled_do_painel(arquivo):
    """O toggle do painel (hangar_panel_common) grava enabled no arquivo; regravar pelo app não pode
    sumir com ele — senão um peer desligado volta pro scan calado."""
    peers.gravar_peer("pc", "http://pc:8765", "t")
    dados = peers.listar_peers()
    dados["pc"]["enabled"] = False
    arquivo.write_text(json.dumps(dados), encoding="utf-8")   # mão do painel
    peers.gravar_peer("pc", "http://pc:8765", "t")
    assert peers.listar_peers()["pc"]["enabled"] is False


def test_gravar_preserva_web_url_do_painel(arquivo):
    """web_url é o link que o painel usa pra abrir a máquina remota; o formulário do app não
    tem esse campo, então regravar pela tela não pode apagá-lo."""
    peers.gravar_peer("pc", "http://pc:8765", "t", "https://pc.ts.net")
    peers.gravar_peer("pc", "http://pc:8765", "t2")          # sem web_url, como a tela manda
    assert peers.listar_peers()["pc"]["web_url"] == "https://pc.ts.net"
    assert peers.listar_peers()["pc"]["token"] == "t2"
    peers.gravar_peer("pc", "http://pc:8765", "t2", "https://novo.ts.net")
    assert peers.listar_peers()["pc"]["web_url"] == "https://novo.ts.net"   # informado, sobrescreve


def test_arquivo_corrompido_recusa_escrever_por_cima(arquivo):
    """Corrompido à mão: recusar é certo, mas apagar o que sobrou seria perder os tokens de vez
    — o usuário ainda consegue recuperar editando o arquivo (mesmo racional do hangar_panel_common)."""
    arquivo.write_text('{"pc": {"token": "x"', encoding="utf-8")
    with pytest.raises(ValueError, match="ilegível"):
        peers.gravar_peer("notebook", "http://n:8765", "t")
    assert arquivo.read_text(encoding="utf-8") == '{"pc": {"token": "x"'


def test_apagar_remove(arquivo):
    peers.gravar_peer("a", "http://a:8765", "ta")
    peers.gravar_peer("b", "http://b:8765", "tb")
    peers.remover_peer("a")
    d = json.loads(arquivo.read_text(encoding="utf-8"))
    assert list(d) == ["b"]


def test_apagar_desconhecido_recusa_e_nao_escreve(arquivo):
    peers.gravar_peer("a", "http://a:8765", "ta")
    with pytest.raises(ValueError, match="não está no peers.json"):
        peers.remover_peer("nao-existe")
    assert json.loads(arquivo.read_text(encoding="utf-8"))["a"]["token"] == "ta"


# LACUNA VISIVEL, nao contornada: o peers.json guarda os TOKENS da malha, e no Windows ele NAO
# esta protegido — la nao existe bit de modo (o st_mode volta 0o666 e quem decide acesso e a ACL),
# e a ACL equivalente ainda nao esta implementada. Esconder isso atras de um assert que nao roda
# seria pior; e o mesmo tratamento que o test_pi_inbox ja da ao arquivo de conexao do Pi.
@pytest.mark.skipif(os.name != "posix",
                    reason="modo 0600 nao existe no Windows; a protecao por ACL ainda nao existe")
def test_arquivo_nasce_e_permanece_0600(arquivo):
    peers.gravar_peer("a", "http://a:8765", "ta")
    assert arquivo.stat().st_mode & 0o777 == 0o600
    arquivo.chmod(0o644)          # mão na config deixou frouxo
    peers.gravar_peer("a", "http://a:8765", "ta")
    peers.remover_peer("a")
    assert arquivo.stat().st_mode & 0o777 == 0o600     # gravação devolve 0600


def test_arquivo_ausente_continua_sendo_sem_peers(tmp_path, monkeypatch):
    monkeypatch.setattr(peers, "_PEERS_FILE", tmp_path / "nunca-existiu.json")
    assert peers.listar_peers() == {}


def test_identificador_malformado_recusado_antes_de_escrever(arquivo):
    """fullmatch e não match+$: com `$`, 'casa\n' (quebra de linha final) passava e o arquivo
    nascia com controle de linha no nome (mesmo cuidado de contas.py:79)."""
    for ruim in ("casa\n", "Casa", "ca sa", "", "a" * 40, "contas/", "casa::x"):
        with pytest.raises(ValueError):
            peers.gravar_peer(ruim, "http://x:8765", "t")
    assert not arquivo.exists()                          # recusa é recusa: nada escrito


def test_gravar_recusa_url_e_token_invalidos_antes_de_escrever(arquivo):
    for base, token in (("ftp://x", "t"), ("x:8765", "t"), ("http://x:8765", "")):
        with pytest.raises(ValueError):
            peers.gravar_peer("notebook", base, token)
    assert not arquivo.exists()


def test_gravacao_concorrente_nao_perde_update(arquivo):
    """Duas gravações ao mesmo tempo (app + CLI/painel) não podem apagar a mudança uma da outra:
    sem lock, é read-modify-write do arquivo inteiro — quem grava por último recarrega o estado
    ANTIGO do outro. Este é o teste do lock, não do os.replace."""
    import threading

    inicio = threading.Barrier(2)

    def grava(pid: str):
        inicio.wait()
        peers.gravar_peer(pid, f"http://{pid}:8765", f"t-{pid}")

    ts = [threading.Thread(target=grava, args=(p,)) for p in ("pc", "srv")]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    d = json.loads(arquivo.read_text(encoding="utf-8"))
    assert d["pc"]["token"] == "t-pc"
    assert d["srv"]["token"] == "t-srv"


def test_gravar_nao_deixa_tmp_para_tras(arquivo):
    orfao = arquivo.with_name(arquivo.name + ".morto.tmp")
    orfao.write_text('{"vazou": "token-antigo"}', encoding="utf-8")
    peers.gravar_peer("a", "http://a:8765", "ta")
    assert not orfao.exists()
    assert list(arquivo.parent.glob(arquivo.name + ".*.tmp")) == []