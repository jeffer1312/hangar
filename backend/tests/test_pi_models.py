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
