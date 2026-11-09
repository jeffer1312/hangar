import json
import os
import shutil
import subprocess
import time

import pytest
from fastapi.testclient import TestClient

from app import runtime_config as rc
from app.api import app
from app.config import settings


@pytest.fixture(autouse=True)
def _isolate(tmp_path, monkeypatch):
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path)
    yield


@pytest.fixture(autouse=True)
def _no_real_focus(monkeypatch):
    # Sem isto o reuso chamaria o hyprctl da maquina de quem roda e puxaria janela de verdade.
    from app import window_focus
    monkeypatch.setattr(window_focus, "focus_tree", lambda root, env: False)


@pytest.fixture
def client():
    """Mesmo arranjo de test_api.py: sem armar o token, toda rota devolve 401."""
    previous = settings.auth_token
    settings.auth_token = "secret"
    yield TestClient(app)
    settings.auth_token = previous


def _as_json(*items) -> str:
    return json.dumps(list(items))


# --- validação na gravação -------------------------------------------------------------------

def test_valid_list_is_accepted_and_empty_restores_default():
    value = _as_json(
        {"id": "terminal", "type": "internal", "action": "terminal"},
        {"id": "a1", "type": "send_text", "label": "Relatório", "text": "/relatorio-pm",
         "icon": "emoji:📋", "send_direct": True},
        {"id": "a2", "type": "shell", "label": "Editor", "command": "code .", "confirm": True},
    )
    rc.aplicar({"shortcuts": value})
    assert rc.get("shortcuts") == value
    rc.aplicar({}, remover={"shortcuts"})
    assert rc.get("shortcuts") == ""


def test_broken_json_is_rejected():
    with pytest.raises(ValueError, match="JSON invalido"):
        rc.aplicar({"shortcuts": "{nao é json"})


def test_unknown_type_is_rejected_naming_the_item():
    with pytest.raises(ValueError, match="item 2"):
        rc.aplicar({"shortcuts": _as_json(
            {"id": "terminal", "type": "internal", "action": "terminal"},
            {"id": "x", "type": "foguete"},
        )})


def test_internal_with_unknown_action_is_rejected():
    with pytest.raises(ValueError, match="action desconhecida"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "internal", "action": "jetpack"})})


def test_item_missing_required_field_is_rejected():
    with pytest.raises(ValueError, match="sem texto"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "send_text", "label": "Oi"})})
    with pytest.raises(ValueError, match="sem comando"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "shell", "label": "Oi", "command": " "})})
    with pytest.raises(ValueError, match="sem rotulo"):
        rc.aplicar({"shortcuts": _as_json({"id": "x", "type": "shell", "command": "true"})})
    with pytest.raises(ValueError, match="sem id"):
        rc.aplicar({"shortcuts": _as_json({"type": "internal", "action": "rodar"})})


def test_value_must_be_a_list():
    with pytest.raises(ValueError, match="esperado uma lista"):
        rc.aplicar({"shortcuts": json.dumps({"id": "x"})})


def _auth():
    return {"Authorization": "Bearer secret"}


@pytest.fixture
def private_tmux(monkeypatch):
    """Todo `tmux` do backend vai pra um socket proprio: nada encosta nas sessoes de quem roda."""
    if shutil.which("tmux") is None or os.name != "posix":
        pytest.skip("terminal de atalho precisa de tmux (POSIX)")
    from app import tmux
    from tmux_teste import matar_servidor, novo_socket
    sock = novo_socket()

    def run(args, **kw):
        if args and args[0] == "tmux":
            args = ["tmux", "-L", sock, *args[1:]]
        return subprocess.run(args, **kw)
    monkeypatch.setattr(tmux, "RUN", run)
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    yield sock
    matar_servidor(sock)


@pytest.fixture
def home(tmp_path, monkeypatch):
    # Todo teste No Hangar roda com a home falsa: o padrao e rodar na home e nao pode escrever na real.
    path = tmp_path / "casa"
    path.mkdir()
    monkeypatch.setenv("HOME", str(path))
    return path


