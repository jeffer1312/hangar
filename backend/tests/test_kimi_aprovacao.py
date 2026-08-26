"""Pedido de aprovacao do Kimi (plano/comando/arquivo) lido do WIRE, nao do pane.

As linhas usadas aqui sao o formato MEDIDO nos wire.jsonl reais desta maquina (Kimi 0.34.0) — e a
mesma disciplina do test_kimi_transcript.py: copiar o shape do wire, nunca o que a doc sugere.
"""
import json
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app import state, terminal_input as ti
from app.adapters.kimi import transcript as kt
from app.config import settings
from app.models import SessionInfo


def _req(rid, kind="approval", tool="ExitPlanMode", display=None, action="", extra=None):
    req = {"turnId": 8, "toolCallId": "tool_x", "toolName": tool, "action": action,
           "display": {"kind": "plan_review", "plan": "# plano"} if display is None else display}
    if extra:
        req.update(extra)
    return {"type": "interaction.request", "agentId": "main", "id": rid, "kind": kind,
            "toolCallId": "tool_x", "request": req, "time": 1786454000000}


def _resolved(rid, resposta=None):
    return {"type": "interaction.resolved", "id": rid,
            "response": resposta if resposta is not None else {"decision": "approved",
                                                               "selectedLabel": "Approve"},
            "time": 1786454010000}


def _write(tmp_path, lines):
    p = tmp_path / "wire.jsonl"
    p.write_text("\n".join(json.dumps(l) for l in lines) + "\n", encoding="utf-8")
    return str(p)


# Copiado de um painel REAL (Kimi 0.34.0, sessao desta maquina em 26/08/2026). O `▶` do cursor e o
# motivo de o classify nunca ter visto este menu: ele so conhece `❯` (Claude) e `>` (Pi).
_PANE_APROVACAO = (
    "  ▶ Ready to build with this plan?\n"
    "\n"
    "  ▶ 1. Approve\n"
    "    2. Reject\n"
    "    3. Revise\n"
    "\n"
    "  ↑/↓ select · 1/2/3 choose · ↵ confirm\n"
)


# ── Leitura do wire ──────────────────────────────────────────────────────────

def test_plano_vira_approve_reject_revise(tmp_path):
    # O que o usuario ve no terminal: "Ready to build with this plan? 1. Approve 2. Reject 3. Revise".
    pend = kt.read_pending_interaction(_write(tmp_path, [_req("approval_1")]))
    assert pend is not None
    assert [e["label"] for e in pend["escolhas"]] == ["Approve", "Reject", "Revise"]
    assert [e["requires_feedback"] for e in pend["escolhas"]] == [False, False, True]
    assert pend["titulo"] == "Ready to build with this plan?"
    # Sem resumo: o plano inteiro ja esta no chat como tool_use do ExitPlanMode.
    assert pend["resumo"] == ""


def test_resolvido_deixa_de_ser_pendente(tmp_path):
    wire = _write(tmp_path, [_req("approval_1"), _resolved("approval_1")])
    assert kt.read_pending_interaction(wire) is None
    assert kt.interacao_resolvida(wire, "approval_1") is True


def test_resolucao_de_outro_pedido_nao_conta(tmp_path):
    # Pareamento e por `id`. Sem ele, a resposta de um pedido anterior apagaria o pedido de agora.
    wire = _write(tmp_path, [_resolved("approval_0"), _req("approval_1")])
    assert kt.read_pending_interaction(wire) is not None
    assert kt.interacao_resolvida(wire, "approval_1") is False


def test_pergunta_do_askuserquestion_nao_vira_botao(tmp_path):
    # `kind: "question"` ja chega ao app pelo tool.call (read_pending_call) e abre o stepper nativo.
    # Emitir aqui tambem faria a mesma pergunta aparecer duas vezes, em dois desenhos.
    wire = _write(tmp_path, [_req("question_1", kind="question", tool="",
                                  display={"kind": "generic"})])
    assert kt.read_pending_interaction(wire) is None


