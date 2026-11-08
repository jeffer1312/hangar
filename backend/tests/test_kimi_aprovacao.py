"""Pedido de aprovacao do Kimi (plano/comando/arquivo) lido do WIRE, nao do pane.

As linhas usadas aqui sao o formato MEDIDO nos wire.jsonl reais desta maquina (Kimi 0.34.0) — e a
mesma disciplina do test_kimi_transcript.py: copiar o shape do wire, nunca o que a doc sugere.
"""
import json
from unittest.mock import patch

import pytest
from fastapi.testclient import TestClient

from app import state, terminal_input as ti
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


def test_select_no_kimi_opcao_fora_da_lista_nao_digita_nada(cliente, tmp_path):
    wire = _write(tmp_path, [_req("approval_1")])   # 3 escolhas
    with patch("app.api.registry.list", return_value=_sessao_kimi(wire)), \
         patch("app.api.terminal_input.select_kimi") as sel:
        r = cliente.post("/api/sessions/k1/select", json={"option": 4}, headers=_h())
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_opcao_fora_da_lista"
    sel.assert_not_called()
