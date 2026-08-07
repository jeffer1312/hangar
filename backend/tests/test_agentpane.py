import subprocess
import pytest
from app import agentpane, tmux

SESS = "cp-test-agentpane"


@pytest.fixture
def sessao():
    subprocess.run(["tmux", "kill-session", "-t", f"={SESS}"], capture_output=True)
    subprocess.run(["tmux", "new-session", "-d", "-s", SESS, "-x", "200", "-y", "50",
                    "sleep 600"], check=True)
    yield SESS
    subprocess.run(["tmux", "kill-session", "-t", f"={SESS}"], capture_output=True)


def _segunda_janela(nome):
    subprocess.run(["tmux", "new-window", "-t", f"={nome}", "sleep 600"], check=True)


def test_list_panes_of_traz_todos_os_panes(sessao):
    assert len(tmux.list_panes_of(sessao)) == 1
    _segunda_janela(sessao)
    panes = tmux.list_panes_of(sessao)
    assert len(panes) == 2
    assert all(p["pane_id"].startswith("%") and p["pid"] > 0 for p in panes)


def test_alvo_e_o_pane_do_agente_mesmo_com_janela_nova(sessao, monkeypatch):
    primeiro = tmux.list_panes_of(sessao)[0]
    monkeypatch.setattr(agentpane, "_pane_do_agente",
                        lambda pid, children: pid == primeiro["pid"])
    _segunda_janela(sessao)
    agentpane.invalidate(sessao)
    assert agentpane.resolve_target(sessao) == primeiro["pane_id"]
    assert tmux._pane_target(sessao) == primeiro["pane_id"]


def test_sem_pane_de_agente_cai_no_alvo_antigo(sessao, monkeypatch):
    # A segunda janela e OBRIGATORIA aqui: com um pane so, resolve_target usa o atalho e nunca
    # chama _pane_do_agente — o teste passaria/falharia por outro motivo. (Achado do pass: a 1a
    # versao deste teste era impossivel de passar.)
    _segunda_janela(sessao)
    monkeypatch.setattr(agentpane, "_pane_do_agente", lambda pid, children: False)
    agentpane.invalidate(sessao)
    assert agentpane.resolve_target(sessao) is None
    assert tmux._pane_target(sessao) == f"={sessao}:"


def test_sessao_inexistente_nao_explode():
    assert agentpane.resolve_target("cp-test-nao-existe") is None
    assert tmux._pane_target("cp-test-nao-existe") == "=cp-test-nao-existe:"


def test_pane_pid_e_do_pane_do_agente(sessao, monkeypatch):
    primeiro = tmux.list_panes_of(sessao)[0]
    monkeypatch.setattr(agentpane, "_pane_do_agente",
                        lambda pid, children: pid == primeiro["pid"])
    _segunda_janela(sessao)
    agentpane.invalidate(sessao)
    # pane_pid e a fonte autoritativa do .jsonl (registry.py:485). Com list-panes -t %N o tmux
    # resolve a JANELA do pane e devolve o primeiro dela — nao o pane pedido.
    assert tmux.pane_pid(sessao) == primeiro["pid"]
