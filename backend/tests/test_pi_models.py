import json
from pathlib import Path
from unittest.mock import call, patch

import pytest

from app import pi_models as pm
from app import terminal_input
from app.terminal_input import DriveError, TerminalInput

FIX = Path(__file__).parent / "fixtures"
# Sidecar REAL capturado de uma sessao Pi descartavel (pi 0.82.1), so com o catalogo podado —
# nao ha conteudo de conversa nenhum aqui, e o repositorio e publico.
SIDECAR = json.loads((FIX / "pi_models_sidecar.json").read_text())


def _write_sidecar(tmp_path: Path, data, jsonl="/x/2026-07-28T01-55-09-315Z_abc.jsonl") -> str:
    p = pm.sidecar_path(jsonl, tmp_path)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(data if isinstance(data, str) else json.dumps(data), encoding="utf-8")
    return jsonl


# ── sidecar ──────────────────────────────────────────────────────────────────
def test_sidecar_path_keyed_by_jsonl_stem(tmp_path):
    p = pm.sidecar_path("/home/u/.pi/agent/sessions/2026-07-28T01-55-09-315Z_abc.jsonl", tmp_path)
    assert p == tmp_path / ".claude-pocket-pi" / "models" / "2026-07-28T01-55-09-315Z_abc.json"


def test_read_catalog_real_sidecar(tmp_path):
    jsonl = _write_sidecar(tmp_path, SIDECAR)
    cat = pm.read_catalog(jsonl, tmp_path)
    assert cat["current"] == {"provider": "kimi-coding", "id": "k3", "name": "Kimi K3"}
    assert cat["thinking"] == "low"
    # Niveis do k3 medidos ao vivo: NAO sao os 7 canonicos — por isso vem da sessao, nao de constante.
    assert cat["levels"] == ["low", "high", "max"]
    assert {m["id"] for m in cat["models"]} >= {"k3", "cline-pass/glm-5.2"}


@pytest.mark.parametrize("bad", ["nao e json", '{"models": "nao e lista"}', "[]"])
def test_read_catalog_rejects_garbage(tmp_path, bad):
    jsonl = _write_sidecar(tmp_path, bad)
    assert pm.read_catalog(jsonl, tmp_path) is None


def test_read_catalog_missing_file_is_none(tmp_path):
    assert pm.read_catalog("/x/nunca-existiu.jsonl", tmp_path) is None


# ── read-back: o pedido PEGOU? ───────────────────────────────────────────────
_AFTER = {"current": {"provider": "clinepass", "id": "cline-pass/glm-5.2", "name": "GLM"},
          "thinking": "high", "levels": ["off", "low", "medium", "high"], "models": [], "ts": 2.0}


def test_confirms_modelo_exige_igualdade():
    assert pm.confirms(_AFTER, "clinepass", "cline-pass/glm-5.2", None) is True
    # O `/cp-model` recusa sem levantar nada (sem chave pro provedor): o sidecar fica no modelo
    # VELHO e o app declarava sucesso em cima do no-op.
    assert pm.confirms(_AFTER, "kimi-coding", "k3", None) is False
    assert pm.confirms({**_AFTER, "current": None}, "clinepass", "cline-pass/glm-5.2", None) is False


def test_confirms_nivel_aceita_o_clamp_mas_nao_o_ignorado():
    # xhigh NAO esta nos levels do modelo -> o Pi clampa e cair em `high` e o certo.
    assert pm.confirms(_AFTER, None, None, "xhigh") is True
    # medium ESTA nos levels: se nao ficou medium, nao aplicou.
    assert pm.confirms(_AFTER, None, None, "medium") is False
    assert pm.confirms(_AFTER, None, None, "high") is True


def test_read_back_sonda_ate_confirmar(tmp_path):
    # Settle fixo curto lia o catalogo ANTERIOR e reportava "trocou" sobre dado velho.
    before = {"current": {"provider": "kimi-coding", "id": "k3"}, "models": [], "ts": 1.0}
    with patch.object(pm, "read_catalog", side_effect=[before, before, _AFTER]), \
         patch.object(pm.time, "sleep", lambda *_: None):
        got = pm.read_back("/x/a.jsonl", tmp_path, "clinepass", "cline-pass/glm-5.2", None,
                           deadline=5.0)
    assert got is _AFTER


def test_read_back_devolve_a_ultima_leitura_no_prazo(tmp_path):
    before = {"current": {"provider": "kimi-coding", "id": "k3"}, "models": [], "ts": 1.0}
    with patch.object(pm, "read_catalog", return_value=before), \
         patch.object(pm.time, "sleep", lambda *_: None):
        got = pm.read_back("/x/a.jsonl", tmp_path, "clinepass", "cline-pass/glm-5.2", None,
                           deadline=0.0)
    assert got is before   # quem chama e que decide entre "recusou" e "nao deu pra confirmar"


# ── comandos que vao virar tecla ─────────────────────────────────────────────
def test_model_command_uses_space_not_slash():
    # O id ja contem barra: "clinepass/cline-pass/glm-5.2" seria ambiguo do outro lado.
    assert pm.model_command("clinepass", "cline-pass/glm-5.2") == "/cp-model clinepass cline-pass/glm-5.2"


@pytest.mark.parametrize("provider,model", [
    ("clinepass", "com espaco"), ("com espaco", "k3"), ("clinepass", "k3\nEnter"), ("", "k3"),
])
def test_model_command_rejects_untypeable(provider, model):
    with pytest.raises(pm.PiModelError) as e:
        pm.model_command(provider, model)
    assert e.value.status == 422


def test_think_command_normalizes_and_rejects():
    assert pm.think_command(" XHigh ") == "/cp-think xhigh"
    with pytest.raises(pm.PiModelError):
        pm.think_command("ultracode")   # nivel do Claude, nao do Pi


def test_check_known(tmp_path):
    pm.check_known(SIDECAR, "kimi-coding", "k3")            # nao levanta
    with pytest.raises(pm.PiModelError):
        pm.check_known(SIDECAR, "kimi-coding", "k9")
    with pytest.raises(pm.PiModelError):
        pm.check_known(SIDECAR, "outro-provedor", "k3")     # id certo, provedor errado


# ── sequencia de teclas ──────────────────────────────────────────────────────
def test_send_pi_commands_key_sequence():
    ti = TerminalInput()
    with patch.object(terminal_input, "deliverable", return_value=True), \
         patch.object(terminal_input, "_wait_input_ready", return_value=True), \
         patch("app.terminal_input.send_keys") as sk, \
         patch("time.sleep"):
        ti.send_pi_commands("s1", ["/cp-model kimi-coding k3", "/cp-think low"])
    # literal=True no comando (senao o tmux leria "Enter"/"Space" como nome de tecla) e UM Enter
    # por comando — os nossos comandos nao tem completion de argumento pra engolir o primeiro.
    assert sk.call_args_list == [
        call("s1", "/cp-model kimi-coding k3", literal=True),
        call("s1", "Enter"),
        call("s1", "/cp-think low", literal=True),
        call("s1", "Enter"),
    ]


def test_send_pi_commands_refuses_when_overlay_open():
    ti = TerminalInput()
    with patch.object(terminal_input, "deliverable", return_value=False), \
         patch("app.terminal_input.send_keys") as sk:
        with pytest.raises(DriveError):
            ti.send_pi_commands("s1", ["/cp-think low"])
    sk.assert_not_called()   # nada foi digitado as cegas num menu aberto
