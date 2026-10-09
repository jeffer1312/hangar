"""Contexto da sessão Claude lido do transcript, para quem não usa a statusline do Hangar."""
import json

from app import claude_context as cc


def _resposta(model="claude-opus-5-5", entrada=2, lido=0, escrito=0, lateral=False, tipo="assistant"):
    return {"type": tipo, "isSidechain": lateral, "message": {"model": model, "usage": {
        "input_tokens": entrada, "cache_read_input_tokens": lido, "cache_creation_input_tokens": escrito,
        "output_tokens": 50}}}


def _transcript(tmp_path, *linhas):
    p = tmp_path / "s.jsonl"
    p.write_text("".join(json.dumps(l) + "\n" for l in linhas) + '{"truncad', encoding="utf-8")
    return p


def test_usa_a_ultima_resposta_do_agente_principal(tmp_path):
    p = _transcript(tmp_path,
                    _resposta(lido=10_000),
                    _resposta(entrada=3, lido=90_000, escrito=2_000),
                    # Subagente e resposta sintética não são o contexto da conversa.
                    _resposta(lido=5, lateral=True),
                    _resposta(model="<synthetic>", entrada=0))
    assert cc.from_transcript(p, tmp_path) == {"used": 92_003, "window": 200_000}


def test_janela_de_1m_pelo_modelo_configurado_ou_pelo_uso(tmp_path):
    p = _transcript(tmp_path, _resposta(lido=90_000))
    (tmp_path / "settings.json").write_text(json.dumps({"model": "opus[1m]"}), encoding="utf-8")
    assert cc.from_transcript(p, tmp_path)["window"] == 1_000_000
    # Uso acima da janela padrão só cabe na de 1M, mesmo sem a configuração dizer.
    assert cc.window(250_000, tmp_path / "sem-config") == 1_000_000
    assert cc.window(150_000, tmp_path / "sem-config") == 200_000


def test_sem_resposta_ou_sem_arquivo_e_none(tmp_path):
    assert cc.from_transcript(_transcript(tmp_path, {"type": "user", "message": {"content": "oi"}}), tmp_path) is None
    assert cc.from_transcript(tmp_path / "nao-existe.jsonl", tmp_path) is None
    assert cc.from_transcript(None) is None


def test_measured_context_wins_without_a_1m_model_alias(tmp_path):
    p = _transcript(tmp_path, _resposta(entrada=4, lido=76_573, escrito=27))
    p.with_suffix(".context.json").write_text(json.dumps({"used": 76_604, "window": 1_000_000}), encoding="utf-8")
    assert cc.from_transcript(p, tmp_path, model="opus") == {"used": 76_604, "window": 1_000_000}


def test_invalid_measurement_keeps_transcript_fallback(tmp_path):
    p = _transcript(tmp_path, _resposta(lido=90_000))
    for value in (None, {"used": -1, "window": 1_000_000}, {"used": 90_002, "window": 0}):
        p.with_suffix(".context.json").write_text(json.dumps(value), encoding="utf-8")
        assert cc.from_transcript(p, tmp_path, model="opus[1m]") == {"used": 90_002, "window": 1_000_000}


def test_declared_window_wins_over_a_measurement_from_before_resume(tmp_path):
    p = _transcript(tmp_path, _resposta(lido=90_000))
    p.with_suffix(".context.json").write_text(json.dumps({"used": 90_002, "window": 1_000_000}), encoding="utf-8")
    assert cc.from_transcript(p, tmp_path, window_tokens=256_000) == {"used": 90_002, "window": 256_000}


def test_modelo_da_sessao_vence_o_da_conta(tmp_path):
    # O Hangar abre a sessão com `--model opus[1m]` e não mexe no settings.json da conta.
    (tmp_path / "settings.json").write_text(json.dumps({"model": "sonnet"}), encoding="utf-8")
    assert cc.window(100_000, tmp_path, model="opus[1m]") == 1_000_000
    (tmp_path / "settings.json").write_text(json.dumps({"model": "opus[1m]"}), encoding="utf-8")
    assert cc.window(100_000, tmp_path, model="sonnet") == 200_000
    # Janela declarada pelo motor (CLAUDE_CODE_MAX_CONTEXT_TOKENS) vence os dois.
    assert cc.window(100_000, tmp_path, model="opus[1m]", window_tokens=256_000) == 256_000


