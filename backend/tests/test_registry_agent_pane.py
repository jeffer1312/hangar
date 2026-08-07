"""Task 5.5: registry.list() resolve pelo pane do AGENTE, nao so pelo pane ATIVO.

Mesmo padrao do test_agentpane.py: tmux REAL, fixture que mata a sessao no finally (inclusive
quando o teste falha), e o predicado _pane_do_agente monkeypatchado (nao depende de um `claude` de
verdade rodando).
"""
import logging
import subprocess
import uuid
import pytest
import app.api as api_mod
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
    # split-window (NAO new-window): medido que #{pane_active} e por JANELA -- com 2 JANELAS de 1
    # pane cada, as DUAS saem active=1 (o tmux nunca desmarca a janela 0) e o `list-panes -a` lista a
    # janela 0 primeiro, entao o pane do agente ganhava por acidente de ordenacao mesmo SEM a
    # correcao desta task (achado C1 da revisao: o teste passava com o codigo antigo). split-window
    # cria um 2o PANE na MESMA janela (:0) e so ai o tmux desmarca active no pane original -- o
    # cenario que reproduz o bug de verdade.
    subprocess.run(["tmux", "split-window", "-t", f"={nome}:0", "-c", cwd, "sleep 600"], check=True)


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


def test_list_nao_faz_fork_por_sessao(tmp_path, monkeypatch):
    # Brief item 4: a correcao NAO pode acrescentar fork/varredura por sessao. Prova com 2 sessoes
    # reais (uma delas com pane extra, via split -- o cenario que reproduz de verdade, achado C1).
    #
    # Achado menor 3 da revisao: espiar so list_panes_all/list_panes_of nao mede o que o nome do
    # teste promete -- _cwd_has_siblings (chamado de dentro de resolve_tracked) TAMBEM e um
    # `tmux list-panes -a`, que pode rodar por sessao. Espiar tmux.RUN (o ponto baixo por onde TODO
    # fork do modulo passa, mesmo padrao de test_tmux.py) mede o fork de verdade.
    #
    # Socket proprio (-L): sem isto o `reg.list()` enxerga TODAS as sessoes tmux da maquina (as reais
    # do dev, nao so as deste teste) -- qualquer uma delas com --session-id no cmdline dispara
    # _cwd_has_siblings por conta propria e o total de forks passa a depender do que mais esta
    # rodando na maquina, nao do que o teste criou (medido: contaminava o teste com forks alheios).
    sock = f"cp-test-{uuid.uuid4().hex[:8]}"
    a, b = f"cp-test-fork-a-{uuid.uuid4().hex[:6]}", f"cp-test-fork-b-{uuid.uuid4().hex[:6]}"
    cwd_a, cwd_b = tmp_path / "a", tmp_path / "b"
    cwd_a.mkdir()
    cwd_b.mkdir()
    try:
        subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", a, "-c", str(cwd_a),
                        "-x", "200", "-y", "50", "sleep 600"], check=True)
        subprocess.run(["tmux", "-L", sock, "split-window", "-t", f"={a}:0", "-c", str(cwd_a),
                        "sleep 600"], check=True)
        subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", b, "-c", str(cwd_b),
                        "-x", "200", "-y", "50", "sleep 600"], check=True)

        chamadas_of = []
        monkeypatch.setattr(tmux, "list_panes_of", lambda n: chamadas_of.append(n))

        chamadas_run = []

        def _espiao(args, **kw):
            chamadas_run.append(list(args))
            return subprocess.run(["tmux", "-L", sock, *args[1:]], **kw)

        monkeypatch.setattr(tmux, "RUN", _espiao)

        reg = registry.SessionRegistry(projects_dir=tmp_path)
        reg.list()

        # As duas sessoes sao `sleep` puro (sem --session-id no cmdline) -> resolve_tracked nunca
        # chega no passo do cmdline e _cwd_has_siblings nunca dispara; o UNICO fork esperado e o
        # `list-panes -a` de list(). Se a correcao tivesse introduzido fork por sessao, o total
        # cresceria com o numero de sessoes/panes (aqui: 2 sessoes, 3 panes).
        assert chamadas_run == [["tmux", "list-panes", "-a", "-F",
                                 "#{session_name}\t#{pane_active}\t#{pane_pid}\t#{pane_current_path}\t#{pane_id}"
                                 "\t#{@cp_hidden}"]]
        assert chamadas_of == []                # nenhuma chamada por-sessao durante list()
    finally:
        # kill-SESSION (alvo exato), nunca kill-server -- mesma proibicao do test_tmux.py: um `-L`
        # esquecido num kill-server derruba o servidor tmux DEFAULT (todas as sessoes Claude vivas
        # do usuario), nao so o socket privado deste teste. Aqui o `sock` e sempre um uuid novo e
        # nunca ficaria vazio de verdade, mas o padrao errado nao pode ser o que fica pra copiar.
        subprocess.run(["tmux", "-L", sock, "kill-session", "-t", f"={a}"], capture_output=True)
        subprocess.run(["tmux", "-L", sock, "kill-session", "-t", f"={b}"], capture_output=True)


def test_pane_info_resolve_pelo_pane_do_agente_com_split(sessao, tmp_path, monkeypatch):
    # Task 6, Step 6: com um split (o shell escondido, ou qualquer split manual), o pane ATIVO pode
    # ser o do shell -- api._pane_info tem que devolver o pane do AGENTE (aqui, um Pi), nao o
    # ativo, reusando a MESMA resolucao que registry.list() ja usa (_agent_pane).
    agente = tmux.list_panes_of(sessao)[0]
    _segunda_janela(sessao, str(tmp_path))     # 2o pane, fica ATIVO -- o agente perde o "ativo"

    monkeypatch.setattr(agentpane, "_pane_do_agente", lambda pid, children: pid == agente["pid"])
    monkeypatch.setattr(registry, "_cmdline",
                        lambda pid: "pi --whatever" if pid == agente["pid"] else "bash")

    provider, pane_id = api_mod._pane_info(sessao)

    assert provider == "pi"
    assert pane_id == agente["pane_id"]
