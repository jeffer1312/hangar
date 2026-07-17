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
    # Os caches sao de CLASSE (compartilhados) -> zera entre testes pra nao vazar de um pro outro.
    SessionRegistry._jsonl_cache.clear()
    SessionRegistry._fd_locked.clear()
    SessionRegistry._status_cache.clear()
    SessionRegistry._label_cache.clear()
    yield
    SessionRegistry._jsonl_cache.clear()
    SessionRegistry._fd_locked.clear()
    SessionRegistry._status_cache.clear()
    SessionRegistry._label_cache.clear()


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


def test_resolve_uses_active_marker_for_resumed_transcript(tmp_path):
    # RESUME: cmdline --session-id X mas X.jsonl NUNCA existe (o claude escreve no Y resumido). O hook
    # gravou o marcador X -> Y; resolve deve devolver Y (real), nao o path fantasma X.jsonl.
    import json as _json
    projects = tmp_path / "projects"
    proj = projects / "-home-u-p"
    proj.mkdir(parents=True)
    real = proj / "deadbeef-0000-0000-0000-000000000000.jsonl"  # transcript resumido (existe)
    real.write_text("{}")
    mk = tmp_path / ".claude-pocket-active"   # config_base = projects.parent = tmp_path
    mk.mkdir()
    (mk / f"{_UUID}.json").write_text(_json.dumps({"jsonl": str(real), "ts": 1.0}))
    reg = SessionRegistry(projects_dir=projects)
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {_UUID}"), \
         patch.object(registry, "_open_jsonl", return_value=None), \
         patch.object(registry, "_config_dir_of", return_value=None):
        j, tracked = reg.resolve_tracked("cc", "/home/u/p")
    assert j == str(real)
    assert tracked is True


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


def test_resolve_no_cross_contamination_with_siblings(tmp_path):
    # VARIAS sessoes no MESMO cwd: um jsonl orfao mais NOVO (resume/clear de outra sessao) NAO pode
    # virar o transcript desta. Com irmaos no cwd, resolve usa o <id>.jsonl direto, nao o mais novo.
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    (proj / "5f3a45ba-0000-0000-0000-000000000000.jsonl").write_text("{}\n")  # o MEU (do id)
    orphan = proj / "3843d4e2-0000-0000-0000-000000000000.jsonl"
    orphan.write_text("{}\n")  # mais novo, de OUTRA sessao -> nao deve me contaminar
    os.utime(proj / "5f3a45ba-0000-0000-0000-000000000000.jsonl", (1, 1))  # o meu e mais VELHO
    my_id = "5f3a45ba-0000-0000-0000-000000000000"
    reg = SessionRegistry(projects_dir=tmp_path)
    panes = [{"name": "cc", "cwd": "/home/u/p"}, {"name": "cc2", "cwd": "/home/u/p"}]  # 2 no cwd
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {my_id}"), \
         patch.object(registry, "_open_jsonl", return_value=None), \
         patch.object(registry.tmux, "list_panes_active", return_value=panes):
        j, tracked = reg.resolve_tracked("cc", "/home/u/p")
    assert j.endswith(f"{my_id}.jsonl")  # o MEU id, nao o orfao mais novo
    assert tracked


def test_resolve_fd_lock_survives_transient_fd_absence(tmp_path):
    # fd pego num write trava a resolucao; nos polls SEGUINTES sem fd, MANTEM (nao reverte pro id da
    # cmdline). Resume: o id da cmdline nunca vira arquivo -> sem o lock oscilava fd<->id e o chat piscava.
    proj = tmp_path / "-home-u-p"
    proj.mkdir()
    real = str(proj / "3843d4e2-0000-0000-0000-000000000000.jsonl")
    cmd_id = "f5d2c518-0000-0000-0000-000000000000"  # id da cmdline; jsonl nunca existe
    reg = SessionRegistry(projects_dir=tmp_path)
    # poll 1: fd ABERTO -> trava em `real`
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {cmd_id}"), \
         patch.object(registry, "_open_jsonl", return_value=real):
        j1, _ = reg.resolve_tracked("cc", "/home/u/p")
    assert j1 == real
    # poll 2: fd AUSENTE -> mantem `real` (nao volta pro f5d2c518.jsonl fantasma)
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {cmd_id}"), \
         patch.object(registry, "_open_jsonl", return_value=None):
        j2, tracked = reg.resolve_tracked("cc", "/home/u/p")
    assert j2 == real
    assert tracked