def test_hangar_concurrent_clicks_start_one_copy(client, monkeypatch, tmp_path, private_tmux):
    from concurrent.futures import ThreadPoolExecutor
    from app import shortcut_terminals
    monkeypatch.setenv("SHELL", "/bin/sh")
    with ThreadPoolExecutor(4) as pool:
        results = list(pool.map(
            lambda i: shortcut_terminals.start_hangar("global:k1", str(tmp_path), "sleep 30", "RDP", {}, f"s{i}", True),
            range(4)))
    assert len({term["id"] for term, _ in results}) == 1
    assert sorted(reused for _, reused in results) == [False, True, True, True]


def test_windows_start_writes_wrapper_marks_hidden_first_and_reads_exit_file(monkeypatch, tmp_path):
    from app import shortcut_terminals, tmux
    calls = []
    monkeypatch.setattr(shortcut_terminals, "_IS_WINDOWS", True)
    monkeypatch.setattr(shortcut_terminals, "_windows_dir", lambda: tmp_path)
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: calls.append(args) or subprocess.CompletedProcess(args, 0, "", ""))
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    term = shortcut_terminals.start("s", str(tmp_path), "delphi-vm\nexit 3", "RDP", {}, ask=True)
    inner = (tmp_path / f"{term['id']}-cmd.cmd").read_bytes().decode("latin-1")
    outer = (tmp_path / f"{term['id']}.cmd").read_bytes().decode("latin-1")
    assert "delphi-vm" in inner and "exit 3" in inner
    assert "\r\r" not in inner and "\r\r" not in outer                  # sem CRLF duplicado
    assert 'cmd /d /c "' in outer and '>"' in outer and "echo %ERRORLEVEL%" in outer and "goto h" in outer
    assert not any(a == ";" for a in calls[0])                          # opcoes em chamadas separadas
    assert calls[1][-2:] == ["@cp_hidden", "1"]                          # escondida antes de tudo
    (tmp_path / f"{term['id']}.exit").write_text("3\n")
    assert shortcut_terminals._windows_status(term["id"]) == (False, 3)


@pytest.mark.parametrize("line,expected", [
    ("1\t\t\n", (True, None)),
    ("1\t0\t\n", (False, 0)),
    ("1\t3\t\n", (False, 3)),
    ("1\t\tHUP\n", (False, None)),
    ("0\t\t\n", (True, None)),
])
def test_dead_pane_without_exit_code_is_still_settling(monkeypatch, line, expected):
    # O tmux marca o pane morto ao ler o fim do pty e só recolhe o código no SIGCHLD seguinte.
    if os.name == "nt":
        pytest.skip("o psmux devolve o codigo por arquivo")
    from app import shortcut_terminals as st, tmux
    monkeypatch.setattr(tmux, "RUN", lambda args, **kw: subprocess.CompletedProcess(args, 0, line, ""))
    assert st.status("cp-short-s-abcdef") == expected


def test_windows_run_code_writes_powershell_script_with_bom_and_one_crlf(monkeypatch, tmp_path):
    from app import shortcut_terminals as st
    monkeypatch.setattr(st, "_windows_dir", lambda: tmp_path)
    launch = st._windows_command("abcdef", "Write-Host 'ação'\nexit 3", powershell=True)
    script = (tmp_path / "abcdef-cmd.ps1").read_bytes()
    outer = (tmp_path / "abcdef.cmd").read_bytes().decode("latin-1")
    assert script.startswith(b"\xef\xbb\xbf")
    assert b"\r\r\n" not in script and b"\r\n" in script
    assert "Write-Host 'ação'" in script.decode("utf-8-sig")
    assert "powershell.exe" in outer.lower() and "-File" in outer
    assert "abcdef-cmd.ps1" in outer and launch.endswith('abcdef.cmd"')
    assert "-cmd.ps1" in st._FILE_SUFFIXES


