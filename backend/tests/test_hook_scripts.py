import json
import os
import subprocess
import sys
from pathlib import Path

HOOK_SCRIPT = Path(__file__).parent.parent / "hooks" / "askq_capture.py"


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
    out = tmp_path / ".hangar-askq" / "abc.json"
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
    assert not (tmp_path / ".hangar-askq").exists()  # nada escrito


NAV_HOOK = Path(__file__).parent.parent / "hooks" / "nav_hook.py"


def _roda_nav_hook(home: Path, pane: str | None = None,
                   prompt: str = "abre a página de login no localhost") -> str:
    # Sem navegador aberto, a dica só vai em mensagem de tela; sem chave do Jev, a regex decide.
    env = {**os.environ, "HOME": str(home)}
    env.pop("TMUX_PANE", None)
    env.pop("TYPESAFE_API_KEY", None)
    if pane:
        env["TMUX_PANE"] = pane
    r = subprocess.run([sys.executable, str(NAV_HOOK)], input=json.dumps({"prompt": prompt}), text=True,
                       capture_output=True, env=env)
    assert r.returncode == 0
    return r.stdout


def test_nav_hook_fica_calado_sem_electron(tmp_path):
    nav = tmp_path / ".hangar" / "nav"
    nav.mkdir(parents=True)
    assert _roda_nav_hook(tmp_path) == ""                       # sem _srv.json
    # 2**22 e o pid_max do Linux 64-bit: nenhum processo pode ter este pid.
    (nav / "_srv.json").write_text(json.dumps({"porta": 1, "token": "x", "pid": 2**22 + 7}))
    assert _roda_nav_hook(tmp_path) == ""                       # pid morto: arquivo sobrou de crash


def test_nav_hook_avisa_como_abrir_e_a_url_quando_ja_aberto(tmp_path, monkeypatch):
    nav = tmp_path / ".hangar" / "nav"
    nav.mkdir(parents=True)
    (nav / "_srv.json").write_text(json.dumps({"porta": 1, "token": "x", "pid": os.getpid()}))
    saida = json.loads(_roda_nav_hook(tmp_path))
    ctx = saida["hookSpecificOutput"]["additionalContext"]
    assert saida["hookSpecificOutput"]["hookEventName"] == "UserPromptSubmit"
    assert "hangar-preview open <url>" in ctx
    # Com o sidecar da sessao, a URL entra no aviso (o nome vem do tmux; aqui o pane nao existe,
    # entao o hook nao acha a sessao e cai no aviso de "abra").
    (nav / "s--x.json").write_text(json.dumps({"chave": "srv-1::minha", "url": "http://localhost:3100/"}))
    assert "hangar-preview open <url>" in json.loads(_roda_nav_hook(tmp_path, pane="%999999"))["hookSpecificOutput"]["additionalContext"]

    from importlib import util as iu
    spec = iu.spec_from_file_location("nav_hook", NAV_HOOK)
    mod = iu.module_from_spec(spec)
    spec.loader.exec_module(mod)
    assert mod._url_do_navegador(str(nav), "minha") == "http://localhost:3100/"
    assert mod._url_do_navegador(str(nav), "outra") is None
    assert "http://localhost:3100/" in mod.texto("http://localhost:3100/")


def _run_hook(tmp_path, payload):
    return subprocess.run([sys.executable, str(HOOK_SCRIPT)], input=json.dumps(payload), text=True,
                          capture_output=True, env={**os.environ, "CLAUDE_CONFIG_DIR": str(tmp_path)})


def test_pergunta_encerrada_no_terminal_apaga_so_a_propria(tmp_path):
    # Respondida ou recusada direto no terminal, a pergunta não pode ficar aberta para a lista.
    pergunta = {"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion", "session_id": "abc",
                "tool_use_id": "t1", "tool_input": {"questions": []}}
    sidecar = tmp_path / ".hangar-askq" / "abc.json"
    for evento in ("PostToolUse", "PostToolUseFailure"):
        _run_hook(tmp_path, pergunta)
        # Fim de outra chamada (pergunta nova já gravou por cima) não apaga.
        _run_hook(tmp_path, {**pergunta, "hook_event_name": evento, "tool_use_id": "outra"})
        assert sidecar.exists()
        assert _run_hook(tmp_path, {**pergunta, "hook_event_name": evento}).returncode == 0
        assert not sidecar.exists(), evento
    # Sidecar de versão antiga do Claude Code, sem id: o fim desta chamada também o tira.
    sidecar.write_text(json.dumps({"tool_name": "AskUserQuestion", "session_id": "abc"}), encoding="utf-8")
    _run_hook(tmp_path, {**pergunta, "hook_event_name": "PostToolUse"})
    assert not sidecar.exists()