def test_resolve_marker_unsticks_fd_lock_after_clear(tmp_path):
    # REGRESSAO: /clear rola transcript NOVO escrito em append-and-close (fd quase nunca aberto no poll)
    # e cujo session-id novo NUNCA vai pro cmdline (congela no boot). O fd-lock ficava preso no transcript
    # PRE-clear e o chat nao migrava (nem limpava, nem recebia msg nova). O marcador do hook
    # (boot_id -> transcript ativo) deve DESTRAVAR o cache e resolver pro transcript pos-clear.
    import json as _json
    projects = tmp_path / "projects"
    proj = projects / "-home-u-p"
    proj.mkdir(parents=True)
    boot_id = _UUID  # session-id do cmdline (congela no boot)
    pre = str(proj / "aaaa0000-0000-0000-0000-000000000000.jsonl")   # transcript PRE-clear (fd-locked)
    post = proj / "bbbb1111-0000-0000-0000-000000000000.jsonl"       # transcript POS-clear (marcador)
    post.write_text("{}")
    mk = tmp_path / ".claude-pocket-active"   # config_base = projects.parent = tmp_path
    mk.mkdir()
    reg = SessionRegistry(projects_dir=projects)
    # poll 1: fd ABERTO no transcript PRE-clear -> trava em `pre`
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {boot_id}"), \
         patch.object(registry, "_config_dir_of", return_value=None), \
         patch.object(registry, "_open_jsonl", return_value=pre):
        j1, _ = reg.resolve_tracked("cc", "/home/u/p")
    assert j1 == pre
    assert "cc" in SessionRegistry._fd_locked
    # /clear: o hook grava o marcador boot_id -> transcript POS-clear
    (mk / f"{boot_id}.json").write_text(_json.dumps({"jsonl": str(post), "ts": 2.0}))
    # poll 2: fd AUSENTE (append-and-close) -> o marcador destrava o lock e migra pro POS-clear
    with patch.object(registry.tmux, "pane_pid", return_value=1), \
         patch.object(registry, "_descendant_pids", return_value=[1]), \
         patch.object(registry, "_cmdline", return_value=f"claude --session-id {boot_id}"), \
         patch.object(registry, "_config_dir_of", return_value=None), \
         patch.object(registry, "_open_jsonl", return_value=None):
        j2, tracked = reg.resolve_tracked("cc", "/home/u/p")
    assert j2 == str(post)   # migrou pro pos-clear, NAO ficou preso no pre
    assert tracked


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
    out = {s.name: s for s in await reg.list_with_state()}
    assert {n: s.state for n, s in out.items()} == {"idle1": "idle", "ask1": "awaiting_input", "work1": "working"}
    # As opções do menu chegam no SessionInfo, não só o state (feature #1 — linha acionável).
    assert out["ask1"].options == ["Sim", "Nao"]


async def test_list_with_state_frozen_spinner_reads_idle(tmp_path, monkeypatch):
    # Marcador de turno concluido (spinner IGUAL nos 2 frames) -> idle, nao working.
    from app.models import SessionInfo
    reg = SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(reg, "list", lambda: [SessionInfo(name="f", cwd="/p", jsonl=None, tracked=True)])
    monkeypatch.setattr(registry.tmux, "capture_pane", lambda name, lines=200: "✻ Worked for 8s\n")
    out = await reg.list_with_state()
    assert out[0].state == "idle"


