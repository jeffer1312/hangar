import os
import time
from unittest.mock import patch
from app import registry
from app.registry import (
    SessionRegistry,
    sanitize_cwd,
    _session_id_from_cmdline,
)

_UUID = "12345678-1234-1234-1234-123456789abc"


def test_sanitize_cwd_matches_claude_scheme():
    assert sanitize_cwd("/home/jeffer1312/Projetos/claude-pocket") == \
        "-home-jeffer1312-Projetos-claude-pocket"


# --- cmdline --session-id parsing (sinal DETERMINISTICO, funciona em idle) ---

def test_session_id_from_cmdline_flag():
    assert _session_id_from_cmdline(f"claude --session-id {_UUID}") == _UUID


def test_session_id_from_cmdline_equals():
    assert _session_id_from_cmdline(f"claude --session-id={_UUID}") == _UUID


def test_session_id_from_cmdline_resume():
    assert _session_id_from_cmdline(f"claude --resume {_UUID}") == _UUID


def test_session_id_from_cmdline_bare_is_none():
    assert _session_id_from_cmdline("claude") is None


def test_session_id_from_cmdline_resume_without_id_is_none():
    # `--resume` sozinho abre um picker, nao especifica sessao -> nao casar.
    assert _session_id_from_cmdline("claude --resume") is None


# --- prioridade de resolucao: cmdline VENCE o newest-by-mtime (mata a colisao) ---

def test_resolve_prefers_cmdline_session_id_over_mtime(tmp_path):
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    (proj / "stale.jsonl").write_text("{}")  # newest-by-mtime que NAO deve vencer
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "pane_pid", return_value=999), \
         patch.object(registry, "_descendant_pids", return_value=[999]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {_UUID}"):
        j = reg.resolve("cc", "/home/u/p")
    assert j.endswith(f"{_UUID}.jsonl")


def test_resolve_ignores_daemon_session_id(tmp_path):
    # O `claude daemon` (filho) tem um --session-id TRANSITORIO proprio; resolver por ele apontava pro
    # jsonl inexistente do daemon. Deve pular o daemon e cair no fallback (REPL bare = sem flag).
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    (proj / "real.jsonl").write_text("{}")
    reg = SessionRegistry(projects_dir=tmp_path)
    daemon = "deadbeef-0000-0000-0000-000000000000"

    def cmdline(p):
        return f"claude daemon run --session-id {daemon}" if p == 2 else "claude"

    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1, 2]), \
         patch.object(registry, "_cmdline", side_effect=cmdline), \
         patch.object(registry, "_open_jsonl", return_value=None):
        j = reg.resolve("cc", "/home/u/p")
    assert j.endswith("real.jsonl")  # daemon ignorado -> mtime, NAO o uuid do daemon


def test_resolve_picks_main_session_over_subagent(tmp_path):
    # A arvore do claude tem SUB-AGENTES (--agent), cada um com --session-id proprio. Deve pegar o id do
    # REPL principal (com --fork-session/--resume), nao o do sub-agent.
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    reg = SessionRegistry(projects_dir=tmp_path)
    main = "12345678-1234-1234-1234-123456789abc"
    sub = "deadbeef-0000-0000-0000-000000000000"

    def cmdline(p):
        if p == 2:
            return f"claude --session-id {sub} --agent claude"      # sub-agent -> pular
        if p == 3:
            return f"claude --session-id {main} --fork-session --resume /x/y.jsonl"  # REPL principal
        return "claude"

    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1, 2, 3]), \
         patch.object(registry, "_cmdline", side_effect=cmdline), \
         patch.object(registry, "_open_jsonl", return_value=None):
        j = reg.resolve("cc", "/home/u/p")
    assert j.endswith(f"{main}.jsonl")


def test_resolve_jsonl_picks_newest(tmp_path):
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    old = proj / "old.jsonl"
    old.write_text("{}")
    new = proj / "new.jsonl"
    new.write_text("{}")
    now = time.time()
    os.utime(old, (now - 100, now - 100))
    os.utime(new, (now, now))
    reg = SessionRegistry(projects_dir=tmp_path)
    assert reg.resolve_jsonl("/home/u/p").endswith("new.jsonl")


def test_list_maps_sessions_to_jsonl(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "list_sessions",
                      return_value=[{"name": "cc", "cwd": "/home/u/p"}]), \
         patch.object(reg, "resolve_jsonl", return_value="/x/s.jsonl"):
        out = reg.list()
    assert out[0].name == "cc" and out[0].jsonl == "/x/s.jsonl"


def test_resolve_jsonl_returns_none_when_dir_empty(tmp_path):
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    reg = SessionRegistry(projects_dir=tmp_path)
    assert reg.resolve_jsonl("/home/u/p") is None