def test_comando_usa_as_quatro_escolhas_padrao(tmp_path):
    display = {"kind": "command", "command": "rm -rf build", "cwd": "/x", "language": "bash"}
    pend = kt.read_pending_interaction(
        _write(tmp_path, [_req("approval_2", tool="Bash", display=display, action="Running: rm")]))
    assert [e["label"] for e in pend["escolhas"]] == [
        "Approve once", "Approve for this session", "Reject", "Reject with feedback"]
    assert pend["escolhas"][3]["requires_feedback"] is True
    assert pend["titulo"] == "Run this command?"
    # Crase porque o OptionButtons transforma o trecho entre crases em <code>.
    assert pend["resumo"] == "`rm -rf build`"


def test_plano_com_opcoes_proprias_mantem_os_rotulos_do_modelo(tmp_path):
    display = {"kind": "plan_review", "plan": "# p",
               "options": [{"label": "Caminho A"}, {"label": "Caminho B"}]}
    pend = kt.read_pending_interaction(_write(tmp_path, [_req("approval_3", display=display)]))
    assert [e["label"] for e in pend["escolhas"]] == ["Caminho A", "Caminho B", "Reject", "Revise"]


def test_plano_com_uma_opcao_so_cai_no_approve_generico(tmp_path):
    # O `>= 2` e do proprio TUI: com uma opcao so ele ignora a lista e desenha "Approve".
    display = {"kind": "plan_review", "plan": "# p", "options": [{"label": "Unica"}]}
    pend = kt.read_pending_interaction(_write(tmp_path, [_req("approval_4", display=display)]))
    assert [e["label"] for e in pend["escolhas"]] == ["Approve", "Reject", "Revise"]


def test_goal_start_muda_as_escolhas_conforme_o_modo(tmp_path):
    yolo = kt.read_pending_interaction(_write(tmp_path, [
        _req("approval_5", tool="Goal", display={"kind": "goal_start", "mode": "yolo"})]))
    assert [e["label"] for e in yolo["escolhas"]] == [
        "Switch to Auto and start", "Keep YOLO and start", "Do not start"]
    manual = kt.read_pending_interaction(_write(tmp_path, [
        _req("approval_6", tool="Goal", display={"kind": "goal_start", "mode": "manual"})]))
    assert len(manual["escolhas"]) == 4


def test_pedido_sem_display_nao_oferece_botao(tmp_path):
    # Sem display nao da pra saber o que o painel desenhou; chutar as 4 do padrao mandaria a tecla
    # `2` pra uma opcao que talvez nem exista.
    linha = _req("approval_7")
    del linha["request"]["display"]
    assert kt.read_pending_interaction(_write(tmp_path, [linha])) is None


def test_wire_ilegivel_e_ausencia_de_dado_nao_confirmacao(tmp_path):
    ausente = str(tmp_path / "nao-existe.jsonl")
    assert kt.read_pending_interaction(ausente) is None
    # A prova de entrega tem que dizer NAO: "nao deu pra ler" nunca pode virar "chegou".
    assert kt.interacao_resolvida(ausente, "approval_1") is False


def test_linha_corrompida_nao_derruba_a_leitura(tmp_path):
    # O tailer le enquanto o Kimi escreve: linha pela metade e rotina.
    p = tmp_path / "wire.jsonl"
    p.write_text('{"type":"interaction.req\n' + json.dumps(_req("approval_8")) + "\n",
                 encoding="utf-8")
    assert kt.read_pending_interaction(str(p)) is not None


# ── state.aprovacao_kimi: o que vira pergunta + botoes na tela ───────────────

def test_aprovacao_kimi_devolve_pergunta_e_opcoes(tmp_path):
    wire = _write(tmp_path, [_req("approval_1")])
    assert state.aprovacao_kimi(wire, lambda: _PANE_APROVACAO) == (
        "Ready to build with this plan?", ["Approve", "Reject", "Revise"])


def test_aprovacao_kimi_junta_o_resumo_na_pergunta(tmp_path):
    wire = _write(tmp_path, [_req("approval_2", tool="Bash",
                                  display={"kind": "command", "command": "ls"})])
    pergunta, opcoes = state.aprovacao_kimi(wire, lambda: _PANE_APROVACAO)
    assert pergunta == "Run this command?\n`ls`"
    assert len(opcoes) == 4