async def test_list_with_state_scrapes_pane_for_awaiting_marker(tmp_path, monkeypatch):
    # Um marcador de hook "awaiting_input" NAO carrega a pergunta (so state+ts); list_with_state
    # precisa raspar o pane dessas sessoes pra obter question/options — senao a linha mostra
    # awaiting SEM a pergunta (o caso real: sessoes bloqueadas TEM marcador awaiting).
    from app.models import SessionInfo
    ASK_PANE = (
        "☐ Plano\n"
        "Would you like to proceed?\n"
        "\n"
        "❯ 1. Yes\n"
        "  2. No\n"
        "Esc to cancel\n"
    )
    reg = SessionRegistry(projects_dir=tmp_path)
    info = SessionInfo(name="ask1", cwd="/p", jsonl="/x/abc.jsonl", tracked=True)
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(registry.hook_state, "get_state", lambda sid: ("awaiting_input", 1.0))
    monkeypatch.setattr(registry.tmux, "capture_pane", lambda name, lines=200: ASK_PANE)

    out = await reg.list_with_state()

    assert out[0].state == "awaiting_input"
    assert out[0].question == "Would you like to proceed?"
    assert out[0].options == ["Yes", "No"]


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


# --- create(resume_session_id=...): retomar conversa MORTA do Arquivo (sobe sessao NOVA com --resume) ---

def test_create_with_resume_session_id_builds_resume_command(tmp_path):
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False), \
         patch.object(registry.tmux, "new_session", return_value=True) as ns:
        info = reg.create("cc", "/home/u/p", resume_session_id=_UUID)
    # comando relançado carrega --resume (nao --session-id) com o uuid EXISTENTE
    assert ns.call_args[0][2] == f"claude --resume {_UUID}"
    assert info.jsonl.endswith(f"{_UUID}.jsonl")
    assert SessionRegistry._jsonl_cache["cc"] == info.jsonl


def test_create_with_resume_session_id_rejects_bad_uuid(tmp_path):
    # uuid vai DIRETO pro comando do shell -> invalido (injecao) e recusado antes de tocar tmux.
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "has_session", return_value=False):
        with pytest.raises(ValueError):
            reg.create("cc", "/home/u/p", resume_session_id="; rm -rf ~")


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


import asyncio
from app import hook_state as hs_mod


def test_list_with_state_prefers_marker(monkeypatch):
    reg = SessionRegistry()
    info = type("I", (), {"name": "cc", "cwd": "/p", "jsonl": "/x/sid123.jsonl", "state": "idle", "last_activity": None})()
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: ("working", 1.0) if sid == "sid123" else None)
    called = {"pane": 0}
    def fake_capture(name):
        called["pane"] += 1; return ""
    monkeypatch.setattr("app.registry.tmux.capture_pane", fake_capture)
    # Statusline cacheada fresca: o sweep de statusline (que LEGITIMAMENTE captura ate
    # _STATUS_BUDGET panes por chamada, mesmo de sessao com marcador) nao dispara — o assert
    # abaixo volta a medir SO o fast-path da classificacao, que e o que este teste protege.
    import time as _t
    SessionRegistry._status_cache["cc"] = (_t.monotonic(), None)
    out = asyncio.run(reg.list_with_state())
    assert out[0].state == "working"
    assert called["pane"] == 0          # marcador presente -> classificacao NAO raspa o pane


def test_list_with_state_statusline_sweep_respeita_budget(monkeypatch):
    # O sweep de statusline captura no maximo _STATUS_BUDGET panes por chamada, mesmo com todas
    # as sessoes stale — e o que impede a feature de virar tempestade de forks.
    import app.registry as reg_mod
    reg = SessionRegistry()
    infos = [type("I", (), {"name": f"s{i}", "cwd": "/p", "jsonl": f"/x/sid{i}.jsonl",
                            "state": "idle", "last_activity": None})() for i in range(5)]
    monkeypatch.setattr(reg, "list", lambda: infos)
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: ("idle", 1.0))
    called = {"pane": 0}
    def fake_capture(name):
        called["pane"] += 1; return "🤖 X │ ⚡5h:10%"
    monkeypatch.setattr("app.registry.tmux.capture_pane", fake_capture)
    out = asyncio.run(reg.list_with_state())
    assert called["pane"] == reg_mod._STATUS_BUDGET
    assert sum(1 for i in out if getattr(i, "status_line", None)) == reg_mod._STATUS_BUDGET


