import os
import time
import pytest
from unittest.mock import patch
from app import registry
from app.registry import (
    SessionRegistry,
    sanitize_cwd,
    _session_id_from_cmdline,
)

_UUID = "12345678-1234-1234-1234-123456789abc"


@pytest.fixture(autouse=True)
def _clear_jsonl_cache():
    # O cache e de CLASSE (compartilhado) -> zera entre testes pra nao vazar resolucao de um pro outro.
    SessionRegistry._jsonl_cache.clear()
    yield
    SessionRegistry._jsonl_cache.clear()


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


def test_resolve_caches_across_transient_absence(tmp_path):
    # A sessao dirigida por job spawna claude por turno -> o processo com --session-id SOME entre
    # turnos. Sem cache, a resolucao oscilava pro mtime (jsonl errado) e o watcher limpava o chat.
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    (proj / "stale.jsonl").write_text("{}")  # mtime fallback que NAO deve aparecer apos cachear
    reg = SessionRegistry(projects_dir=tmp_path)
    # 1o resolve: processo com sid PRESENTE -> cacheia.
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {_UUID}"):
        j1 = reg.resolve("cc", "/home/u/p")
    # 2o resolve: processo SUMIU (cmdline bare, sem fd) -> deve devolver o CACHE, nao o mtime.
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value="claude"), \
         patch.object(registry, "_open_jsonl", return_value=None):
        j2 = reg.resolve("cc", "/home/u/p")
    assert j1 == j2 and j1.endswith(f"{_UUID}.jsonl")


# --- pos-/clear: o claude rola um session-id NOVO mas o --session-id do cmdline congela no de boot ---

def test_resolve_follows_post_clear_transcript(tmp_path):
    # /clear -> claude passa a escrever num jsonl NOVO (id novo), o cmdline segue o id de boot. Como o
    # jsonl de boot JA existe (foi escrito antes do clear), seguir o mais recente do projeto = pos-clear.
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    boot = proj / f"{_UUID}.jsonl"
    boot.write_text("{}")
    new = proj / "deadbeef-0000-0000-0000-000000000000.jsonl"  # transcript pos-clear
    new.write_text("{}")
    os.utime(boot, (1000, 1000))
    os.utime(new, (2000, 2000))  # mais recente
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {_UUID}"), \
         patch.object(registry, "_open_jsonl", return_value=None):
        j = reg.resolve("cc", "/home/u/p")
    assert j.endswith("deadbeef-0000-0000-0000-000000000000.jsonl")


def test_resolve_post_clear_ignores_subagent_transcript(tmp_path):
    # Durante uma Task, o jsonl mais novo pode ser de um SUBAGENTE (--agent) com o fd aberto -> nao deve
    # virar o transcript da sessao; fica no jsonl do REPL (boot, ainda nao havia /clear).
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    boot = proj / f"{_UUID}.jsonl"
    boot.write_text("{}")
    sub = proj / "deadbeef-0000-0000-0000-000000000000.jsonl"  # transcript do subagente, mais novo
    sub.write_text("{}")
    os.utime(boot, (1000, 1000))
    os.utime(sub, (2000, 2000))
    reg = SessionRegistry(projects_dir=tmp_path)

    def cmdline(p):
        return f"claude --session-id deadbeef-0000-0000-0000-000000000000 --agent claude" if p == 2 \
            else f"claude --session-id {_UUID}"

    def open_jsonl(pid, _proj):
        return str(sub) if pid == 2 else None  # so o subagente segura o fd aberto

    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1, 2]), \
         patch.object(registry, "_cmdline", side_effect=cmdline), \
         patch.object(registry, "_open_jsonl", side_effect=open_jsonl):
        j = reg.resolve("cc", "/home/u/p")
    assert j.endswith(f"{_UUID}.jsonl")  # subagente excluido -> fica no REPL


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
    with patch.object(registry.tmux, "list_panes_active",
                      return_value=[{"name": "cc", "pid": 111, "cwd": "/home/u/p"}]), \
         patch.object(reg, "resolve_jsonl", return_value="/x/s.jsonl"):
        out = reg.list()
    assert out[0].name == "cc" and out[0].jsonl == "/x/s.jsonl"


async def test_list_with_state_classifies(tmp_path, monkeypatch):
    # list_with_state anexa o estado vivo: idle (sem spinner/menu), awaiting_input (menu ❯ N.) e
    # working (spinner que ANIMA entre os 2 frames). Reusa list() pra resolucao (mockada aqui).
    from app.models import SessionInfo
    reg = SessionRegistry(projects_dir=tmp_path)
    infos = [
        SessionInfo(name="idle1", cwd="/p", jsonl=None, tracked=True),
        SessionInfo(name="ask1", cwd="/p", jsonl=None, tracked=True),
        SessionInfo(name="work1", cwd="/p", jsonl=None, tracked=True),
    ]
    monkeypatch.setattr(reg, "list", lambda: infos)

    counter = {"work1": 0}

    def fake_capture(name, lines=200):
        if name == "idle1":
            return "● resposta\n────\n❯ \n────\n"
        if name == "ask1":
            return "❯ 1. Sim\n  2. Nao\nEsc to cancel\n"
        # work1: spinner muda entre frame 1 e 2 (animando) -> working confirmado no 2o frame
        c = counter["work1"]; counter["work1"] += 1
        return f"✻ Trabalhando… ({3 + c * 2}s)\n"

    monkeypatch.setattr(registry.tmux, "capture_pane", fake_capture)
    out = {s.name: s.state for s in await reg.list_with_state()}
    assert out == {"idle1": "idle", "ask1": "awaiting_input", "work1": "working"}


