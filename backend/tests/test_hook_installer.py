import json
import os
import subprocess
import sys
from pathlib import Path

from app import hook_installer
from app.config import ConfigDirInfo

HOOK_SCRIPT = Path(__file__).parent.parent / "hooks" / "askq_capture.py"


def _settings(d: Path) -> Path:
    return d / "settings.json"


def test_fresh_config_dir_gets_block(tmp_path):
    # Sem settings.json -> cria o bloco PreToolUse/AskUserQuestion do zero.
    changed = hook_installer._ensure_settings_file(_settings(tmp_path))
    assert changed is True
    data = json.loads(_settings(tmp_path).read_text(encoding="utf-8"))
    pre = data["hooks"]["PreToolUse"]
    assert len(pre) == 1
    assert pre[0]["matcher"] == "AskUserQuestion"
    assert pre[0]["hooks"][0]["command"] == hook_installer._COMMAND


def test_preserves_existing_hooks_and_keys(tmp_path):
    # O PONTO do teste: nao pode clobbar hook alheio (Bash) nem outras chaves (model).
    seed = {
        "hooks": {
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]}
            ]
        },
        "model": "x",
    }
    sp = _settings(tmp_path)
    sp.write_text(json.dumps(seed), encoding="utf-8")
    changed = hook_installer._ensure_settings_file(sp)
    assert changed is True
    data = json.loads(sp.read_text(encoding="utf-8"))
    assert data["model"] == "x"  # chave nao-hook preservada
    pre = data["hooks"]["PreToolUse"]
    matchers = [b["matcher"] for b in pre]
    assert "Bash" in matchers and "AskUserQuestion" in matchers
    bash = next(b for b in pre if b["matcher"] == "Bash")
    assert bash["hooks"][0]["command"] == "echo hi"  # bloco alheio intacto


def test_idempotent(tmp_path):
    sp = _settings(tmp_path)
    assert hook_installer._ensure_settings_file(sp) is True
    assert hook_installer._ensure_settings_file(sp) is False  # 2a vez: nada a fazer
    data = json.loads(sp.read_text(encoding="utf-8"))
    ours = [b for b in data["hooks"]["PreToolUse"] if b["matcher"] == "AskUserQuestion"]
    assert len(ours) == 1  # sem duplicar


def _ours(data, event, script):
    """Entradas NOSSAS (as que referenciam `script`) sob hooks[event]."""
    return [
        h
        for b in data.get("hooks", {}).get(event, [])
        for h in b.get("hooks", [])
        if hook_installer._refers_to(h.get("command"), script)
    ]


def test_old_format_entry_is_replaced_not_appended(tmp_path):
    # Regressao real: o command mudou de 'python3 X' pra '"venv/py" "X"'. A igualdade
    # exata nao reconhecia a entrada antiga e ACRESCENTAVA uma segunda.
    sp = _settings(tmp_path)
    old = f"python3 {hook_installer.HOOK}"
    sp.write_text(json.dumps({"hooks": {"PreToolUse": [
        {"matcher": "AskUserQuestion", "hooks": [{"type": "command", "command": old}]}
    ]}}), encoding="utf-8")
    assert hook_installer._ensure_settings_file(sp) is True
    data = json.loads(sp.read_text(encoding="utf-8"))
    ours = _ours(data, "PreToolUse", hook_installer.HOOK)
    assert len(ours) == 1
    assert ours[0]["command"] == hook_installer._COMMAND


def test_duplicated_old_and_new_collapse_to_one(tmp_path):
    # Estado ja materializado na maquina do usuario: as duas versoes convivendo.
    sp = _settings(tmp_path)
    old = f"python3 {hook_installer.HOOK}"
    sp.write_text(json.dumps({"hooks": {"PreToolUse": [
        {"matcher": "AskUserQuestion", "hooks": [{"type": "command", "command": old}]},
        {"matcher": "AskUserQuestion",
         "hooks": [{"type": "command", "command": hook_installer._COMMAND}]},
    ]}}), encoding="utf-8")
    assert hook_installer._ensure_settings_file(sp) is True
    data = json.loads(sp.read_text(encoding="utf-8"))
    ours = _ours(data, "PreToolUse", hook_installer.HOOK)
    assert len(ours) == 1
    assert ours[0]["command"] == hook_installer._COMMAND
    assert len(data["hooks"]["PreToolUse"]) == 1  # bloco que ficou vazio sumiu
    assert hook_installer._ensure_settings_file(sp) is False  # ja curado