def test_registry_le_o_modelo_do_processo_e_do_sidecar(tmp_path, monkeypatch):
    from app import registry
    from app.models import SessionInfo
    p = _transcript(tmp_path, _resposta(lido=100_000))
    monkeypatch.setattr(registry, "_escolhas_status", lambda _sid: (None, None))
    monkeypatch.setattr(registry.procinfo, "_model_of", lambda _pid: ("opus[1m]", None))
    monkeypatch.setattr(registry.procinfo, "_env_var_of", lambda _pid, _nome: None)
    info = SessionInfo(name="s", jsonl=str(p), conta=f"claude:{tmp_path}")
    assert registry._claude_context(info, 4242) == {"used": 100_002, "window": 1_000_000}
    monkeypatch.setattr(registry.headless_sessions, "load",
                        lambda _n: {"model": "opus", "context_window": 400_000})
    sem_terminal = SessionInfo(name="s", jsonl=str(p), headless=True, conta=f"claude:{tmp_path}")
    assert registry._claude_context(sem_terminal, None)["window"] == 400_000


def test_modelo_em_uso_sai_da_resposta_da_abertura_ou_da_conta(tmp_path):
    (tmp_path / "settings.json").write_text(json.dumps({"model": "claude-fable-5-1[1m]"}), encoding="utf-8")
    # Sem resposta ainda (sessão nova ou depois do /clear): vale o da abertura, senão o da conta.
    assert cc.session_model(None, "opus[1m]", tmp_path) == "opus[1m]"
    assert cc.session_model(None, None, tmp_path) == "claude-fable-5-1[1m]"
    # Sessão de motor não cai na conta, que guarda o modelo da Anthropic.
    assert cc.session_model(None, None, tmp_path, engine=True) is None
    # A resposta vence os dois: é o que está respondendo, mesmo depois de um /model no terminal.
    assert cc.session_model("claude-sonnet-5-5", "opus[1m]", tmp_path) == "claude-sonnet-5-5"
    # O transcript não diz a variante: a data sai, e o [1m] volta quando o uso só cabe na de 1M ou
    # quando o modelo configurado é a variante de 1M da mesma família.
    assert cc.session_model("claude-haiku-4-5-20251001", None, tmp_path) == "claude-haiku-4-5"
    assert cc.session_model(None, "claude-haiku-4-5-20251001", tmp_path) == "claude-haiku-4-5"
    assert cc.session_model("claude-opus-5-5", None, tmp_path, used=250_000) == "claude-opus-5-5[1m]"
    assert cc.session_model("claude-opus-5-5", "opus[1m]", tmp_path) == "claude-opus-5-5[1m]"
    assert cc.session_model("claude-fable-5-1", None, tmp_path) == "claude-fable-5-1[1m]"
    assert cc.session_model("kimi-for-coding", None, tmp_path, used=250_000, engine=True) == "kimi-for-coding"


def test_leitura_devolve_contexto_e_modelo_juntos(tmp_path):
    p = _transcript(tmp_path, _resposta(model="claude-sonnet-5-5", lido=90_000), _resposta(model="<synthetic>", entrada=0))
    assert cc.read(p, tmp_path) == ({"used": 90_002, "window": 200_000}, "claude-sonnet-5-5")
    assert cc.read(tmp_path / "nao-existe.jsonl", tmp_path) == (None, None)


async def test_lista_traz_o_modelo_da_conta_logo_depois_do_clear(tmp_path, monkeypatch):
    import time
    from app import registry
    from app.models import SessionInfo
    from app.registry import SessionRegistry
    (tmp_path / "settings.json").write_text(json.dumps({"model": "claude-fable-5-1[1m]"}), encoding="utf-8")
    antes = _transcript(tmp_path, _resposta(model="claude-sonnet-5-5", lido=90_000))
    depois = tmp_path / "novo.jsonl"
    depois.write_text(json.dumps({"type": "user", "message": {"content": "oi"}}) + "\n", encoding="utf-8")
    reg = SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(SessionRegistry, "_context_cache", {})
    monkeypatch.setattr(SessionRegistry, "_status_cache", {"s": (time.monotonic(), None)})
    monkeypatch.setattr(registry, "_escolhas_status", lambda _sid: (None, None))
    monkeypatch.setattr(registry.hook_state, "get_state", lambda _sid: ("idle", 1.0))
    monkeypatch.setattr(registry, "pergunta_aberta", lambda _sid: None)
    info = SessionInfo(name="s", jsonl=str(antes), tracked=True, conta=f"claude:{tmp_path}")
    monkeypatch.setattr(reg, "list", lambda: [info])
    assert (await reg.list_with_state())[0].model == "claude-sonnet-5-5"
    info.jsonl, info.model = str(depois), None
    assert (await reg.list_with_state())[0].model == "claude-fable-5-1[1m]"


