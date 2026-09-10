import json

from app import codex_contas, costs_sources as cs


def test_codex_cache_rele_so_rollout_alterado_e_remove_arquivo_ausente(tmp_path, monkeypatch):
    monkeypatch.setattr(cs, "_config_dirs", lambda: [])
    for name in ("raiz_pi", "raiz_omp", "raiz_kimi"):
        monkeypatch.setattr(cs, name, lambda: tmp_path / "ausente")
    account = codex_contas.Account("default", tmp_path, True)
    sessions = tmp_path / "sessions"
    sessions.mkdir()
    paths = {sessions / "rollout-a.jsonl", sessions / "rollout-b.jsonl"}
    monkeypatch.setattr(cs, "_rollouts_codex_por_conta", lambda _: {
        account.id: (account, {p for p in paths if p.exists()})})
    monkeypatch.setattr(cs, "_contas_codex", lambda: [account])
    calls = []
    original = cs._linhas_rollout_codex

    def read(path, account_id):
        calls.append(path)
        return original(path, account_id)

    monkeypatch.setattr(cs, "_linhas_rollout_codex", read)

    def write(path, tokens):
        events = [
            {"type": "session_meta", "payload": {"id": path.stem, "cwd": "/project"}},
            {"type": "turn_context", "payload": {"model": "gpt-5.6-sol"}},
            {"type": "event_msg", "timestamp": "2026-09-10T12:00:00Z", "payload": {
                "type": "token_count", "info": {"total_token_usage": {
                    "input_tokens": tokens, "cached_input_tokens": 0, "output_tokens": 2}}}},
        ]
        path.write_text("\n".join(json.dumps(e) for e in events) + "\n", encoding="utf-8")

    a, b = sorted(paths)
    write(a, 10)
    write(b, 20)
    cs.invalidar_cache()
    try:
        assert sum(r.input for r in cs.coletar()) == 30
        assert sorted(calls) == [a, b]
        calls.clear()
        assert sum(r.input for r in cs.coletar()) == 30
        assert calls == []
        write(a, 100)
        assert sum(r.input for r in cs.coletar()) == 120
        assert calls == [a]
        calls.clear()
        b.unlink()
        assert sum(r.input for r in cs.coletar()) == 100
        assert calls == []
        write(b, 20)
        assert sum(r.input for r in cs.coletar()) == 120
        assert calls == [b]
    finally:
        cs.invalidar_cache()