def test_windows_restart_reads_back_the_inner_command_with_crlf_intact(monkeypatch, tmp_path):
    from app import shortcut_terminals, tmux
    monkeypatch.setattr(shortcut_terminals, "_IS_WINDOWS", True)
    monkeypatch.setattr(shortcut_terminals, "_windows_dir", lambda: tmp_path)
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: subprocess.CompletedProcess(args, 0, "", ""))
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    term = shortcut_terminals.start("", str(tmp_path), "echo a\necho b", "X", {}, key="global:k")
    row = {"tmux": term["tmux"], "id": term["id"], "owner": "", "key": "global:k", "label": "X",
           "origin": "", "ask": True, "alive": False, "pid": None, "exit_code": 0, "created": 1, "seq": 1}
    seen = []
    monkeypatch.setattr(shortcut_terminals, "_rows", lambda: [row])
    monkeypatch.setattr(shortcut_terminals, "_option", lambda target, opt: str(tmp_path) if opt == "@cp_shortcut_cwd" else "pi")
    monkeypatch.setattr(shortcut_terminals, "start_hangar", lambda *a: seen.append(a) or (None, False))
    shortcut_terminals.restart_hangar(term["id"], {})
    assert seen[0][2] == "echo a\r\necho b"


def _fake_row(term, tmp_path):
    return {"tmux": term["tmux"], "id": term["id"], "owner": "", "key": "global:k", "label": "X",
            "origin": "", "ask": True, "alive": False, "pid": None, "exit_code": 0, "created": 1, "seq": 1}


def test_windows_restart_without_the_cmd_file_fails_instead_of_running_the_stored_option(monkeypatch, tmp_path):
    from app import shortcut_terminals as st
    started = []
    monkeypatch.setattr(st, "_IS_WINDOWS", True)
    monkeypatch.setattr(st, "_windows_dir", lambda: tmp_path)
    monkeypatch.setattr(st, "_rows", lambda: [_fake_row({"tmux": "shortcut-hangar-abcdef", "id": "abcdef"}, tmp_path)])
    monkeypatch.setattr(st, "_option", lambda target, opt: "C:dirtemp" if opt == "@cp_shortcut_cmd" else str(tmp_path))
    monkeypatch.setattr(st, "start_hangar", lambda *a: started.append(a) or (None, False))
    with pytest.raises(st.RestartError):
        st.restart_hangar("abcdef", {})
    assert started == []


@pytest.mark.parametrize("empty", ["@cp_shortcut_cmd", "@cp_shortcut_cwd"])
def test_restart_with_an_unrecovered_option_fails_instead_of_running_nothing(monkeypatch, tmp_path, empty):
    from app import shortcut_terminals as st
    started = []
    monkeypatch.setattr(st, "_rows", lambda: [_fake_row({"tmux": "shortcut-hangar-abcdef", "id": "abcdef"}, tmp_path)])
    monkeypatch.setattr(st, "_option", lambda target, opt: "" if opt == empty else "algo")
    monkeypatch.setattr(st, "start_hangar", lambda *a: started.append(a) or (None, False))
    with pytest.raises(st.RestartError):
        st.restart_hangar("abcdef", {})
    assert started == []


def test_linux_start_fails_and_kills_the_session_when_the_key_cannot_be_stored(monkeypatch, tmp_path):
    from app import shortcut_terminals as st, tmux
    killed = []
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    rc = lambda args: 1 if "@cp_shortcut_key" in args else 0            # noqa: E731
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: subprocess.CompletedProcess(args, rc(args), "", ""))
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    monkeypatch.setattr(tmux, "kill_session", lambda name: killed.append(name) or True)
    assert st.start("", str(tmp_path), "sleep 1", "X", {}, key="global:k") is None
    assert len(killed) == 1


def test_windows_start_kills_the_session_when_it_cannot_be_hidden(monkeypatch, tmp_path):
    from app import shortcut_terminals, tmux
    killed = []
    monkeypatch.setattr(shortcut_terminals, "_IS_WINDOWS", True)
    monkeypatch.setattr(shortcut_terminals, "_windows_dir", lambda: tmp_path)
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    rc = lambda args: 1 if "@cp_hidden" in args else 0                  # noqa: E731
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: subprocess.CompletedProcess(args, rc(args), "", ""))
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    monkeypatch.setattr(tmux, "kill_session", lambda name: killed.append(name) or True)
    assert shortcut_terminals.start("s", str(tmp_path), "x", "X", {}) is None
    assert len(killed) == 1


