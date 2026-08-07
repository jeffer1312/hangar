"""Task 5.5: registry.list() resolve pelo pane do AGENTE, nao so pelo pane ATIVO.

Mesmo padrao do test_agentpane.py: tmux REAL, fixture que mata a sessao no finally (inclusive
quando o teste falha), e o predicado _pane_do_agente monkeypatchado (nao depende de um `claude` de
verdade rodando).
"""
import logging
import subprocess
import pytest
from app import agentpane, registry, tmux

SESS = "cp-test-registry-agentpane"
_UUID = "12345678-1234-1234-1234-123456789abc"


@pytest.fixture
def sessao(tmp_path):
    # cwd EXCLUSIVO desta sessao (tmp_path): _cwd_has_siblings faz um `tmux list-panes -a` real e
    # conta por cwd -- com "/tmp" cru colidiria com qualquer outra sessao da maquina que use /tmp.
    subprocess.run(["tmux", "kill-session", "-t", f"={SESS}"], capture_output=True)
    subprocess.run(["tmux", "new-session", "-d", "-s", SESS, "-c", str(tmp_path),
                    "-x", "200", "-y", "50", "sleep 600"], check=True)
    yield SESS
    subprocess.run(["tmux", "kill-session", "-t", f"={SESS}"], capture_output=True)


def _segunda_janela(nome, cwd):
    subprocess.run(["tmux", "new-window", "-t", f"={nome}", "-c", cwd, "sleep 600"], check=True)


def test_list_resolve_pelo_pane_do_agente_com_janela_extra(sessao, tmp_path, monkeypatch):
    # O teste que importa (brief item 1): agente na janela 0, segunda janela em primeiro plano com
    # o shell. registry.list() tem que devolver TRACKED e com jsonl resolvido -- nao "sem id".
    agente = tmux.list_panes_of(sessao)[0]
    _segunda_janela(sessao, str(tmp_path))

    monkeypatch.setattr(agentpane, "_pane_do_agente", lambda pid, children: pid == agente["pid"])
    monkeypatch.setattr(registry, "_cmdline",
                        lambda pid: f"claude --session-id {_UUID}" if pid == agente["pid"] else "bash")

    reg = registry.SessionRegistry(projects_dir=tmp_path)
    info = {i.name: i for i in reg.list()}[sessao]

    assert info.tracked is True
    assert info.jsonl is not None and info.jsonl.endswith(f"{_UUID}.jsonl")


def test_pane_unico_e_byte_identico_ao_pane_ativo(sessao):
    # Brief item 2: sessao de UM pane so nao pode mudar de comportamento. Com 1 pane so, _agent_pane
    # nunca chama o predicado -- so o fallback do ativo, os mesmos campos que list_panes_active ja
    # trazia (name/pid/cwd/pane_id).
    ativo = next(p for p in tmux.list_panes_active() if p["name"] == sessao)
    grupo = tmux.list_panes_all()[sessao]
    assert len(grupo) == 1

    escolhido = registry.SessionRegistry._agent_pane(grupo, {})

    for campo in ("name", "pid", "cwd", "pane_id"):
        assert escolhido[campo] == ativo[campo]


def test_sem_pane_de_agente_cai_no_ativo_e_loga(sessao, tmp_path, monkeypatch, caplog):
    # Brief item 3: 2+ panes, nenhum reconhecido -> cai no pane ATIVO sem excecao, e loga (uma vez).
    _segunda_janela(sessao, str(tmp_path))
    monkeypatch.setattr(agentpane, "_pane_do_agente", lambda pid, children: False)

    ativo = next(p for p in tmux.list_panes_active() if p["name"] == sessao)
    grupo = tmux.list_panes_all()[sessao]

    with caplog.at_level(logging.WARNING, logger="claude_pocket.registry"):
        escolhido = registry.SessionRegistry._agent_pane(grupo, {})

    assert escolhido["pane_id"] == ativo["pane_id"]
    assert any("nenhum parece do agente" in r.message for r in caplog.records)


def test_list_nao_faz_fork_por_sessao(sessao, tmp_path, monkeypatch):
    # Brief item 4: a correcao NAO pode acrescentar fork/varredura por sessao. Prova com 2 sessoes
    # reais (uma delas com pane extra): list() paga UMA chamada tmux pra TODAS (list_panes_all) e
    # NUNCA o caminho por-sessao (list_panes_of, o que o agentpane.resolve_target usaria).
    outro_cwd = tmp_path / "outra"
    outro_cwd.mkdir()
    outra_sessao = f"{SESS}-2"
    subprocess.run(["tmux", "kill-session", "-t", f"={outra_sessao}"], capture_output=True)
    subprocess.run(["tmux", "new-session", "-d", "-s", outra_sessao, "-c", str(outro_cwd),
                    "-x", "200", "-y", "50", "sleep 600"], check=True)
    try:
        _segunda_janela(sessao, str(tmp_path))

        chamadas_of = []
        monkeypatch.setattr(tmux, "list_panes_of", lambda n: chamadas_of.append(n))

        chamadas_all = {"n": 0}
        original = tmux.list_panes_all

        def _contando():
            chamadas_all["n"] += 1
            return original()

        monkeypatch.setattr(tmux, "list_panes_all", _contando)

        reg = registry.SessionRegistry(projects_dir=tmp_path)
        reg.list()

        assert chamadas_all["n"] == 1          # UMA chamada tmux pra resolver TODAS as sessoes
        assert chamadas_of == []                # nenhuma chamada por-sessao durante list()
    finally:
        subprocess.run(["tmux", "kill-session", "-t", f"={outra_sessao}"], capture_output=True)