def test_aprovacao_kimi_sem_transcript_e_none():
    assert state.aprovacao_kimi(None, lambda: _PANE_APROVACAO) is None


def test_pedido_orfao_de_execucao_anterior_nao_vira_botao(tmp_path):
    # Retomar uma sessao (`kimi -S`) reabre o MESMO wire.jsonl: um `interaction.request` que morreu
    # sem `interaction.resolved` continua la pra sempre. Sem a prova de TELA, a sessao nova nascia
    # mostrando botoes de um painel inexistente — e nenhum toque resolveria (o select_kimi recusa).
    wire = _write(tmp_path, [_req("approval_1")])
    assert state.aprovacao_kimi(wire, lambda: "composer vazio, sem painel\n") is None


def test_o_pane_so_e_capturado_quando_o_wire_tem_pedido(tmp_path):
    # A lista roda pra toda sessao a cada poll: capturar antes de perguntar seria a tempestade de
    # forks que o fast-path de marcador existe pra evitar.
    wire = _write(tmp_path, [_req("approval_1"), _resolved("approval_1")])
    chamadas = []

    def pane():
        chamadas.append(1)
        return _PANE_APROVACAO

    assert state.aprovacao_kimi(wire, pane) is None
    assert chamadas == []


# ── Drive: tecla numerica, nunca contagem de linha ───────────────────────────

@pytest.fixture
def pane_e_teclas(monkeypatch):
    """Substitui a captura do pane e o send_keys; devolve (set_pane, teclas)."""
    estado = {"pane": ""}
    teclas: list[tuple] = []
    monkeypatch.setattr(ti, "_capture", lambda name: estado["pane"])
    monkeypatch.setattr(ti, "send_keys",
                        lambda name, k, literal=False: teclas.append((k, literal)))
    return estado, teclas


def test_select_kimi_manda_o_numero_e_nada_mais(pane_e_teclas):
    estado, teclas = pane_e_teclas
    estado["pane"] = _PANE_APROVACAO
    ti.select_kimi("s", 3)
    # UMA tecla: o painel faz `Number(printable)-1` e ja submete. Nada de (n-1)xDown + Enter — o
    # cursor do Kimi e `▶`, que o _cursor_row nao le, entao contar linha ali seria as cegas.
    assert teclas == [("3", True)]


def test_select_kimi_recusa_sem_o_painel_na_tela(pane_e_teclas):
    estado, teclas = pane_e_teclas
    estado["pane"] = "conversa qualquer, sem painel\n"
    with pytest.raises(ti.DriveError):
        ti.select_kimi("s", 1)
    # Numero solto no composer viraria texto na conversa: nada pode ter sido digitado.
    assert teclas == []


def test_select_kimi_recusa_opcao_de_dois_digitos(pane_e_teclas):
    estado, teclas = pane_e_teclas
    estado["pane"] = _PANE_APROVACAO
    with pytest.raises(ValueError):
        ti.select_kimi("s", 10)
    assert teclas == []


def test_painel_e_modo_feedback_sao_reconhecidos_no_pane(pane_e_teclas):
    estado, _ = pane_e_teclas
    estado["pane"] = _PANE_APROVACAO
    assert ti.aprovacao_kimi_aberta("s") is True
    assert ti.feedback_kimi_aberto("s") is False
    # Escolher "Revise" troca o rodape: o painel espera a justificativa em vez de resolver.
    estado["pane"] = _PANE_APROVACAO.replace("↑/↓ select · 1/2/3 choose · ↵ confirm",
                                             "Type feedback · ↵ submit.")
    assert ti.aprovacao_kimi_aberta("s") is True
    assert ti.feedback_kimi_aberto("s") is True