def test_list_and_close_require_auth(client):
    assert client.get("/api/sessions/s/shortcut-terminals").status_code == 401
    assert client.post("/api/sessions/s/shortcut-terminals/abcdef/close").status_code == 401


def test_session_close_carries_the_shortcut_terminals(monkeypatch, tmp_path, private_tmux):
    from app import shortcut_terminals as st
    monkeypatch.setenv("SHELL", "/bin/sh")
    st.start("velho", str(tmp_path), "sleep 30", "x", {})
    assert len(st.list_for("velho")) == 1
    st.close_all("velho")
    assert st.list_for("velho") == []


def test_terminal_is_hidden_from_the_session_list(monkeypatch, tmp_path, private_tmux):
    from app import shortcut_terminals as st, tmux
    monkeypatch.setenv("SHELL", "/bin/sh")
    term = st.start("s", str(tmp_path), "sleep 30", "x", {})
    panes = tmux.list_panes_all()[term["tmux"]]
    assert panes[0]["hidden"] is True


@pytest.mark.skipif(shutil.which("tmux") is None or os.name != "posix", reason="precisa de tmux")
def test_terminal_socket_attaches_to_its_own_shortcut(client, monkeypatch, tmp_path):
    # O attach do painel roda `tmux attach` no servidor padrao: este caso usa ele, com nome de teste.
    from app import shortcut_terminals as st, termsock
    if not termsock.painel_disponivel():
        pytest.skip("esta maquina nao abre painel de terminal")
    monkeypatch.setenv("SHELL", "/bin/sh")
    owner = "cp-test-shortcut"
    st.close_all(owner)
    term = st.start(owner, str(tmp_path), "echo marca-do-atalho; sleep 30", "x", {})
    try:
        with client.websocket_connect(
                f"/api/sessions/{owner}/term?token=secret&cols=80&rows=24&shortcut={term['id']}") as ws:
            seen = b""
            limit = time.monotonic() + 5
            while b"marca-do-atalho" not in seen and time.monotonic() < limit:
                seen += ws.receive_bytes()
            assert b"marca-do-atalho" in seen
            ws.close()
    finally:
        st.close_all(owner)
    assert st.list_for(owner) == []


def test_shell_only_fields_are_validated():
    shell = {"id": "x", "type": "shell", "label": "L", "command": "c", "runs_in": "hangar",
             "hangar_home": False, "answer_in_app": False}
    rc.validate_shortcut_item(shell, "w")
    rc.validate_shortcut_item({**shell, "runs_in": "session"}, "w")
    with pytest.raises(ValueError, match="runs_in"):
        rc.validate_shortcut_item({**shell, "runs_in": "global"}, "w")
    with pytest.raises(ValueError, match="hangar_home"):
        rc.validate_shortcut_item({**shell, "hangar_home": "sim"}, "w")
    with pytest.raises(ValueError, match="answer_in_app"):
        rc.validate_shortcut_item({**shell, "answer_in_app": 1}, "w")
    with pytest.raises(ValueError, match="runs_in"):
        rc.validate_shortcut_item(
            {"id": "y", "type": "send_text", "label": "L", "text": "t", "runs_in": "hangar"}, "w")


@pytest.mark.parametrize("raw,expected", [
    ("start https://x/a%20b", "start https://x/a%%20b"),
    ("for %i in (a) do echo %i", "for %%i in (a) do echo %%i"),
    ("echo %1 %* %~dp0", "echo %%1 %%* %%~dp0"),
    ("echo 100%%", "echo 100%%%%"),
    ("echo %HANGAR_TEST_VAR% %NAO_EXISTE_XYZ%", "echo %HANGAR_TEST_VAR% %%NAO_EXISTE_XYZ%%"),
    ("echo %ERRORLEVEL% %cd%", "echo %ERRORLEVEL% %cd%"),
])
def test_batch_escape_keeps_cmd_line_meaning_and_env_vars(monkeypatch, raw, expected):
    from app import shortcut_terminals as st
    monkeypatch.setenv("HANGAR_TEST_VAR", "1")
    escaped = st._batch_escape(raw)
    assert escaped == expected
    assert st._batch_unescape(escaped) == raw                          # o "Rodar de novo" recupera o original


