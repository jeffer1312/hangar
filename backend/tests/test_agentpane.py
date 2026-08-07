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


def test_pane_unico_devolve_none_nao_o_pane_id(sessao):
    # Achado 2 da revisao (rodada 1): com 1 pane so, `=nome:` e `%N` apontam pro MESMO lugar -> nao
    # vale o raio de explosao de trocar por `%N` (psmux so tem compatibilidade MEDIDA pra `=NOME:`).
    assert agentpane.resolve_target(sessao) is None
    assert tmux._pane_target(sessao) == f"={sessao}:"


def test_kill_e_recria_mesma_sessao_invalida_o_cache_quente(sessao, monkeypatch):
    # Achado 1 da revisao (rodada 1): registry.py e o adapter do Codex matam e recriam a sessao com o
    # MESMO nome (resume). Sem invalidar, o cache de 60s continuaria apontando pro pane MORTO da vida
    # anterior. Sem sleep de 60s: o teste prova a invalidacao, nao o TTL.
    monkeypatch.setattr(agentpane, "_pane_do_agente", lambda pid, children: True)   # 1o pane sempre "e o agente"

    velho = tmux.list_panes_of(sessao)[0]
    _segunda_janela(sessao)   # so com 2+ panes o resolve_target desce pro /proc (achado 2)
    agentpane.invalidate(sessao)
    assert agentpane.resolve_target(sessao) == velho["pane_id"]   # cache quente, aponta pro pane velho

    assert tmux.kill_session(sessao) is True
    assert tmux.new_session(sessao, "/tmp", "sleep 600") is True   # mesmo nome, sessao nova

    novo = tmux.list_panes_of(sessao)[0]
    assert novo["pane_id"] != velho["pane_id"]   # tmux nunca reusa %N: prova que sao panes diferentes
    _segunda_janela(sessao)
    assert agentpane.resolve_target(sessao) == novo["pane_id"]   # NAO o velho, que kill/new invalidam