def test_picker_de_pergunta_e_painel_de_aprovacao_nao_se_confundem(pane_e_teclas):
    # Rodapes diferentes ("↵ choose" x "↵ confirm"): cada detector so enxerga o seu desenho.
    estado, _ = pane_e_teclas
    estado["pane"] = _PANE_APROVACAO
    assert ti.picker_kimi_aberto("s") is False
    estado["pane"] = "  1. Sim\n  2. Nao\n  1-2 · ↵ choose\n"
    assert ti.aprovacao_kimi_aberta("s") is False
    assert ti.picker_kimi_aberto("s") is True


# ── O estado que faz os botoes aparecerem (chat e lista) ────────────────────

# Pane de uma sessao Kimi com o painel de aprovacao aberto: o `▶` do cursor nao e lido pelo
# classify (ele so conhece `❯` e `>`), e a lua do spinner do turno em curso continua na tela — e
# por isso que ler o pane devolvia "trabalhando" com os botoes escondidos.
_PANE_KIMI_TRABALHANDO = "🌘 Thinking for 12s\n" + _PANE_APROVACAO


async def test_state_monitor_abre_os_botoes_a_partir_do_wire(monkeypatch, tmp_path):
    wire = _write(tmp_path, [_req("approval_1")])
    monkeypatch.setattr(state.tmux, "has_session", lambda name: True)
    monkeypatch.setattr(state.tmux, "capture_pane", lambda name: _PANE_KIMI_TRABALHANDO)
    mon = state.StateMonitor("k1", hook_grace=None, sid_get=lambda: None,
                             transcript_get=lambda: wire)
    async for ev in mon.stream():
        assert ev.state == "awaiting_input"
        assert ev.options == ["Approve", "Reject", "Revise"]
        assert ev.question == "Ready to build with this plan?"
        break


async def test_state_monitor_sem_aprovacao_segue_o_pane(monkeypatch, tmp_path):
    wire = _write(tmp_path, [_req("approval_1"), _resolved("approval_1")])
    monkeypatch.setattr(state.tmux, "has_session", lambda name: True)
    monkeypatch.setattr(state.tmux, "capture_pane", lambda name: "conversa\n")
    mon = state.StateMonitor("k1", hook_grace=None, sid_get=lambda: None,
                             transcript_get=lambda: wire)
    async for ev in mon.stream():
        assert ev.state == "idle"
        break


async def test_lista_mostra_aguardando_mesmo_com_o_turno_aberto(monkeypatch, tmp_path):
    # No painel aberto o turno segue ABERTO no wire, entao `corrige_ocioso_kimi` promoveria a sessao
    # pra "working" e ela sumiria da coluna de quem espera resposta.
    from app import registry as reg
    wire = _write(tmp_path, [_req("approval_1")])
    info = SessionInfo(name="k1", cwd="/p")
    info.provider = "kimi"
    info.jsonl = wire
    r = reg.SessionRegistry()
    monkeypatch.setattr(r, "list", lambda: [info])
    monkeypatch.setattr(reg, "_kimi_corrige_ocioso", lambda i, m: ("working", 1.0))
    monkeypatch.setattr(reg.tmux, "capture_pane", lambda name: _PANE_APROVACAO)
    infos = await r.list_with_state()
    assert infos[0].state == "awaiting_input"
    assert infos[0].options == ["Approve", "Reject", "Revise"]


async def test_lista_sem_pedido_no_wire_segue_o_marcador(monkeypatch, tmp_path):
    from app import registry as reg
    wire = _write(tmp_path, [_req("approval_1"), _resolved("approval_1")])
    info = SessionInfo(name="k1", cwd="/p")
    info.provider = "kimi"
    info.jsonl = wire
    r = reg.SessionRegistry()
    monkeypatch.setattr(r, "list", lambda: [info])
    monkeypatch.setattr(reg, "_kimi_corrige_ocioso", lambda i, m: ("working", 1.0))
    monkeypatch.setattr(reg.tmux, "capture_pane", lambda name: _PANE_APROVACAO)
    # Painel na TELA mas nada pendente no wire: nao ha o que perguntar, o marcador manda.
    infos = await r.list_with_state()
    assert infos[0].state == "working"


# ── Rota /select: a escolha volta pelo wire, nao pela tela ───────────────────