def test_windows_start_does_not_store_the_command_option_nor_an_empty_owner(monkeypatch, tmp_path):
    from app import shortcut_terminals, tmux
    calls = []
    monkeypatch.setattr(shortcut_terminals, "_IS_WINDOWS", True)
    monkeypatch.setattr(shortcut_terminals, "_windows_dir", lambda: tmp_path)
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: calls.append(args) or subprocess.CompletedProcess(args, 0, "", ""))
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    term = shortcut_terminals.start("", str(tmp_path), "echo a;\necho b", "X", {}, key="global:k")
    assert term is not None
    options = [a[a.index("set-option") + 3] for a in calls if "set-option" in a]
    assert "@cp_shortcut_cmd" not in options and "@cp_shortcut_owner" not in options
    assert "@cp_shortcut_cwd" in options and "@cp_shortcut_key" in options


def test_windows_restart_gets_the_original_percent_back(monkeypatch, tmp_path):
    from app import shortcut_terminals as st, tmux
    monkeypatch.setattr(st, "_IS_WINDOWS", True)
    monkeypatch.setattr(st, "_windows_dir", lambda: tmp_path)
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: subprocess.CompletedProcess(args, 0, "", ""))
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    term = st.start("", str(tmp_path), "start https://x/a%20b", "X", {}, key="global:k")
    assert "a%%20b" in (tmp_path / f"{term['id']}-cmd.cmd").read_bytes().decode("latin-1")
    seen = []
    monkeypatch.setattr(st, "_rows", lambda: [_fake_row(term, tmp_path)])
    monkeypatch.setattr(st, "_option", lambda target, opt: str(tmp_path) if opt == "@cp_shortcut_cwd" else "")
    monkeypatch.setattr(st, "start_hangar", lambda *a: seen.append(a) or (None, False))
    st.restart_hangar(term["id"], {})
    assert seen[0][2] == "start https://x/a%20b"


def test_windows_sweep_removes_only_old_files_of_terminals_that_no_longer_exist(monkeypatch, tmp_path):
    from app import shortcut_terminals as st
    monkeypatch.setattr(st, "_IS_WINDOWS", True)
    monkeypatch.setattr(st, "_windows_dir", lambda: tmp_path)
    old = time.time() - 3600
    names = ["aaaaaa.cmd", "aaaaaa-cmd.cmd", "aaaaaa.exit", "bbbbbb.cmd", "cccccc.cmd", "leia-me.txt"]
    for n in names:
        (tmp_path / n).write_text("x")
    for n in names:
        if n != "cccccc.cmd":                                          # cccccc e de um start em andamento
            os.utime(tmp_path / n, (old, old))
    live = {"bbbbbb"}
    monkeypatch.setattr(st, "_read_rows", lambda: [
        {"id": i, "created": 1, "seq": 1, "tmux": i, "alive": False, "label": "", "exit_code": None, "owner": "",
         "key": "", "origin": "", "ask": True, "pid": None} for i in live])
    st.list_all()
    assert sorted(p.name for p in tmp_path.iterdir()) == ["bbbbbb.cmd", "cccccc.cmd", "leia-me.txt"]


def test_windows_sweep_does_nothing_when_the_multiplexer_did_not_answer(monkeypatch, tmp_path):
    from app import shortcut_terminals as st
    monkeypatch.setattr(st, "_IS_WINDOWS", True)
    monkeypatch.setattr(st, "_windows_dir", lambda: tmp_path)
    (tmp_path / "aaaaaa.cmd").write_text("x")
    os.utime(tmp_path / "aaaaaa.cmd", (1, 1))
    monkeypatch.setattr(st, "_read_rows", lambda: None)
    with pytest.raises(st.MuxUnavailable):
        st.list_all()
    assert (tmp_path / "aaaaaa.cmd").exists()


