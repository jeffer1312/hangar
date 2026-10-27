"""Fase do grupo de orquestração `auto` (`runs.group_phase`), que o Rust pergunta para decidir se o
último membro sai e se o grupo `orq` sozinho dissolve: viva enquanto a execução vive, ilegível conta
como viva."""
import json

import pytest

from pathlib import Path
from orq_contract_client import runs, call


@pytest.fixture(autouse=True)
def root(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME",str(tmp_path))
    r=tmp_path/".hangar/orq"
    r.mkdir(parents=True)
    return r


def _run(root, gid: str, ended: bool = False) -> None:
    d = root / f"2026-09-28-{gid}"
    d.mkdir()
    (d / "orq.json").write_text(json.dumps({"arbiter": "arb", "repo": str(root), "auto": True}),
                                encoding="utf-8")
    evs = [{"tipo": "execucao_inicio", "ts": "2026-09-28T10:00:00-03:00", "gid": gid,
            "plano": "p.md", "branch": "main"}]
    if ended:
        evs.append({"tipo": "execucao_fim", "ts": "2026-09-28T11:00:00-03:00", "resultado": "ok"})
    (d / "eventos.jsonl").write_text("".join(json.dumps(e) + "\n" for e in evs), encoding="utf-8")


def test_phase_follows_the_run(root):
    _run(root, "g1")
    _run(root, "g2", ended=True)
    assert (runs.group_phase("g1"), runs.group_phase("g2"), runs.group_phase("g3")) == ("live", "ended", None)


def test_a_watchdog_restart_keeps_a_live_run_live(root, monkeypatch):
    # Restart=always leaves a window with no heartbeat; the run itself is still alive.
    # O arquivo de batimento está ausente: a vida da execução basta.
    _run(root, "g1")
    assert runs.group_phase("g1") == "live"


def test_unreadable_runs_root_is_unknown(root, caplog):
    root.chmod(0)
    try:
        assert runs.group_phase("g1") == "unknown"
    finally:
        root.chmod(0o755)
    assert "orq" in caplog.text


def test_an_unreadable_runs_root_is_warned_once(root, caplog):
    root.chmod(0)
    try:
        assert call("runs.phases",home=Path.home(),gids=["g"]*3) == ["unknown"]*3
    finally:
        root.chmod(0o755)
    assert len([r for r in caplog.records if "ilegíveis" in r.getMessage()]) == 1


def test_unreadable_events_are_unknown(root):
    _run(root, "g1", ended=True)
    ev = next(root.iterdir()) / "eventos.jsonl"
    ev.unlink()
    ev.mkdir()   # read_text raises IsADirectoryError: the run may be this group's, alive
    assert runs.group_phase("g1") == "unknown"
