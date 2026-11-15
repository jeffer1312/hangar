import subprocess
import uuid
from unittest.mock import MagicMock, patch

import pytest

from app import tmux

from tmux_teste import matar_servidor, matar_sessao, novo_socket


def test_timeout_do_tmux_nao_vira_lista_vazia():
    # Caso do Windows (25/08/2026): as sessoes sumiam todas do app com o servidor ainda verde.
    # `list-panes -a` estourando o timeout virava rc=1, que era lido como "zero sessões". Agora levanta.
    with patch.object(tmux, "RUN", side_effect=subprocess.TimeoutExpired(["tmux"], 5)):
        with pytest.raises(tmux.MuxIndisponivel):
            tmux.list_panes_all()


def test_timeout_do_tmux_nao_registra_o_argv(caplog):
    # O argv pode carregar o texto de um atalho (credencial): nem o log nem o stderr o repetem.
    args = ["tmux", "new-session", "-d", "--", "sh", "-c", "senha=SEGREDO123"]
    with patch.object(tmux, "RUN", side_effect=subprocess.TimeoutExpired(args, 5)):
        with caplog.at_level("DEBUG"):
            cp = tmux._run(args)
    assert cp.returncode == tmux.RC_INDISPONIVEL and "SEGREDO123" not in cp.stderr
    assert "SEGREDO123" not in caplog.text


def test_tmux_ausente_tambem_levanta():
    # FileNotFoundError (tmux fora do PATH) e a mesma classe de "nao sei", nao "nao ha sessao".
    with patch.object(tmux, "RUN", side_effect=FileNotFoundError("tmux")):
        with pytest.raises(tmux.MuxIndisponivel):
            tmux.list_panes_all()


def test_sem_servidor_rodando_continua_sendo_lista_vazia():
    # A contraparte: rc=1 vindo do PROPRIO tmux ("no server running") e resposta legitima de que
    # nao ha sessao nenhuma — levantar aqui trocaria a lista vazia correta por um erro na tela.
    fake = MagicMock(stdout="", returncode=1, stderr="no server running on /tmp/tmux-1000/default")
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_panes_all() == {}


def test_list_panes_all_tolerates_a_missing_hidden_field():
    # `#{@cp_hidden}` é opção de USUÁRIO no `-F`. Multiplexador que não a interpola (psmux) devolve
    # 5 campos ou o literal cru: perder a MARCA e chato; perder a SESSAO seria o app vazio.
    stdout = ("cc\t1\t123\t/home/u/p\t%9\n"                  # 5 campos: sem a opcao
              "term-cc\t1\t124\t/home/u/p\t%10\t1\n"          # 6 campos: marcada
              "web\t1\t125\t/home/u/w\t%11\t#{@cp_hidden}\n")  # literal nao interpolado
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout=stdout, returncode=0)):
        out = tmux.list_panes_all()
    assert set(out) == {"cc", "term-cc", "web"}
    assert out["cc"][0]["hidden"] is False
    assert out["term-cc"][0]["hidden"] is True
    assert out["web"][0]["hidden"] is False
    assert out["cc"][0]["pane_id"] == "%9"       # os 5 primeiros campos seguem no lugar certo


def test_has_session_is_exact_against_real_tmux():
    # SEMANTICA REAL do tmux (nao mock): sem o `=`, o `-t` resolve exact -> fnmatch -> PREFIX match,
    # então `has_session("X")` respondia VIVO por causa da IRMÃ "X-2".
    # Socket proprio (-L) e sessao de nome aleatorio: nao encosta no tmux/sessoes do usuario.
    sock = novo_socket()
    base = f"pocket-{uuid.uuid4().hex[:6]}"

    def tmux_on_sock(args, **_kw):
        # injeta o `-L <socket>` logo apos o "tmux" -> o has_session real roda contra ESTE servidor
        return subprocess.run(["tmux", "-L", sock, *args[1:]], capture_output=True, text=True)

    subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", f"{base}-2", "sleep 60"],
                   capture_output=True, text=True)
    try:
        with patch.object(tmux, "RUN", tmux_on_sock):
            assert tmux.has_session(f"{base}-2") is True   # exata e viva
            assert tmux.has_session(base) is False         # NUNCA existiu (so a irma "-2") -> prefix mentia
            assert tmux.has_session(base[:-3]) is False    # prefixo puro
    finally:
        matar_sessao(f"{base}-2", sock)
        matar_servidor(sock)
