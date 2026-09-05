"""tmux.cwd_de: o cwd de UMA sessao, sem a varredura completa do registry.

Dirige um tmux DE VERDADE (mesma razao do test_agentpane: o valor testado e a resposta do
multiplexador, e um mock so provaria que o mock devolve o que eu escrevi nele).
"""
import shutil
import subprocess
import uuid

import pytest
from app import tmux

from tmux_teste import matar_servidor, matar_sessao, novo_socket

pytestmark = pytest.mark.skipif(shutil.which("tmux") is None,
                                reason="tmux nao instalado no ambiente")


@pytest.fixture
def sessao(tmp_path):
    sock = novo_socket()
    nome = f"cp-cwd-{uuid.uuid4().hex[:6]}"

    def tmux_no_socket(args, **_kw):
        if "tmux" not in args:
            return subprocess.run(args, capture_output=True, text=True, errors="replace")
        i = args.index("tmux")
        return subprocess.run([*args[:i + 1], "-L", sock, *args[i + 1:]],
                              capture_output=True, text=True, errors="replace")

    # Diretorio com ESPACO e ACENTO de proposito: e o formato do cwd real desta maquina
    # ("Área de trabalho"), e a rota /commands usa o retorno pra achar as skills do projeto.
    dir_sessao = tmp_path / "Área de trabalho"
    dir_sessao.mkdir()
    subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", nome,
                    "-c", str(dir_sessao), "sleep 600"], check=True, capture_output=True)
    try:
        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(tmux, "RUN", tmux_no_socket)
            yield nome, str(dir_sessao), sock
    finally:
        matar_sessao(nome, sock)
        matar_servidor(sock)


def test_devolve_o_cwd_da_sessao(sessao):
    nome, dir_sessao, _sock = sessao
    assert tmux.cwd_de(nome) == dir_sessao


def test_sessao_inexistente_e_none(sessao):
    # None, e nunca "" nem o cwd de outra sessao: e o que faz a rota /commands cair no plano B
    # (registry.list(), que enxerga tambem a sessao Codex, que vive num sidecar sem pane).
    assert tmux.cwd_de(f"cp-nao-existe-{uuid.uuid4().hex[:6]}") is None


def test_nome_numerico_sem_sessao_nao_cai_na_sessao_anexada(sessao):
    # O `:` do alvo: sem ele um nome NUMERICO e lido como INDICE DE JANELA e o `list-panes -s`
    # responde pelos panes da sessao ANEXADA com rc=0 — a rota devolveria o cwd de outra sessao
    # com cara de sucesso. Pegadinha ja documentada no `list_panes_of`, que usa o mesmo comando.
    assert tmux.cwd_de("0") is None


def test_com_2_panes_devolve_none_em_vez_de_chutar(sessao):
    # O cuidado principal da funcao. Com 2+ panes quem escolhe o cwd no registry e o `_agent_pane`,
    # que procura o pane do AGENTE — nao o ativo (que no tmux e por JANELA). Com o agente numa
    # janela e um shell na outra em foco, responder "pelo ativo" daria o cwd do SHELL, e a rota
    # /commands listaria as skills do projeto errado. None manda o chamador pra varredura.
    nome, _dir, sock = sessao
    subprocess.run(["tmux", "-L", sock, "new-window", "-t", f"={nome}:", "-c", "/tmp", "sleep 600"],
                   check=True, capture_output=True)
    assert tmux.cwd_de(nome) is None
