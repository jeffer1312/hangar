"""Fatos da lista que continuam do Python: o Rust descobre e classifica as linhas Claude e pergunta
o resto a cada produção (`POST /internal/list/facts`)."""
import asyncio
import json

import pytest

from app import hook_state, list_facts, plugin_bridge, registry, sse


@pytest.fixture(autouse=True)
def _no_tmux(monkeypatch):
    # Os terminais de atalho leem o tmux do usuário; aqui não interessam.
    monkeypatch.setattr(sse, "_shortcuts_snapshot", lambda: "[]")


def _row(name, provider="claude", **extra):
    return {"name": name, "provider": provider, "state": "idle", **extra}


def _compute(rows, owner_clients=0, pane_pids=None):
    return asyncio.run(list_facts.compute(rows, owner_clients, pane_pids or {}))


def test_facts_compute_only_non_migrated_rows(monkeypatch):
    seen = []

    async def fake_state(infos, state_only=False):
        seen.append(([i.name for i in infos], state_only))
        for i in infos:
            i.state, i.label = "working", f"rótulo {i.name}"
            if i.provider == "codex":
                # O snapshot ao vivo do Codex vence o sidecar (`registry.list_with_state`).
                i.codex_service_tier = "priority"
        return infos

    monkeypatch.setattr(sse._list_registry, "list_with_state", fake_state)
    out = _compute([_row("cc"), _row("cx", "codex", codex_service_tier="default"), _row("pp", "pi"),
                    _row("hl", headless=True)])
    # As Claude (com e sem terminal) são do Rust: nem chegam ao classificador do Python.
    assert seen == [(["cx", "pp"], True)]
    assert set(out["states"]) == {"cx", "pp"}
    assert out["states"]["cx"]["state"] == "working"
    assert out["states"]["cx"]["label"] == "rótulo cx"
    assert out["states"]["cx"]["codex_service_tier"] == "priority"


def test_facts_account_for_kimi_pi_omp_rows(monkeypatch):
    from app import cotas, pi_models

    async def keep(infos, state_only=False):
        return infos

    monkeypatch.setattr(sse._list_registry, "list_with_state", keep)
    monkeypatch.setattr(cotas, "provider_padrao_kimi", lambda: "apikey")
    seen = []

    def atual(jsonl, cfg):
        seen.append((jsonl, cfg))
        return "kimi-coding"

    monkeypatch.setattr(pi_models, "provider_atual", atual)
    monkeypatch.setattr(cotas, "conta_de_provider_pi", lambda p: f"chave:{p}" if p else None)
    monkeypatch.setattr(registry, "_config_dir_of", lambda pid: f"/cfg/{pid}")
    out = _compute([_row("k", "kimi"), _row("p", "pi", jsonl="/t/p.jsonl"), _row("o", "omp"),
                    _row("cx", "codex", conta="codex:/h")], pane_pids={"p": 41})
    assert out["states"]["k"]["conta"] == "kimi:apikey"
    assert out["states"]["p"]["conta"] == "chave:kimi-coding"
    # O sidecar do catálogo mora na conta do pane da sessão.
    assert seen == [("/t/p.jsonl", "/cfg/41")]
    # Sem transcript não há catálogo: a conta fica vazia, como hoje.
    assert out["states"]["o"]["conta"] is None
    assert "conta" not in out["states"]["cx"]


def test_facts_transfer_overrides_by_name(monkeypatch):
    def fake_transfers(infos):
        for i in infos:
            if i.name == "fim":
                i.transfer_id, i.transfer_phase = "t1", "complete"
        infos.append(registry.SessionInfo(name="nova", provider="claude", transfer_id="t2",
                                          transfer_phase="prepared", cwd="/w"))
        infos[:] = [i for i in infos if i.name != "troca"] + [
            registry.SessionInfo(name="troca", provider="claude", transfer_id="t3",
                                 transfer_phase="copying", problema="x")]

    monkeypatch.setattr(registry, "_decorate_transfers", fake_transfers)
    out = _compute([_row("fim"), _row("troca", "codex"), _row("quieta")])
    by_name = {r["name"]: r for r in out["overrides"]}
    assert set(by_name) == {"fim", "nova", "troca"}
    assert by_name["troca"]["provider"] == "claude" and by_name["troca"]["problema"] == "x"
    # Fase terminal volta a ser classificada; troca em curso, não.
    assert sorted(out["frozen"]) == ["nova", "troca"]


def test_facts_leave_the_browser_request_to_rust(monkeypatch, tmp_path):
    # O pedido de navegador é do Rust (`nav_requests.rs`): os fatos não o levam mais.
    monkeypatch.setattr(sse, "_nav_arquivo", lambda: tmp_path / "_pendentes.json")
    sse._NAV_MARCADORES.clear()
    sse.nav_pendente("viva", "http://a")
    out = _compute([])
    assert "nav" not in out
    json.dumps(out)
    sse._NAV_MARCADORES.clear()


def test_demote_service_updates_map_and_file(tmp_path, monkeypatch):
    state_dir = tmp_path / ".hangar-state"
    state_dir.mkdir()
    sidecar = state_dir / "sid1.json"
    sidecar.write_text(json.dumps({"state": "awaiting_input", "ts": 5.0}))
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    monkeypatch.setattr(hook_state, "hook_state", hs)
    assert hs.get_state("sid1")[0] == "awaiting_input"
    for sid in ["sid1", "ausente"]:
        hs.demote_awaiting(sid)
    assert hs.get_state("sid1") == ("idle", 5.0)
    assert json.loads(sidecar.read_text()) == {"state": "idle", "ts": 5.0}