def test_list_with_state_falls_back_to_pane(monkeypatch):
    reg = SessionRegistry()
    info = type("I", (), {"name": "cc", "cwd": "/p", "jsonl": "/x/none.jsonl", "state": "idle", "last_activity": None})()
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: None)   # sem marcador
    monkeypatch.setattr("app.registry.tmux.capture_pane", lambda name: "")  # pane vazio -> idle
    out = asyncio.run(reg.list_with_state())
    assert out[0].state == "idle"


# ---------------------------------------------------------------------------
# Feature #7: bool "stalled" derivado (working ha mais de CP_STALL_SECONDS sem o jsonl avancar)
# ---------------------------------------------------------------------------

def test_list_with_state_marks_stalled_past_threshold(tmp_path, monkeypatch):
    reg = SessionRegistry(projects_dir=tmp_path)
    jsonl = tmp_path / "sid123.jsonl"
    jsonl.write_text("{}\n")
    old = time.time() - 999
    os.utime(jsonl, (old, old))  # transcript parado ha muito mais que o threshold
    info = type("I", (), {"name": "cc", "cwd": "/p", "jsonl": str(jsonl), "state": "idle", "last_activity": None})()
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: ("working", old) if sid == "sid123" else None)
    monkeypatch.setattr(registry.settings, "stall_seconds", 300)
    out = asyncio.run(reg.list_with_state())
    assert out[0].state == "working"
    assert out[0].stalled is True


def test_list_with_state_not_stalled_when_recent(tmp_path, monkeypatch):
    # "working" mas o jsonl acabou de mexer (mtime fresco) -> ainda dentro do threshold, nao travada.
    reg = SessionRegistry(projects_dir=tmp_path)
    jsonl = tmp_path / "sid456.jsonl"
    jsonl.write_text("{}\n")  # mtime = agora
    info = type("I", (), {"name": "cc", "cwd": "/p", "jsonl": str(jsonl), "state": "idle", "last_activity": None})()
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: ("working", time.time()) if sid == "sid456" else None)
    monkeypatch.setattr(registry.settings, "stall_seconds", 300)
    out = asyncio.run(reg.list_with_state())
    assert out[0].state == "working"
    assert out[0].stalled is False


def test_list_with_state_not_stalled_when_not_working(tmp_path, monkeypatch):
    # Parada ha muito tempo mas em "idle" (nao "working") -> nao e travada, e conversa terminada.
    reg = SessionRegistry(projects_dir=tmp_path)
    jsonl = tmp_path / "sid789.jsonl"
    jsonl.write_text("{}\n")
    old = time.time() - 999
    os.utime(jsonl, (old, old))
    info = type("I", (), {"name": "cc", "cwd": "/p", "jsonl": str(jsonl), "state": "idle", "last_activity": None})()
    monkeypatch.setattr(reg, "list", lambda: [info])
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: ("idle", old) if sid == "sid789" else None)
    monkeypatch.setattr(registry.settings, "stall_seconds", 300)
    out = asyncio.run(reg.list_with_state())
    assert out[0].state == "idle"
    assert out[0].stalled is False


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


# --- guarda de colisao: 2+ sessoes -> MESMO jsonl. So a DONA (cmdline sid == basename do jsonl) mantem;
#     quem tomou emprestado o transcript de outra e rebaixada (jsonl=None, tracked=False) -> sem
#     duplicata na lista nem send roteado pro terminal errado. ---