async def test_list_with_state_frozen_spinner_reads_idle(tmp_path, monkeypatch):
    # Marcador de turno concluido (spinner IGUAL nos 2 frames) -> idle, nao working.
    from app.models import SessionInfo
    reg = SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(reg, "list", lambda: [SessionInfo(name="f", cwd="/p", jsonl=None, tracked=True)])
    monkeypatch.setattr(registry.tmux, "capture_pane", lambda name, lines=200: "✻ Worked for 8s\n")
    out = await reg.list_with_state()
    assert out[0].state == "idle"


def test_resolve_jsonl_returns_none_when_dir_empty(tmp_path):
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    reg = SessionRegistry(projects_dir=tmp_path)
    assert reg.resolve_jsonl("/home/u/p") is None


def test_create_pins_fresh_jsonl_not_existing_mtime(tmp_path):
    # Pasta JA tem um jsonl antigo. A sessao nova nao pode resolver pra ele (newest-by-mtime) ->
    # create() fixa o jsonl proprio (uuid novo) no cache na hora.
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    (proj / "old.jsonl").write_text("{}\n")
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as ns:
        info = reg.create("cc", "/home/u/p")
    assert info.jsonl.endswith(".jsonl") and not info.jsonl.endswith("old.jsonl")
    assert SessionRegistry._jsonl_cache["cc"] == info.jsonl
    # o comando passado ao tmux carrega o --session-id do uuid novo
    assert "--session-id" in ns.call_args[0][2]


def test_create_rejects_duplicate_name(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=True):
        with pytest.raises(ValueError):
            reg.create("cc", "/home/u/p")


def test_create_raises_when_tmux_fails(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=False):
        with pytest.raises(ValueError):
            reg.create("cc", "/home/u/p")


def test_resolve_tracked_true_with_session_id(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "pane_pid", return_value=111), \
         patch.object(registry, "_descendant_pids", return_value=[111]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {_UUID}"):
        jsonl, tracked = reg.resolve_tracked("cc", "/home/u/p")
    assert tracked is True and jsonl.endswith(f"{_UUID}.jsonl")


def test_resolve_tracked_false_on_mtime_fallback(tmp_path):
    # bare claude (sem --session-id, sem fd, sem cache) -> cai no mtime -> NAO tracked.
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    (proj / "x.jsonl").write_text("{}\n")
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "pane_pid", return_value=111), \
         patch.object(registry, "_descendant_pids", return_value=[111]), \
         patch.object(registry, "_cmdline", return_value="claude"), \
         patch.object(registry, "_open_jsonl", return_value=None):
        jsonl, tracked = reg.resolve_tracked("cc", "/home/u/p")
    assert tracked is False and jsonl.endswith("x.jsonl")


def test_resolve_uses_session_config_dir(tmp_path, monkeypatch):
    # resolve_tracked deve usar o config dir DO PROCESSO (via /proc/<pid>/environ) e nao o projects_dir
    # do backend quando o session tem CLAUDE_CONFIG_DIR proprio.
    cfg = tmp_path / ".cfg"
    sid = "11111111-1111-1111-1111-111111111111"
    cwd = "/work/proj"
    jpath = cfg / "projects" / sanitize_cwd(cwd) / f"{sid}.jsonl"
    jpath.parent.mkdir(parents=True, exist_ok=True)
    jpath.write_text("", encoding="utf-8")

    monkeypatch.setattr(registry.tmux, "pane_pid", lambda name: 4242)
    monkeypatch.setattr(registry, "_descendant_pids", lambda root, children=None: [4242])
    monkeypatch.setattr(registry, "_cmdline", lambda pid: f"claude --session-id {sid}")
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)

    SessionRegistry._jsonl_cache.clear()
    r = SessionRegistry(projects_dir=tmp_path / "backend-projects")  # dir diferente do session
    resolved, tracked = r.resolve_tracked("cc", cwd)
    assert tracked is True
    assert resolved == str(jpath)   # usou o config dir da SESSAO, nao o backend projects_dir
    SessionRegistry._jsonl_cache.clear()


def test_mtime_fallback_uses_session_config_dir(tmp_path, monkeypatch):
    # Branch 4 (fallback newest-by-mtime): sessao SEM --session-id deve achar o jsonl mais recente sob o
    # config dir do pane pid (herdado pela arvore), nao o do backend. tracked=False.
    cfg = tmp_path / ".cfg"
    cwd = "/work/proj"
    proj = cfg / "projects" / sanitize_cwd(cwd)
    proj.mkdir(parents=True, exist_ok=True)
    f = proj / "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl"
    f.write_text("", encoding="utf-8")

    monkeypatch.setattr(registry.tmux, "pane_pid", lambda name: 4242)
    monkeypatch.setattr(registry, "_descendant_pids", lambda root, children=None: [4242])
    monkeypatch.setattr(registry, "_cmdline", lambda pid: "claude")          # sem --session-id
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: cfg)

    SessionRegistry._jsonl_cache.clear()
    r = SessionRegistry(projects_dir=tmp_path / "backend-projects")          # dir do backend != cfg
    resolved, tracked = r.resolve_tracked("cc", cwd)
    assert tracked is False
    assert resolved == str(f)        # achou sob o config dir da SESSAO, nao o do backend
    SessionRegistry._jsonl_cache.clear()
