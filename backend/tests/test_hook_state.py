import json, time
from pathlib import Path
from app import hook_state


def _write(d: Path, sid: str, state: str):
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{sid}.json").write_text(json.dumps({"state": state, "ts": time.time()}))


def test_load_existing_seeds_map(tmp_path):
    sd = tmp_path / ".claude-pocket-state"
    _write(sd, "aaa", "working")
    _write(sd, "bbb", "idle")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "working"
    assert hs.get_state("bbb")[0] == "idle"


def test_get_state_none_when_absent(tmp_path):
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("missing") is None


def test_apply_updates_existing(tmp_path):
    sd = tmp_path / ".claude-pocket-state"
    _write(sd, "aaa", "working")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    _write(sd, "aaa", "idle")            # state flips
    hs._apply(sd / "aaa.json")
    assert hs.get_state("aaa")[0] == "idle"


def test_apply_ignores_bad_json(tmp_path):
    sd = tmp_path / ".claude-pocket-state"; sd.mkdir(parents=True)
    (sd / "x.json").write_text("{ not json")
    hs = hook_state.HookState()
    hs._apply(sd / "x.json")             # no raise
    assert hs.get_state("x") is None


# 13/08/2026: uma sessao Kimi apareceu "pronta" na lista e no chat por 18 minutos
# enquanto escrevia codigo. O marcador do hook estava congelado em idle desde as 08:38:35 — no Kimi,
# um turno que comeca a partir de um prompt ENFILEIRADO na TUI nao dispara UserPromptSubmit nem
# TurnStarted, e o pane tambem nao salva (o spinner e fase de lua, fora de SPINNER_GLYPHS). O
# transcript crescendo e a unica prova de vida.
def test_corrige_ocioso_kimi(tmp_path):
    import os
    from app.state import corrige_ocioso_kimi

    wire = tmp_path / "wire.jsonl"
    wire.write_text("{}\n", encoding="utf-8")

    def com_mtime(mt):
        os.utime(wire, (mt, mt))
        return str(wire)

    # Turno andando: wire escrito DEPOIS do marcador ocioso.
    assert corrige_ocioso_kimi(("idle", 1000.0), com_mtime(1090.0)) == ("working", 1090.0)

    # Ociosa de verdade: medido em 18 sessoes reais, o Stop chega no MESMO segundo da ultima linha.
    assert corrige_ocioso_kimi(("idle", 1000.0), com_mtime(1000.0)) == ("idle", 1000.0)
    assert corrige_ocioso_kimi(("idle", 1000.0), com_mtime(1001.5)) == ("idle", 1000.0)  # na folga
    assert corrige_ocioso_kimi(("idle", 1000.0), com_mtime(999.0)) == ("idle", 1000.0)   # mais velho

    # So mexe em idle: awaiting_input pertence ao pane (a pergunta so existe la) e working ja esta
    # certo — promover qualquer um dos dois aqui seria inventar estado.
    alto = com_mtime(9000.0)
    assert corrige_ocioso_kimi(("awaiting_input", 1000.0), alto) == ("awaiting_input", 1000.0)
    assert corrige_ocioso_kimi(("working", 1000.0), alto) == ("working", 1000.0)

    # Sem marcador, sem caminho, ou caminho que nao existe: nunca inventa um estado.
    assert corrige_ocioso_kimi(None, alto) is None
    assert corrige_ocioso_kimi(("idle", 1000.0), None) == ("idle", 1000.0)
    assert corrige_ocioso_kimi(("idle", 1000.0), str(tmp_path / "sumiu.jsonl")) == ("idle", 1000.0)