def test_dedupe_collision_owner_keeps_borrower_demoted():
    from app.models import SessionInfo
    reg = SessionRegistry()
    owner = SessionInfo(name="A", cwd="/c", jsonl="/p/X.jsonl", tracked=True)
    borrower = SessionInfo(name="B", cwd="/c", jsonl="/p/X.jsonl", tracked=True)
    sids = {"A": "X", "B": "Y"}      # A e dona de X; B tem sid proprio Y -> X.jsonl nao e dela
    reg._dedupe_collisions([owner, borrower], sids)
    assert owner.jsonl == "/p/X.jsonl" and owner.tracked is True
    assert borrower.jsonl is None and borrower.tracked is False


def test_dedupe_no_collision_untouched():
    from app.models import SessionInfo
    reg = SessionRegistry()
    a = SessionInfo(name="A", cwd="/c", jsonl="/p/X.jsonl", tracked=True)
    b = SessionInfo(name="B", cwd="/c", jsonl="/p/Y.jsonl", tracked=True)
    reg._dedupe_collisions([a, b], {"A": "X", "B": "Y"})
    assert a.jsonl == "/p/X.jsonl" and b.jsonl == "/p/Y.jsonl"
    assert a.tracked and b.tracked


def test_dedupe_collision_no_owner_demotes_all():
    # Nenhuma e dona (ambas resumiram o transcript de um terceiro) -> rebaixa as duas: nao arriscar
    # mostrar/rotear pro transcript errado pra ninguem.
    from app.models import SessionInfo
    reg = SessionRegistry()
    a = SessionInfo(name="A", cwd="/c", jsonl="/p/Z.jsonl", tracked=True)
    b = SessionInfo(name="B", cwd="/c", jsonl="/p/Z.jsonl", tracked=True)
    reg._dedupe_collisions([a, b], {"A": "X", "B": "Y"})
    assert a.jsonl is None and b.jsonl is None


# --- resume de sessao "sem id": relança com --resume -> passa a rastrear, continuando a conversa ---

def test_resume_respawns_with_resume_flag(tmp_path):
    cwd = "/home/u/p"
    proj = tmp_path / sanitize_cwd(cwd)
    proj.mkdir()
    (proj / f"{_UUID}.jsonl").write_text("{}\n")  # o transcript a retomar (precisa existir)
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "list_panes_active",
                      return_value=[{"name": "cc", "pid": 111, "cwd": cwd}]), \
         patch.object(registry, "_config_dir_of", return_value=None), \
         patch.object(registry.tmux, "kill_session") as kill, \
         patch.object(registry.tmux, "new_session", return_value=True) as ns:
        info = reg.resume("cc", _UUID)
    assert info.tracked is True and info.jsonl.endswith(f"{_UUID}.jsonl")
    kill.assert_called_once_with("cc")
    assert f"--resume {_UUID}" in ns.call_args[0][2]        # comando relançado carrega --resume
    assert SessionRegistry._jsonl_cache["cc"].endswith(f"{_UUID}.jsonl")


def test_resume_rejects_bad_uuid(tmp_path):
    # session_id vai DIRETO pro comando do shell -> uuid invalido (injecao) e recusado antes de tocar tmux.
    reg = SessionRegistry(projects_dir=tmp_path)
    with pytest.raises(ValueError):
        reg.resume("cc", "; rm -rf ~")


def test_resume_missing_transcript_raises(tmp_path):
    cwd = "/home/u/p"
    (tmp_path / sanitize_cwd(cwd)).mkdir()  # dir existe, mas o <uuid>.jsonl NAO
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "list_panes_active",
                      return_value=[{"name": "cc", "pid": 111, "cwd": cwd}]), \
         patch.object(registry, "_config_dir_of", return_value=None), \
         patch.object(registry.tmux, "new_session", return_value=True):
        with pytest.raises(ValueError):
            reg.resume("cc", _UUID)