def test_windows_start_failure_removes_the_command_files(monkeypatch, tmp_path):
    from app import shortcut_terminals as st, tmux
    monkeypatch.setattr(st, "_IS_WINDOWS", True)
    monkeypatch.setattr(st, "_windows_dir", lambda: tmp_path)
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: subprocess.CompletedProcess(args, 1, "", "boom"))
    monkeypatch.setattr(tmux, "has_session", lambda name: False)
    assert st.start("s", str(tmp_path), "echo segredo", "X", {}) is None
    assert list(tmp_path.iterdir()) == []


def test_windows_abort_before_the_id_removes_the_command_files(monkeypatch, tmp_path):
    from app import shortcut_terminals as st, tmux
    monkeypatch.setattr(st, "_IS_WINDOWS", True)
    monkeypatch.setattr(st, "_windows_dir", lambda: tmp_path)
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    rc = lambda args: 1 if "@cp_hidden" in args else 0                  # noqa: E731
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: subprocess.CompletedProcess(args, rc(args), "", ""))
    monkeypatch.setattr(tmux, "has_session", lambda name: True)
    monkeypatch.setattr(tmux, "kill_session", lambda name: True)
    assert st.start("s", str(tmp_path), "echo segredo", "X", {}) is None
    assert list(tmp_path.iterdir()) == []


def test_hangar_start_refuses_when_the_multiplexer_did_not_answer(client, monkeypatch, tmp_path, home):
    from app import shortcut_terminals as st, tmux
    started = []
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: started.append(args) or subprocess.CompletedProcess(
        args, tmux.RC_INDISPONIVEL, "", "TimeoutExpired"))
    with pytest.raises(st.MuxUnavailable):
        st.start_hangar("global:k1", str(tmp_path), "x", "X", {}, "", True)


def test_windows_kill_group_survives_taskkill_timeout_and_missing_binary(monkeypatch):
    from app import shortcut_terminals as st
    monkeypatch.setattr(st, "_IS_WINDOWS", True)
    for exc in (subprocess.TimeoutExpired(["taskkill"], 10), FileNotFoundError("taskkill"), OSError("x")):
        def boom(*a, _exc=exc, **kw):
            raise _exc
        monkeypatch.setattr(st.subprocess, "run", boom)
        st._kill_group(1234)                                          # nao levanta


def test_mux_without_answer_raises_instead_of_an_empty_list(client, monkeypatch):
    from app import shortcut_terminals as st, terminal_prompt
    forgotten = []
    monkeypatch.setattr(st, "_read_rows", lambda: None)
    monkeypatch.setattr(terminal_prompt, "forget", lambda live: forgotten.append(live))
    with pytest.raises(st.MuxUnavailable):
        st.list_all()
    assert forgotten == []                            # as amostras de pergunta pendente ficam
    r = client.get("/api/hangar-terminals", headers=_auth())
    assert r.status_code == 503 and r.json()["detail"]["code"] == "erro_mux_indisponivel"
    r = client.post("/api/hangar-terminals/abcdef/close", headers=_auth())
    assert r.status_code == 503                       # nunca 404 "nao existe"


def test_mux_without_answer_never_breaks_the_best_effort_paths(monkeypatch):
    from app import shortcut_terminals as st
    monkeypatch.setattr(st, "_read_rows", lambda: None)
    st.close_all("s")
    st._abort("shortcut-x-abcdef")                    # sem lista, cai no kill direto


def test_set_options_failure_names_the_option(monkeypatch, caplog):
    from app import shortcut_terminals as st, tmux
    monkeypatch.setattr(tmux, "_run", lambda args, **kw: subprocess.CompletedProcess(args, 1, "", "boom"))
    with caplog.at_level("WARNING"):
        assert st._set_options("t", (("@cp_shortcut_key", "k"),)) is False
    assert "@cp_shortcut_key" in caplog.text and "rc=1" in caplog.text