def test_collapse_keeps_third_party_hooks_in_same_event(tmp_path):
    sp = _settings(tmp_path)
    old = f"python3 {hook_installer.HOOK}"
    sp.write_text(json.dumps({"hooks": {"PreToolUse": [
        {"hooks": [{"type": "command", "command": "/home/x/.claude/hooks/ecc-review.sh"}]},
        {"matcher": "AskUserQuestion", "hooks": [{"type": "command", "command": old}]},
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]},
        {"matcher": "AskUserQuestion",
         "hooks": [{"type": "command", "command": hook_installer._COMMAND}]},
    ]}}), encoding="utf-8")
    assert hook_installer._ensure_settings_file(sp) is True
    pre = json.loads(sp.read_text(encoding="utf-8"))["hooks"]["PreToolUse"]
    cmds = [h["command"] for b in pre for h in b["hooks"]]
    assert cmds == [
        "/home/x/.claude/hooks/ecc-review.sh",
        hook_installer._COMMAND,
        "rtk hook claude",
    ]  # ordem e conteudo dos terceiros intactos


def test_same_script_name_in_another_checkout_is_a_different_hook(tmp_path):
    # Outro checkout do repo = outro arquivo = outro hook. Nao pode ser adotado nem sumir.
    sp = _settings(tmp_path)
    alheio = "python3 /outro/checkout/backend/hooks/askq_capture.py"
    sp.write_text(json.dumps({"hooks": {"PreToolUse": [
        {"matcher": "AskUserQuestion", "hooks": [{"type": "command", "command": alheio}]}
    ]}}), encoding="utf-8")
    assert hook_installer._ensure_settings_file(sp) is True
    cmds = [h["command"] for b in json.loads(sp.read_text(encoding="utf-8"))["hooks"]["PreToolUse"]
            for h in b["hooks"]]
    assert cmds == [alheio, hook_installer._COMMAND]


def test_invalid_json_left_untouched(tmp_path):
    sp = _settings(tmp_path)
    sp.write_text("{not valid json", encoding="utf-8")
    changed = hook_installer._ensure_settings_file(sp)
    assert changed is False
    assert sp.read_text(encoding="utf-8") == "{not valid json"  # nao sobrescrito


def test_empty_file_gets_block(tmp_path):
    sp = _settings(tmp_path)
    sp.write_text("", encoding="utf-8")
    assert hook_installer._ensure_settings_file(sp) is True
    data = json.loads(sp.read_text(encoding="utf-8"))
    assert len(_ours(data, "PreToolUse", hook_installer.HOOK)) == 1


def test_ensure_installed_targets_config_dirs(tmp_path, monkeypatch):
    # Hermetico: monkeypatch das fontes de dirs pra tmp_path, nunca toca ~/.claude*.
    d1 = tmp_path / "c1"
    d1.mkdir()
    d2 = tmp_path / "c2"
    d2.mkdir()
    monkeypatch.setattr(
        hook_installer, "list_config_dirs",
        lambda: [ConfigDirInfo(path=str(d1), label="a", active=True)],
    )
    monkeypatch.setattr(hook_installer, "_backend_config_base", lambda: d2)
    touched = hook_installer.ensure_askq_hook_installed()
    assert len(touched) == 2
    for d in (d1, d2):
        data = json.loads((d / "settings.json").read_text(encoding="utf-8"))
        assert data["hooks"]["PreToolUse"][0]["matcher"] == "AskUserQuestion"


def test_ensure_installed_failsoft_when_discovery_raises(monkeypatch):
    # list_config_dirs estourando (ex: HOME ausente) NUNCA pode derrubar o startup -> retorna [].
    def _boom():
        raise RuntimeError("HOME nao setado")
    monkeypatch.setattr(hook_installer, "list_config_dirs", _boom)
    assert hook_installer.ensure_askq_hook_installed() == []


def test_capture_script_writes_sidecar(tmp_path):
    raw = json.dumps({
        "tool_name": "AskUserQuestion",
        "session_id": "abc",
        "tool_input": {"questions": [{"q": 1}]},
    })
    r = subprocess.run(
        [sys.executable, str(HOOK_SCRIPT)],
        input=raw, text=True, capture_output=True,
        env={**os.environ, "CLAUDE_CONFIG_DIR": str(tmp_path)},
    )
    assert r.returncode == 0
    out = tmp_path / ".claude-pocket-askq" / "abc.json"
    assert out.exists()
    assert out.read_text(encoding="utf-8") == raw  # grava o stdin cru


def test_capture_script_ignores_non_askq(tmp_path):
    raw = json.dumps({"tool_name": "Bash", "session_id": "abc", "tool_input": {}})
    r = subprocess.run(
        [sys.executable, str(HOOK_SCRIPT)],
        input=raw, text=True, capture_output=True,
        env={**os.environ, "CLAUDE_CONFIG_DIR": str(tmp_path)},
    )
    assert r.returncode == 0
    assert not (tmp_path / ".claude-pocket-askq").exists()  # nada escrito
