import shutil
import subprocess
import uuid
from unittest.mock import MagicMock, patch

import pytest
from types import SimpleNamespace

from app import tmux


def test_list_sessions_parses_output():
    fake = MagicMock(stdout="cc\t/home/u/p\nweb\t/home/u/w\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake) as run:
        out = tmux.list_sessions()
    assert out == [
        {"name": "cc", "cwd": "/home/u/p"},
        {"name": "web", "cwd": "/home/u/w"},
    ]
    args = run.call_args[0][0]
    assert args[:2] == ["tmux", "list-sessions"]


def test_list_sessions_empty_when_no_server():
    fake = MagicMock(stdout="", returncode=1, stderr="no server running")
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_sessions() == []


def test_list_panes_active_parses_pane_id():
    # #{pane_id} e o 5o campo (Task 5 do adapter Pi: bilhete da extensao e por-pane). Uma linha com o
    # formato ANTIGO de 4 campos tem de ser rejeitada, senao um tmux desatualizado alimentaria pid/cwd
    # errados no lugar do pane_id.
    fake = MagicMock(stdout="cc\t1\t123\t/home/u/p\t%9\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_panes_active() == [
            {"name": "cc", "pid": 123, "cwd": "/home/u/p", "pane_id": "%9"},
        ]


def test_list_panes_active_rejects_the_old_four_field_format():
    fake = MagicMock(stdout="cc\t1\t123\t/home/u/p\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_panes_active() == []


def test_send_keys_literal_uses_dashdash():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "echo hi", literal=True)
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "=cc:", "-l", "--", "echo hi"]


def test_send_keys_named_key():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "Enter")
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "=cc:", "Enter"]


def test_capture_pane_returns_stdout():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="screen", returncode=0)) as run:
        assert tmux.capture_pane("cc") == "screen"
    assert run.call_args[0][0][:2] == ["tmux", "capture-pane"]


@pytest.mark.skipif(shutil.which("tmux") is None, reason="tmux nao instalado no ambiente")
def test_has_session_is_exact_against_real_tmux():
    # SEMANTICA REAL do tmux (nao mock): sem o `=`, o `-t` resolve exact -> fnmatch -> PREFIX match,
    # entao `has_session("X")` respondia VIVO por causa da IRMA "X-2" — e o /input dava "entregue"
    # digitando num pane que nao existe. Os outros testes stubam has_session, por isso nenhum via.
    # Socket proprio (-L) e sessao de nome aleatorio: nao encosta no tmux/sessoes do usuario.
    sock = f"cp-test-{uuid.uuid4().hex[:8]}"
    base = f"pocket-{uuid.uuid4().hex[:6]}"

    def tmux_on_sock(args, **_kw):
        # injeta o `-L <socket>` logo apos o "tmux" -> o has_session real roda contra ESTE servidor
        return subprocess.run(["tmux", "-L", sock, *args[1:]], capture_output=True, text=True)

    subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", f"{base}-2", "sleep 60"],
                   capture_output=True, text=True)
    try:
        with patch.object(tmux, "RUN", tmux_on_sock):
            assert tmux.has_session(f"{base}-2") is True   # exata e viva
            assert tmux.has_session(base) is False         # NUNCA existiu (so a irma "-2") -> prefix mentia
            assert tmux.has_session(base[:-3]) is False    # prefixo puro
    finally:
        # kill-SESSION (alvo exato), nunca kill-server: um `-L` esquecido num kill-server derruba o
        # servidor tmux DEFAULT e com ele todas as sessoes do usuario. Matar a unica sessao ja encerra
        # este servidor sozinho, e um socket orfao vazio e inofensivo — nao vale o risco do atalho.
        subprocess.run(["tmux", "-L", sock, "kill-session", "-t", f"={base}-2"],
                       capture_output=True, text=True)


def test_pane_target_uses_exact_session_form():
    # Nome NUMERICO (0/1/2) nao pode virar indice de window -> `=NAME:` forca match exato de sessao.
    assert tmux._pane_target("0") == "=0:"
    assert tmux._pane_target("cc") == "=cc:"


def test_capture_pane_targets_exact_session():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="", returncode=0)) as run:
        tmux.capture_pane("0")
    assert "=0:" in run.call_args[0][0]


def test_pane_pid_targets_exact_session():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="540144\n", returncode=0)) as run:
        assert tmux.pane_pid("0") == 540144
    assert "=0:" in run.call_args[0][0]


class _CP:
    returncode = 0
    stdout = ""
    stderr = ""


def test_new_session_forwards_explicit_config_dir():
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x", config_dir="/home/u/.claude-clean")
    assert "CLAUDE_CONFIG_DIR=/home/u/.claude-clean" in captured["args"]