async def test_fim_sem_resposta_no_mesmo_transcript_mantem_o_modelo(tmp_path, monkeypatch):
    import time
    from app import registry
    from app.models import SessionInfo
    from app.registry import SessionRegistry
    (tmp_path / "settings.json").write_text(json.dumps({"model": "claude-fable-5-1[1m]"}), encoding="utf-8")
    p = _transcript(tmp_path, _resposta(model="claude-sonnet-5-5", lido=90_000))
    reg = SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(SessionRegistry, "_context_cache", {})
    monkeypatch.setattr(SessionRegistry, "_status_cache", {"s": (time.monotonic(), None)})
    monkeypatch.setattr(registry, "_escolhas_status", lambda _sid: (None, None))
    monkeypatch.setattr(registry.hook_state, "get_state", lambda _sid: ("idle", 1.0))
    monkeypatch.setattr(registry, "pergunta_aberta", lambda _sid: None)
    info = SessionInfo(name="s", jsonl=str(p), tracked=True, conta=f"claude:{tmp_path}")
    monkeypatch.setattr(reg, "list", lambda: [info])
    assert (await reg.list_with_state())[0].model == "claude-sonnet-5-5"
    # Um resultado de ferramenta enorme empurra a última resposta para fora do trecho lido: a pílula
    # não pode trocar para o modelo da conta enquanto o transcript é o mesmo.
    p.write_text(json.dumps({"type": "user", "message": {"content": "x" * 100}}) + "\n", encoding="utf-8")
    t, jsonl, ctx, model = SessionRegistry._context_cache["s"]
    SessionRegistry._context_cache["s"] = (0.0, jsonl, ctx, model)
    info.model = None
    assert (await reg.list_with_state())[0].model == "claude-sonnet-5-5"


async def test_clear_zera_o_contexto_ate_a_primeira_resposta(tmp_path, monkeypatch):
    import time
    from app import registry
    from app.models import SessionInfo
    from app.registry import SessionRegistry
    antes = _transcript(tmp_path, _resposta(lido=90_000))
    depois = tmp_path / "novo.jsonl"
    depois.write_text(json.dumps({"type": "user", "message": {"content": "oi"}}) + "\n", encoding="utf-8")
    reg = SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(SessionRegistry, "_context_cache", {})
    monkeypatch.setattr(SessionRegistry, "_status_cache", {"s": (time.monotonic(), None)})
    monkeypatch.setattr(registry, "_escolhas_status", lambda _sid: (None, None))
    monkeypatch.setattr(registry.hook_state, "get_state", lambda _sid: ("idle", 1.0))
    monkeypatch.setattr(registry, "pergunta_aberta", lambda _sid: None)
    info = SessionInfo(name="s", jsonl=str(antes), tracked=True, conta=f"claude:{tmp_path}")
    monkeypatch.setattr(reg, "list", lambda: [info])
    assert (await reg.list_with_state())[0].context == {"used": 90_002, "window": 200_000}
    # /clear: transcript novo, ainda sem resposta. O número da conversa anterior não vale mais.
    info.jsonl, info.context = str(depois), None
    assert (await reg.list_with_state())[0].context is None


async def test_measurement_during_read_does_not_freeze_old_python_context(tmp_path, monkeypatch):
    import time
    from app import registry
    from app.models import SessionInfo
    from app.registry import SessionRegistry
    transcript = _transcript(tmp_path, _resposta(lido=76_602))
    reg = SessionRegistry(projects_dir=tmp_path)
    monkeypatch.setattr(SessionRegistry, "_context_cache", {})
    monkeypatch.setattr(SessionRegistry, "_status_cache", {"s": (time.monotonic(), None)})
    monkeypatch.setattr(registry.hook_state, "get_state", lambda _sid: ("idle", 1.0))
    monkeypatch.setattr(registry, "pergunta_aberta", lambda _sid: None)
    info = SessionInfo(name="s", jsonl=str(transcript), tracked=True, conta=f"claude:{tmp_path}")
    monkeypatch.setattr(reg, "list", lambda: [info])

    def read_then_publish(_info, _pid):
        result = cc.read(transcript, tmp_path, model="opus")
        cc.publish(transcript, {"used": 76_604, "window": 1_000_000})
        return result

    monkeypatch.setattr(registry, "_claude_reading", read_then_publish)
    await reg.list_with_state()
    assert (await reg.list_with_state())[0].context == {"used": 76_604, "window": 1_000_000}