def test_resume_candidates_lists_recent_with_preview(tmp_path):
    import json as _json
    cwd = "/home/u/p"
    proj = tmp_path / sanitize_cwd(cwd)
    proj.mkdir()
    old = proj / f"{_UUID}.jsonl"
    old.write_text(_json.dumps({"type": "user", "uuid": "a", "message": {"content": "primeiro pedido"}}) + "\n")
    new = proj / "deadbeef-0000-0000-0000-000000000000.jsonl"
    new.write_text("{}\n")
    os.utime(old, (1000, 1000))
    os.utime(new, (2000, 2000))  # mais recente -> vem primeiro
    reg = SessionRegistry(projects_dir=tmp_path)
    with patch.object(registry.tmux, "list_panes_active",
                      return_value=[{"name": "cc", "pid": 111, "cwd": cwd}]), \
         patch.object(registry, "_config_dir_of", return_value=None), \
         patch.object(reg, "_cwd_has_siblings", return_value=True), \
         patch.object(reg, "list", return_value=[]):
        resolved_cwd, ambiguous, cands = reg.resume_candidates("cc")
    assert resolved_cwd == cwd and ambiguous is True
    assert cands[0]["session_id"] == "deadbeef-0000-0000-0000-000000000000"  # mais recente
    assert cands[1]["session_id"] == _UUID and cands[1]["preview"] == "primeiro pedido"


def test_marker_by_pids_matches_descendant_and_newest(tmp_path):
    # Marcador do hook casado por PID (sessao bare): pid descendente casa; o ts mais novo vence;
    # jsonl excluido (subagente) cai pro proximo candidato.
    import json as _json
    import os as _os
    from app.registry import _marker_by_pids
    d = tmp_path / ".claude-pocket-active"
    d.mkdir()
    j1 = tmp_path / "a.jsonl"; j1.write_text("x", encoding="utf-8")
    j2 = tmp_path / "b.jsonl"; j2.write_text("x", encoding="utf-8")
    (d / "k1.json").write_text(_json.dumps({"jsonl": str(j1), "ts": 1.0, "pid": 42}), encoding="utf-8")
    (d / "k2.json").write_text(_json.dumps({"jsonl": str(j2), "ts": 2.0, "pid": 42}), encoding="utf-8")
    (d / "k3.json").write_text(_json.dumps({"jsonl": str(j2), "ts": 9.0, "pid": 99}), encoding="utf-8")
    assert _marker_by_pids(tmp_path, [42], set()) == str(j2)      # mais recente do pid 42
    assert _marker_by_pids(tmp_path, [7], set()) is None          # pid nao casa
    assert _marker_by_pids(tmp_path, [42], {_os.path.realpath(str(j2))}) == str(j1)  # exclusao


def test_label_cache_preenche_working_e_nunca_idle(monkeypatch):
    # Barrinha do card: marker "working" sem label -> preenche do cache; marker idle -> NUNCA
    # serve label (e derruba a entrada: spinner cacheado e do turno passado — label fantasma).
    import time as _t
    reg = SessionRegistry()
    mk = {"sidw": ("working", 1.0), "sidi": ("idle", 1.0)}
    infos = [
        type("I", (), {"name": "w", "cwd": "/p", "jsonl": "/x/sidw.jsonl", "state": "idle", "last_activity": None})(),
        type("I", (), {"name": "i", "cwd": "/p", "jsonl": "/x/sidi.jsonl", "state": "idle", "last_activity": None})(),
    ]
    monkeypatch.setattr(reg, "list", lambda: infos)
    monkeypatch.setattr(hs_mod.hook_state, "get_state", lambda sid: mk.get(sid))
    SessionRegistry._status_cache["w"] = (_t.monotonic(), None)
    SessionRegistry._status_cache["i"] = (_t.monotonic(), None)
    SessionRegistry._label_cache["w"] = "Hyperspacing…"
    SessionRegistry._label_cache["i"] = "Fantasma…"
    out = asyncio.run(reg.list_with_state())
    by = {o.name: o for o in out}
    assert by["w"].label == "Hyperspacing…"                 # working herda a barrinha do cache
    assert getattr(by["i"], "label", None) is None          # idle nunca ganha label
    assert "i" not in SessionRegistry._label_cache          # e a entrada fantasma morre no marker