@pytest.fixture
def cliente(monkeypatch):
    settings.auth_token = "secret"
    import app.api as api_mod
    monkeypatch.setattr(api_mod, "_session_exists", lambda name: True)
    monkeypatch.setattr(api_mod, "_recusa_se_painel_aberto", lambda name: None)
    from app.api import app
    return TestClient(app)


def _h():
    return {"Authorization": "Bearer secret"}


def _sessao_kimi(wire):
    info = SessionInfo(name="k1", cwd="/p")
    info.provider = "kimi"
    info.jsonl = wire
    return [info]


def test_select_no_kimi_manda_a_tecla_e_confirma_pelo_wire(cliente, tmp_path):
    wire = _write(tmp_path, [_req("approval_1")])
    with patch("app.api.registry.list", return_value=_sessao_kimi(wire)), \
         patch("app.api.terminal_input.select_kimi") as sel, \
         patch("app.api.terminal.select") as generico, \
         patch("app.adapters.kimi.transcript.interacao_resolvida", return_value=True):
        r = cliente.post("/api/sessions/k1/select", json={"option": 1}, headers=_h())
    assert r.status_code == 200
    assert r.json() == {"ok": True, "feedback_pendente": False}
    sel.assert_called_once_with("k1", 1)
    # O caminho generico (contar linha + Down/Enter) NAO pode ser usado aqui: o cursor do Kimi e
    # `▶`, que o _cursor_row nao le, entao ele iria as cegas.
    generico.assert_not_called()


def test_select_no_kimi_com_revise_confirma_pelo_campo_de_texto(cliente, tmp_path):
    # "Revise" nao resolve nada na hora: o painel abre o campo de justificativa. Sem esta segunda
    # prova, uma tecla que PEGOU gastaria o prazo inteiro e voltaria erro.
    wire = _write(tmp_path, [_req("approval_1")])
    with patch("app.api.registry.list", return_value=_sessao_kimi(wire)), \
         patch("app.api.terminal_input.select_kimi"), \
         patch("app.adapters.kimi.transcript.interacao_resolvida", return_value=False), \
         patch("app.api.terminal_input.feedback_kimi_aberto", return_value=True):
        r = cliente.post("/api/sessions/k1/select", json={"option": 3}, headers=_h())
    assert r.status_code == 200
    assert r.json()["feedback_pendente"] is True


def test_select_no_kimi_sem_confirmacao_com_painel_aberto_e_409(cliente, tmp_path):
    wire = _write(tmp_path, [_req("approval_1")])
    with patch("app.api.registry.list", return_value=_sessao_kimi(wire)), \
         patch("app.api.terminal_input.select_kimi"), \
         patch("app.api._espera_escolha_kimi", return_value=False), \
         patch("app.api.terminal_input.aprovacao_kimi_aberta", return_value=True):
        r = cliente.post("/api/sessions/k1/select", json={"option": 1}, headers=_h())
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_opcao_nao_convergiu"


def test_select_no_kimi_opcao_fora_da_lista_nao_digita_nada(cliente, tmp_path):
    wire = _write(tmp_path, [_req("approval_1")])   # 3 escolhas
    with patch("app.api.registry.list", return_value=_sessao_kimi(wire)), \
         patch("app.api.terminal_input.select_kimi") as sel:
        r = cliente.post("/api/sessions/k1/select", json={"option": 4}, headers=_h())
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_opcao_fora_da_lista"
    sel.assert_not_called()


def test_select_no_kimi_sem_aprovacao_pendente_cai_no_caminho_generico(cliente, tmp_path):
    # Picker cru do Kimi (que nao e aprovacao) continua indo pelo drive de sempre.
    wire = _write(tmp_path, [_req("approval_1"), _resolved("approval_1")])
    with patch("app.api.registry.list", return_value=_sessao_kimi(wire)), \
         patch("app.api.terminal_input.select_kimi") as sel, \
         patch("app.api.terminal.select") as generico:
        r = cliente.post("/api/sessions/k1/select", json={"option": 2}, headers=_h())
    assert r.status_code == 200
    sel.assert_not_called()
    generico.assert_called_once_with("k1", 2)