def test_new_session_falls_back_to_backend_config_dir(monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/home/u/.claude-work")
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "CLAUDE_CONFIG_DIR=/home/u/.claude-work" in captured["args"]


def test_scope_prefix_empty_without_runtime_dir(monkeypatch):
    # Sem XDG_RUNTIME_DIR (host nao-systemd) -> spawn direto, sem wrap.
    monkeypatch.delenv("XDG_RUNTIME_DIR", raising=False)
    assert tmux._scope_prefix() == []


def test_scope_prefix_wraps_when_systemd_available(monkeypatch):
    # Com runtime dir + systemd-run QUE FUNCIONA -> tmux nasce em scope proprio (fora do cgroup do
    # backend). O probe entrou depois deste teste: "systemd-run instalado" deixou de bastar, porque o
    # gerenciador do usuario pode recusar o scope e ai o wrap fazia toda criacao de sessao falhar.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: subprocess.CompletedProcess(args, 0))
    assert tmux._scope_prefix()[:3] == ["systemd-run", "--user", "--scope"]


def test_new_session_passes_wayland_display_from_env(monkeypatch):
    monkeypatch.setenv("WAYLAND_DISPLAY", "wayland-7")
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "WAYLAND_DISPLAY=wayland-7" in captured["args"]


def test_new_session_detects_wayland_socket(monkeypatch, tmp_path):
    # Backend como servico systemd nao tem WAYLAND_DISPLAY -> detecta o socket no runtime dir
    # (ignorando o .lock). Sem isto, wl-paste no pane falha e o paste de imagem no claude morre.
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    (tmp_path / "wayland-1").touch()
    (tmp_path / "wayland-1.lock").touch()
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "WAYLAND_DISPLAY=wayland-1" in captured["args"]


def test_new_session_skips_wayland_without_socket(monkeypatch, tmp_path):
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert not any(str(a).startswith("WAYLAND_DISPLAY=") for a in captured["args"])


def test_new_session_execs_command_so_claude_owns_tty(monkeypatch):
    # O comando vai prefixado com `exec`: o tmux roda via `fish -c`, e sem exec o fish ficaria como
    # dono do tty e o send-keys nao chegaria no claude. Com exec, o fish vira o claude.
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert captured["args"][-1] == "exec claude --session-id x"


# --- scrollback real vs tela alternada ------------------------------------------------------
# Claude Code roda em tela alternada, onde o tmux NAO acumula historico: pedir mais linhas nunca
# traz nada e a UI nao pode oferecer "carregar mais historico". A TUI do Codex sobe com
# --no-alt-screen, entao la o scrollback existe. O formato do display resolve os dois num comando so.

def test_pane_scrollback_zero_em_tela_alternada():
    from unittest.mock import patch
    from app import tmux as t
    with patch.object(t, "_run", return_value=SimpleNamespace(stdout="0\n", returncode=0)):
        assert t.pane_scrollback("sess") == 0


def test_pane_scrollback_devolve_historico_quando_existe():
    from unittest.mock import patch
    from app import tmux as t
    with patch.object(t, "_run", return_value=SimpleNamespace(stdout="1873\n", returncode=0)):
        assert t.pane_scrollback("sess") == 1873


def test_pane_scrollback_falha_de_tmux_e_zero_MAS_logada(caplog):
    # 0 = "sem historico" E tmux quebrado davam a MESMA resposta: a UI so escondia o botao e a falha
    # real (tmux ausente, travado no timeout, sessao zumbi) sumia. Segue devolvendo 0 pra UI nao
    # explodir, mas TEM que aparecer no log.
    from unittest.mock import patch
    from app import tmux as t
    with caplog.at_level("WARNING"), \
         patch.object(t, "_run",
                      return_value=SimpleNamespace(stdout="", stderr="no server running", returncode=1)):
        assert t.pane_scrollback("sess") == 0
    assert "no server running" in caplog.text


def test_capture_pane_falha_de_tmux_e_logada(caplog):
    # stdout vazio numa falha e indistinguivel de pane genuinamente vazio -> o /pane devolvia 200
    # com texto "" e ninguem sabia que o tmux falhou.
    from unittest.mock import patch
    from app import tmux as t
    with caplog.at_level("WARNING"), \
         patch.object(t, "_run",
                      return_value=SimpleNamespace(stdout="", stderr="session not found", returncode=1)):
        assert t.capture_pane("sess") == ""
    assert "session not found" in caplog.text


def test_scope_prefix_sem_scope_quando_systemd_run_falha(monkeypatch):
    # systemd-run pode ESTAR instalado e ainda assim recusar criar scope transiente ("Failed to start
    # transient scope unit"). Sem este gate, TODA criacao de sessao morria com "falha ao criar sessao
    # no tmux" — o app parava de abrir sessao por causa de um detalhe de cgroup que e opcional.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: subprocess.CompletedProcess(args, 1))
    assert tmux._scope_prefix() == []


def test_scope_prefix_usa_scope_quando_systemd_run_funciona(monkeypatch):
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: subprocess.CompletedProcess(args, 0))
    assert tmux._scope_prefix() == ["systemd-run", "--user", "--scope", "--collect", "-q", "--"]


