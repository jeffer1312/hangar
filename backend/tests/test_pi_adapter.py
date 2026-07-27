from app.adapters import get_adapter
from app.adapters.pi.adapter import PiAdapter


def test_pi_is_registered_as_a_provider():
    a = get_adapter("pi")
    assert isinstance(a, PiAdapter)
    assert a.provider == "pi"


def test_spawn_command_pins_the_session_id():
    # Sem --session-id o Pi inventa um id e o backend perde o transcript — o mesmo "sem id" que
    # deixa uma sessao Claude invisivel pro app.
    assert PiAdapter().spawn_command("/w", "abc") == ["pi", "--session-id", "abc"]


def test_adapter_satisfies_the_protocol_surface():
    # O Protocol nao e checado em runtime; sem este teste um metodo faltando so aparece quando o
    # SSE quebra em producao.
    a = PiAdapter()
    for m in ("transcript_stream", "state_monitor", "drain", "send_prompt",
              "deliverable", "spawn_command", "transcript_path"):
        assert callable(getattr(a, m)), m


def test_transcript_stream_actually_parses_pi_lines(tmp_path):
    # Os outros testes so checam `callable`, entao um kwarg errado no TranscriptTailer fecha a task
    # VERDE e so quebra quando o usuario abre o chat. Este consome o stream de verdade.
    import asyncio, json

    f = tmp_path / "2026-01-01T00-00-00-000Z_sid.jsonl"
    f.write_text(json.dumps({"type": "message", "id": "n1", "message": {
        "role": "user", "timestamp": 1785160179834,
        "content": [{"type": "text", "text": "oi"}]}}) + "\n", encoding="utf-8")

    async def first():
        agen = PiAdapter().transcript_stream(str(f))
        try:
            return await asyncio.wait_for(agen.__anext__(), timeout=5)
        finally:
            await agen.aclose()

    ev = asyncio.run(first())
    assert ev.kind == "user_msg" and ev.text == "oi"


def test_transcript_path_delegates_to_sessions(monkeypatch, tmp_path):
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path))
    from app.adapters.pi import sessions as s
    d = tmp_path / s.cwd_slug("/w")
    d.mkdir(parents=True)
    f = d / "2026-01-01T00-00-00-000Z_zzz.jsonl"
    f.write_text("")
    assert PiAdapter().transcript_path("/w", "zzz") == str(f)
