"""O teardown dos testes que falam com o multiplexador DE VERDADE (`tests/tmux_teste.py`).

Testar o ajudante de teste parece exagero ate se olhar o estrago: matar a sessao nao recolhe o
servidor no psmux, e cada arquivo com socket proprio deixava um `tmux server -s __warm__ -L
cp-test-<hash>` vivo — 70 nesta VM em 22/08/2026, ~12,7 GB de working set, um `powershell` e um
`conhost` presos em cada. Nenhum teste ficou vermelho por causa disso: quem caiu foi a MAQUINA, e
com ela a sessao Claude que rodava a suite (`0xc00000fd`, pilha estourada, 9,8 GB de 10 GB em uso).
Limpeza que ninguem cobra e limpeza que uma hora some.
"""
import shutil
import subprocess
import uuid
from unittest.mock import patch

import pytest

import tmux_teste
from tmux_teste import matar_sessao, matar_servidor, novo_socket, processos_do_socket


def test_matar_servidor_recusa_socket_vazio():
    """`kill-server` sem `-L` derruba o servidor tmux DEFAULT — todas as sessoes de trabalho de
    quem roda a suite. A proibicao estava escrita em quatro comentarios; aqui ela e codigo, e
    ninguem chega a rodar comando nenhum."""
    with patch.object(tmux_teste.subprocess, "run") as run:
        for vazio in ("", None):
            with pytest.raises(ValueError, match="kill-server"):
                matar_servidor(vazio)
    assert run.call_args_list == []


def test_matar_servidor_manda_kill_server_no_proprio_socket():
    with patch.object(tmux_teste.subprocess, "run") as run, \
         patch.object(tmux_teste, "processos_do_socket", return_value=[]):
        run.return_value = subprocess.CompletedProcess([], 0, "", "")
        matar_servidor("cp-test-abc123")
    assert run.call_args[0][0] == ["tmux", "-L", "cp-test-abc123", "kill-server"]


def test_matar_servidor_falha_alto_quando_sobra_processo():
    """Mesma regra do `matar_sessao`: limpeza silenciosa que nao limpa e como teste verde que nao
    testa. O `rc` nao serve de prova aqui — o psmux responde 0 tendo matado servidor ou nao tendo
    achado nenhum —, entao quem decide e a tabela de processos."""
    with patch.object(tmux_teste.subprocess, "run") as run, \
         patch.object(tmux_teste, "processos_do_socket", return_value=[(7, "tmux server -s __warm__")]), \
         patch.object(tmux_teste.time, "sleep"):
        run.return_value = subprocess.CompletedProcess([], 0, "", "")
        with pytest.raises(AssertionError, match="sobreviveu ao kill-server"):
            matar_servidor("cp-test-abc123")


def test_socket_registrado_some_da_lista_depois_de_recolhido():
    """`sockets_vazados()` e o que o fixture de sessao do conftest le no fim da suite. Um socket
    entregue por `novo_socket` fica na lista ate alguem recolher o servidor dele."""
    sock = novo_socket()
    assert sock in tmux_teste._SOCKETS
    with patch.object(tmux_teste.subprocess, "run") as run, \
         patch.object(tmux_teste, "processos_do_socket", return_value=[]):
        run.return_value = subprocess.CompletedProcess([], 0, "", "")
        matar_servidor(sock)
    assert sock not in tmux_teste._SOCKETS


def test_sockets_vazados_acusa_servidor_de_pe():
    sock = novo_socket()
    try:
        with patch.object(tmux_teste, "processos_do_socket",
                          lambda s: [(9, f"tmux server -s __warm__ -L {s}")] if s == sock else []):
            assert sock in tmux_teste.sockets_vazados()
    finally:
        tmux_teste._SOCKETS.discard(sock)


@pytest.mark.skipif(shutil.which("tmux") is None, reason="tmux nao instalado no ambiente")
def test_teardown_de_socket_proprio_nao_deixa_processo_vivo():
    """O caso que o briefing pede: depois do teardown, nada do socket de teste continua rodando.

    Sem OS no meio, e o motivo de passar e diferente em cada um — que e o certo. No tmux o servidor
    sai junto com a ultima sessao e o `kill-server` e um no-op barato; no psmux ele fica, e sem o
    `matar_servidor` esta linha final falha. Medido em 23/08/2026 (psmux 3.3.7):

        kill-session -t zz          rc=0   sessao morta
        list-sessions               rc=0   ''            <- igual a um socket virgem
        tabela de processos                tmux.exe server -s __warm__ -L <socket>
        kill-server                 rc=0   0,1s          -> zero processos
    """
    sock = novo_socket("cp-teste-teardown")
    nome = f"cp-teardown-{uuid.uuid4().hex[:6]}"
    subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", nome, "sleep 60"],
                   check=True, capture_output=True)
    try:
        if not processos_do_socket(sock):
            # Multiplexador que nao carrega o socket na cmdline do servidor: a conferencia por
            # processo nao mede nada aqui, e um verde por cegueira e pior que um skip.
            pytest.skip("o servidor deste multiplexador nao se identifica pelo socket na cmdline")
        matar_sessao(nome, sock)          # o teardown de antes, exatamente como era
        matar_servidor(sock)              # o que faltava
        assert processos_do_socket(sock) == []
    finally:
        subprocess.run(["tmux", "-L", sock, "kill-server"], capture_output=True)
        tmux_teste._SOCKETS.discard(sock)
