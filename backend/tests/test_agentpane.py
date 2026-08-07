import shutil
import subprocess
import uuid
from unittest.mock import patch

import pytest
from app import agentpane, tmux

# L12 da revisao final: estes testes dirigem um tmux DE VERDADE. Sem o skipif eles quebravam a
# suite em maquina sem tmux; com nome FIXO, duas copias sob xdist criavam/matavam a MESMA sessao e
# uma derrubava a outra; e sem socket proprio tudo isso acontecia no servidor tmux do usuario, ao
# lado das sessoes de trabalho dele. Mesmo remedio do test_tmux.py (has_session contra tmux real).
pytestmark = pytest.mark.skipif(shutil.which("tmux") is None,
                                reason="tmux nao instalado no ambiente")


@pytest.fixture
def sessao():
    sock = f"cp-test-{uuid.uuid4().hex[:8]}"
    nome = f"cp-agentpane-{uuid.uuid4().hex[:6]}"

    def tmux_no_socket(args, **_kw):
        # Injeta `-L <socket>` logo DEPOIS do "tmux" do argv -- nao em args[1:] como o precedente:
        # `tmux.new_session` (usado num dos testes) prefixa `systemd-run --user --scope`, entao o
        # "tmux" nem sempre e o primeiro elemento. Argv sem "tmux" nenhum (o probe de scope, que
        # roda `systemd-run ... true`) passa direto.
        if "tmux" not in args:
            return subprocess.run(args, capture_output=True, text=True, errors="replace")
        i = args.index("tmux")
        return subprocess.run([*args[:i + 1], "-L", sock, *args[i + 1:]],
                              capture_output=True, text=True, errors="replace")

    # Sessao "ancora": segura o servidor tmux DESTE socket de pe. Sem ela, o teste que mata e
    # recria a sessao derruba a ultima sessao -> o servidor sai -> o proximo `new-session` sobe um
    # servidor NOVO, com a numeracao de `%N` zerada, e "o pane novo tem id diferente do velho"
    # deixa de valer por um motivo que nao e o do teste. No servidor compartilhado do usuario as
    # sessoes dele faziam esse papel por acaso.
    ancora = f"cp-ancora-{uuid.uuid4().hex[:6]}"
    subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", ancora, "sleep 600"],
                   check=True, capture_output=True)
    subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", nome, "-x", "200", "-y", "50",
                    "sleep 600"], check=True, capture_output=True)
    try:
        with patch.object(tmux, "RUN", tmux_no_socket):
            agentpane.invalidate(nome)
            yield nome
    finally:
        agentpane.invalidate(nome)
        # kill-SESSION, nunca kill-server: um `-L` esquecido num kill-server derruba o servidor
        # tmux DEFAULT e com ele todas as sessoes do usuario (mesma nota do test_tmux.py). Matar a
        # ultima sessao ja encerra este servidor sozinho.
        for alvo in (nome, f"term-{nome}", ancora):
            subprocess.run(["tmux", "-L", sock, "kill-session", "-t", f"={alvo}"],
                           capture_output=True)


def _segunda_janela(nome):
    # Via `tmux._run` (patchado pelo fixture) pra cair no MESMO socket privado -- um subprocess
    # cru aqui criaria a janela no servidor tmux do usuario, onde a sessao nem existe.
    assert tmux._run(["tmux", "new-window", "-t", f"={nome}", "sleep 600"]).returncode == 0


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


def test_desempate_com_dois_panes_de_agente_prefere_o_ativo(sessao, monkeypatch):
    # I2 da revisao final: com 2+ panes de agente na mesma sessao, o agentpane pegava o PRIMEIRO da
    # varredura e o registry._agent_pane o ATIVO -- o /input resolvia provider e pane_id por um
    # pane e digitava no outro (e o cache de 60s daqui esticava a discordancia).
    from app.registry import SessionRegistry
    monkeypatch.setattr(agentpane, "_pane_do_agente", lambda pid, children: True)
    _segunda_janela(sessao)      # a janela nova nasce ATIVA
    agentpane.invalidate(sessao)

    ativo = next(p for p in tmux.list_panes_of(sessao) if p["active"])
    assert agentpane.resolve_target(sessao) == ativo["pane_id"]
    # E o outro lado escolhe o MESMO pane -- e disso que o bug dependia.
    grupo = tmux.list_panes_all()[sessao]
    assert SessionRegistry._agent_pane(grupo, {})["pane_id"] == ativo["pane_id"]


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
