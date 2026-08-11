import json

from app.adapters.kimi import sessions as ks


def test_workdir_key_matches_measured_layout():
    # Medido no Kimi 0.34.0: wd_<basename>_<sha256(cwd)[:12]>.
    # /home/jefferson/Projetos/hangar -> wd_hangar_5112ff7a84e0 (dir real em ~/.kimi-code/sessions)
    assert ks.workdir_key("/home/jefferson/Projetos/hangar") == "wd_hangar_5112ff7a84e0"
    assert ks.workdir_key("/tmp/kimi-acp-probe") == "wd_kimi-acp-probe_15ca61fc9ec9"


def test_transcript_path_via_session_index(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    sdir = tmp_path / "sessions" / "wd_x_abcd" / "session_11111111-2222-3333-4444-555555555555"
    wire = sdir / "agents" / "main" / "wire.jsonl"
    wire.parent.mkdir(parents=True)
    wire.write_text("")
    (tmp_path / "session_index.jsonl").write_text(
        json.dumps({"sessionId": "session_11111111-2222-3333-4444-555555555555",
                    "sessionDir": str(sdir), "workDir": "/x"}) + "\n", encoding="utf-8")
    assert ks.transcript_path("/x", "session_11111111-2222-3333-4444-555555555555") == str(wire)


def test_transcript_path_fallback_computed_key(tmp_path, monkeypatch):
    # Sem entrada no indice (janela entre o bilhete do hook e o flush do session_index): cai na
    # chave computada. Sem o DIRETORIO da sessao -> "" (sessao ainda nao existe, nao e erro).
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    sid = "session_aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    assert ks.transcript_path("/w", sid) == ""
    sdir = tmp_path / "sessions" / ks.workdir_key("/w") / sid
    sdir.mkdir(parents=True)
    assert ks.transcript_path("/w", sid) == str(sdir / "agents" / "main" / "wire.jsonl")


def test_is_subagent_wire_and_root():
    main = "/h/sessions/wd_x_y/session_123/agents/main/wire.jsonl"
    sub = "/h/sessions/wd_x_y/session_123/agents/agent-0/wire.jsonl"
    assert not ks.is_subagent_wire(main)
    assert ks.is_subagent_wire(sub)
    assert ks.root_wire(sub) == "/h/sessions/wd_x_y/session_123/agents/main/wire.jsonl"
    assert ks.root_wire(main) == ""


def test_pretrust_writes_workspace_trust_once(tmp_path, monkeypatch):
    monkeypatch.setenv("KIMI_CODE_HOME", str(tmp_path))
    ks.pretrust_cwd("/tmp/kimi-acp-probe")
    f = tmp_path / "workspace-trust" / "wd_kimi-acp-probe_15ca61fc9ec9"
    assert f.is_file()
    data = json.loads(f.read_text())
    assert data["root"] == "/tmp/kimi-acp-probe"
    assert isinstance(data["trustedAt"], int)
    # Segunda chamada NAO reescreve (preserva o trustedAt original do CLI).
    f.write_text('{"root":"/tmp/kimi-acp-probe","trustedAt":1}')
    ks.pretrust_cwd("/tmp/kimi-acp-probe")
    assert json.loads(f.read_text())["trustedAt"] == 1