def test_scope_probe_roda_uma_vez_so(monkeypatch):
    # O probe custa um fork; repetir a cada sessao seria desperdicio num estado que quase nunca muda.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    chamadas = []
    monkeypatch.setattr(tmux, "RUN",
                        lambda args, **k: (chamadas.append(args) or subprocess.CompletedProcess(args, 0)))
    tmux._scope_prefix()
    tmux._scope_prefix()
    tmux._scope_prefix()
    assert len(chamadas) == 1


def test_new_session_sem_systemd_run_quebrado_ainda_cria(monkeypatch):
    # O caminho completo: systemd-run recusando, new_session tem de montar o comando do tmux SEM o
    # prefixo em vez de falhar.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    vistos = []

    def _fake(args, **k):
        vistos.append(args)
        # o probe (…-- true) falha; o tmux de verdade passa
        codigo = 1 if args[-1] == "true" else 0
        return subprocess.CompletedProcess(args, codigo, stdout="", stderr="")

    monkeypatch.setattr(tmux, "RUN", _fake)
    assert tmux.new_session("s", "/tmp", "claude --session-id x") is True
    cmd = vistos[-1]
    assert cmd[0] == "tmux"
    assert "systemd-run" not in cmd


# --- fatiamento de literal grande no Windows (corte do inicio pela TUI no submit) -----------------
# No Windows a TUI do Claude Code come o COMECO de um send-keys -l acima de ~1120 chars (entra em
# modo paste e o Enter envia so a cauda). Foi o corte do prompt de pareamento (1220 -> so ~300
# finais, sem o "[de: claude-pocket]"). O send_keys fatia em pedacos <= _WIN_CHUNK com pausa, SO no
# Windows. Estes testes travam as duas garantias: Windows fatia byte-exato; Linux fica intocado.

def _captura_run(monkeypatch):
    chamadas = []
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: (chamadas.append(list(args)) or _CP()))
    return chamadas


def test_send_literal_curto_uma_chamada_so(monkeypatch):
    # Dentro do teto -> UMA chamada, identico ao de sempre (mesmo no Windows).
    monkeypatch.setattr(tmux.os, "name", "nt")
    chamadas = _captura_run(monkeypatch)
    tmux.send_keys("cc", "x" * 500, literal=True)
    assert chamadas == [["tmux", "send-keys", "-t", "=cc:", "-l", "--", "x" * 500]]


def test_send_literal_windows_fatia_e_e_byte_exato(monkeypatch):
    # > teto no Windows -> fatia. A CONCATENACAO dos pedacos tem de ser byte-exata (nada perdido,
    # nada duplicado, ordem preservada) e o INICIO (o que sumia) vai no primeiro pedaco.
    monkeypatch.setattr(tmux.os, "name", "nt")
    monkeypatch.setattr(tmux.time, "sleep", lambda *_a, **_k: None)   # sem pausa real no teste
    chamadas = _captura_run(monkeypatch)
    texto = "".join(f"{i:04d}|" for i in range(1, 400))   # 1995 chars, marcado por posicao
    tmux.send_keys("cc", texto, literal=True)
    assert len(chamadas) > 1                                            # de fato fatiou
    assert all(c[:6] == ["tmux", "send-keys", "-t", "=cc:", "-l", "--"] for c in chamadas)
    pedacos = [c[6] for c in chamadas]
    assert all(len(p) <= tmux._WIN_CHUNK for p in pedacos)             # nenhum pedaco estoura o teto
    assert "".join(pedacos) == texto                                   # byte-exato
    assert pedacos[0].startswith("0001|")                              # o comeco vai primeiro


def test_send_literal_posix_nunca_fatia(monkeypatch):
    # Decisao do dono do repo: no Linux o bug nao existe -> texto grande vai numa chamada SO, sem
    # pausa. O ramo posix fica byte-identico a hoje, risco zero.
    monkeypatch.setattr(tmux.os, "name", "posix")
    chamadas = _captura_run(monkeypatch)
    texto = "z" * 5000
    tmux.send_keys("cc", texto, literal=True)
    assert chamadas == [["tmux", "send-keys", "-t", "=cc:", "-l", "--", texto]]