def test_facts_shadow_attaches_python_signatures(monkeypatch):
    """Sombra (`CP_LIST_SHADOW=1` no Rust): a lista é do Python, que manda a assinatura de cada
    linha que serviu. Nada de estado recalculado nem presença do app mexida."""
    async def never(infos, state_only=False):
        raise AssertionError("a sombra não reclassifica: o estado vem da lista que o Python serviu")

    monkeypatch.setattr(sse._list_registry, "list_with_state", never)
    monkeypatch.setattr(plugin_bridge, "app_remoto", lambda n: (_ for _ in ()).throw(AssertionError("presença")))
    served = [registry.SessionInfo(name="cx", provider="codex", state="working", label="lendo", tracked=False,
                                   conta="codex:/h", status_line="🤖 GPT │ 💬 1k/2k 50k/200k"),
              registry.SessionInfo(name="cc", provider="claude", state="idle", label="",
                                   context={"used": 100_000, "window": 200_000})]
    monkeypatch.setattr(sse, "recent_list", lambda max_age: served)
    out = asyncio.run(list_facts.compute([_row("cx", "codex"), _row("cc")], 0, {}, shadow=True))
    assert out["states"]["cx"]["state"] == "working" and out["states"]["cx"]["conta"] == "codex:/h"
    sig = out["shadow"]
    assert set(sig) == {"cx", "cc"}
    assert set(sig["cc"]) == set(list_facts.SIG_FIELDS)
    # A mesma redução do `_list_sig`: rótulo do Codex solto é o texto, o resto só presença.
    assert sig["cx"]["label"] == "lendo" and sig["cc"]["label"] is False
    assert sig["cx"]["status_line"] == ("GPT", 5, None, None, None)
    assert sig["cc"]["context"] == 10
    assert "last_activity" not in sig["cc"]
    json.dumps(out)


def test_facts_shadow_without_fresh_python_list(monkeypatch):
    monkeypatch.setattr(sse, "recent_list", lambda max_age: None)
    out = asyncio.run(list_facts.compute([_row("cc")], 0, {}, shadow=True))
    # Sem cliente o refresher para: nada a comparar, e o Rust espera mais antes de tentar de novo.
    assert out["shadow"] is None


def test_facts_without_shadow_sends_null(monkeypatch):
    async def keep(infos, state_only=False):
        return infos

    monkeypatch.setattr(sse._list_registry, "list_with_state", keep)
    assert _compute([_row("cc")])["shadow"] is None


def test_facts_carry_the_owner_apps_python_serves(monkeypatch):
    # Decisão P24: lista e chat do dono pelo Connect (servidos aqui) também são o app presente no Rust.
    from app import plugin_bridge
    async def keep(infos, state_only=False):
        return infos

    monkeypatch.setattr(sse._list_registry, "list_with_state", keep)
    monkeypatch.setattr(plugin_bridge, "_apps_abertos", 2)
    assert _compute([_row("cc")])["owner_apps"] == 2


def test_shadow_signature_has_every_list_sig_field():
    # Campo novo no `_list_sig` sem par aqui deixaria a sombra cega para ele.
    info = registry.SessionInfo(name="x")
    assert len(json.loads(sse._list_sig([info]))[0]) == len(list_facts.SIG_FIELDS)
    assert set(list_facts.SIG_FIELDS) <= set(registry.SessionInfo.model_fields)


def test_facts_codex_without_snapshot_keeps_sidecar_tier(monkeypatch):
    async def keep(infos, state_only=False):
        return infos

    monkeypatch.setattr(sse._list_registry, "list_with_state", keep)
    out = _compute([_row("cx", "codex", codex_service_tier="priority")])
    assert out["states"]["cx"]["codex_service_tier"] == "priority"


def test_orq_failure_goes_as_a_fact_not_an_empty_list(monkeypatch):
    """Leitura das orquestrações que falha vira código nos fatos: o Rust fica com as da última
    resposta boa, marcadas, em vez de servir a lista sem elas calado."""
    def broken():
        raise OSError("disco")

    monkeypatch.setattr(registry.orq_runs, "active", broken)
    out = list_facts._files([], [], {})
    assert (out["orq"], out["orq_error"]) == ([], "orq_unreadable")
    monkeypatch.setattr(registry.orq_runs, "active", lambda: [])
    assert list_facts._files([], [], {})["orq_error"] is None


def test_facts_never_read_or_feed_the_python_plugin(monkeypatch):
    # Pergunta segurada e presença do app são do plugin no Rust: os fatos não consultam o do Python.
    def refuse(*args):
        raise AssertionError("plugin do Python consultado")
    monkeypatch.setattr(plugin_bridge, "pergunta_pendente", refuse)
    monkeypatch.setattr(plugin_bridge, "app_remoto", refuse)

    async def keep(infos, state_only=False):
        return infos

    monkeypatch.setattr(sse._list_registry, "list_with_state", keep)
    out = _compute([_row("ct"), _row("ch", headless=True), _row("cx", "codex")], owner_clients=2)
    assert out["held"] == {}, "o campo segue no contrato, vazio"


def test_one_session_without_snapshot_does_not_break_the_others(monkeypatch):
    from types import SimpleNamespace

    import app.adapters as adapters

    class Headless:
        def problema_de(self, name):
            if name == "subindo":
                raise RuntimeError("snapshot do runtime indisponível")
            return ("headless_turno_erro", "x") if name == "quebrada" else None

    monkeypatch.setattr(adapters, "get_adapter", lambda _key: Headless())
    infos = [SimpleNamespace(name=n, provider="claude", headless=True) for n in ("subindo", "quebrada", "boa")]
    # A sessão ainda sem retrato fica sem problema; as outras seguem com o delas.
    assert list_facts._problems(infos) == {"quebrada": "headless_turno_erro"}
