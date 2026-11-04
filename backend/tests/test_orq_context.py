"""Configuração por contexto, associação ao grupo e concorrência sem herdar outro time."""
import json
from types import SimpleNamespace

import pytest

from pathlib import Path
from app import pair
from orq_contract_client import contexts as oc, markdown as orq_md, roles, call


@pytest.fixture(autouse=True)
def isolated(tmp_path, monkeypatch):
    monkeypatch.setattr(pair, "_pair_dir", lambda: tmp_path)
    monkeypatch.setattr(pair, "_arquivo_dir", lambda: tmp_path / "archive")
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("HANGAR_PAIR_ARCHIVE", str(tmp_path / "archive"))
    for name in ("a", "b", "same", "planner", "arbiter", "legacy"):
        _identity(name, "identity:" + name)


def _identity(name, key, session_id="initial"):
    directory = Path.home() / ".hangar" / "claude-headless"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{name}.json").write_text(json.dumps({"key":key,"session_id":session_id}))


def _orq_sidecar(name, gid="real"):
    """Membro do grupo `orq` como o Rust o grava."""
    path = pair._pair_dir() / f"{name}.json"
    path.write_text(json.dumps({"peers": [], "task": "mobile", "gid": gid, "harness": {}, "orq": True}))
    return path


def save(name):
    context = oc.resolve(name)
    text = roles.escrever_papel("", roles.Papel("executor", "mobile-t*", "codex", "chosen", "", "",
                                                "2", headless=True, abertura_extra="--sem-jev"))
    return context, oc.write(context, text, 0.0)


def test_two_contexts_ignore_default_and_old_contract():
    roles.regras_path("padrao").write_text("default", encoding="utf-8")
    roles.regras_path("old").write_text(roles.escrever_papel(
        "", roles.Papel("executor", "a", "claude", "old", "opus", "high")), encoding="utf-8")
    a, _ = save("a")
    b = oc.resolve("b")
    assert a.gid != b.gid and not b.grouped
    assert orq_md.ler_arquivo(b.path) == ("", 0.0)
    assert roles.ler(a.path.read_text())[0].conta == "chosen"
    assert oc.resolve("a").path == a.path


def test_recreated_name_does_not_inherit_draft(monkeypatch):
    old, _ = save("same")
    _identity("same", "another-life")
    new = oc.resolve("same")
    assert new.gid != old.gid
    assert orq_md.ler_arquivo(new.path) == ("", 0.0)


def test_promotion_preserves_record_and_redirects_source():
    context, mtime = save("planner")
    before = context.path.read_text()
    _orq_sidecar("arbiter")
    oc.promote("planner", "real", mtime)
    grouped = oc.resolve("planner")
    assert grouped.grouped and grouped.gid == "real" and grouped.session_prefix == "planner"
    assert grouped.path.read_text() == before
    assert roles.ler(context.path.read_text()) == []
    assert oc.resolve("planner").path == oc.resolve("arbiter").path == grouped.path
    # Origem e árbitro editam a mesma tabela, com o mtime do registro original.
    text = roles.escrever_papel(before, roles.Papel("revisor", "mobile-review*", "claude", "selected", "", ""))
    oc.write(grouped, text, mtime)
    assert [p.conta for p in roles.ler(oc.resolve("planner").path.read_text())] == ["chosen", "selected"]


def test_promotion_requires_fresh_mtime():
    context, mtime = save("planner")
    _orq_sidecar("arbiter")
    with pytest.raises(orq_md.Conflito):
        oc.promote("planner", "real", 0.0)
    assert context.path.is_file() and not roles.regras_path("real").exists()


def test_competing_contract_is_not_overwritten():
    context, mtime = save("planner")
    target = roles.regras_path("real")
    target.write_text("another contract", encoding="utf-8")
    _orq_sidecar("arbiter")
    with pytest.raises(ValueError, match="outro contrato"):
        oc.promote("planner", "real", mtime)
    assert target.read_text() == "another contract" and context.path.is_file()


def test_write_started_before_promotion_conflicts():
    context, mtime = save("planner")
    text = context.path.read_text()
    _orq_sidecar("arbiter")
    oc.promote("planner", "real", mtime)
    with pytest.raises(orq_md.Conflito):
        oc.write(context, text, mtime)


def test_archived_group_is_not_treated_as_current():
    context, mtime = save("planner")
    link = _orq_sidecar("arbiter")
    oc.promote("planner", "real", mtime)
    link.unlink()
    pair._arquivo_dir().mkdir()
    roles.regras_path("real").rename(pair._arquivo_dir() / "regras-real-20261019-120000.md")
    fresh = oc.resolve("planner")
    assert fresh.gid == context.gid and not fresh.grouped
    assert roles.ler(fresh.path.read_text()) == []
    _, current_mtime = orq_md.ler_arquivo(fresh.path)
    oc.write(fresh, roles.escrever_papel(fresh.path.read_text(), roles.Papel(
        "executor", "new-t*", "claude", "new-choice", "", "")), current_mtime)
    assert oc.resolve("planner").path == fresh.path
    assert roles.ler(fresh.path.read_text())[0].conta == "new-choice"


def test_unknown_group_phase_does_not_clear_record(monkeypatch):
    context, mtime = save("planner")
    link = _orq_sidecar("arbiter")
    oc.promote("planner", "real", mtime)
    link.unlink()
    # Sem vínculo ou arquivo confirmado, a promoção continua sem encerramento comprovado.
    with pytest.raises(oc.PromotionConflict, match="confirmar o encerramento"):
        oc.resolve("planner")
    assert "hangar-orq-group" in context.path.read_text()


def test_promotion_conflict_preserves_external_source_and_rolls_back_group(monkeypatch):
    context, _ = save("planner")
    before = context.path.read_text()
    with pytest.raises(oc.PromotionConflict, match="origem não restaurada"):
        call("context.promote-failure", source=context.path, destination=context.path.parent/"regras-fresh.md", external=True)
    assert context.path.read_text() == "external editor"
    destinations = [p for p in context.path.parent.glob("regras-*.md") if p != context.path]
    assert len(destinations) == 1 and destinations[0].read_text() == before


def test_promotion_failure_restores_source_exclusively(monkeypatch):
    context, _ = save("planner")
    before = context.path.read_text()
    with pytest.raises(oc.PromotionConflict, match="origem restaurada"):
        call("context.promote-failure", source=context.path, destination=context.path.parent/"regras-fresh.md", external=False)
    assert context.path.read_text() == before


def test_associate_facts_carry_what_the_rust_needs(monkeypatch):
    # Os fatos intermediários saíram: o Rust lê os mesmos arquivos no caminho da associação.
    context, _ = save("planner")
    context.path.write_text("<!-- hangar-orq-group: old -->\n", encoding="utf-8")
    pair._arquivo_dir().mkdir()
    (pair._arquivo_dir() / "regras-old-20261019-120000.md").write_text("x", encoding="utf-8")
    assert not oc.resolve("planner").grouped
    _orq_sidecar("planner")
    (Path.home()/".hangar/claude-headless/planner.json").unlink()
    resolved=oc.resolve("planner")
    assert resolved.grouped and resolved.gid == "real"


def test_actual_pair_link_preserved_without_legacy_identity(monkeypatch):
    _orq_sidecar("legacy")
    (Path.home()/".hangar/claude-headless/legacy.json").unlink()
    context = oc.resolve("legacy")
    assert context.gid == "real" and context.grouped


def test_new_orq_group_promotes_founder_record():
    # O que o Rust faz quando o grupo `orq` nasce: grava o sidecar e pede `/internal/orq/promote`.
    context, _ = save("planner")
    _orq_sidecar("planner", "fresh")
    oc.promote("planner", "fresh")
    grouped = oc.resolve("planner")
    assert grouped.grouped and grouped.gid == "fresh"
    assert roles.ler(grouped.path.read_text())[0].conta == "chosen"
    assert roles.ler(context.path.read_text()) == []


def test_sidecar_identity_survives_clear_and_changes_when_recreated(monkeypatch):
    _identity("same", "first", "before")
    first = oc.identity("same")
    _identity("same", "first", "after")
    assert oc.identity("same") == first
    _identity("same", "second", "after")
    assert oc.identity("same") != first


def test_tmux_identity_survives_name_change_and_rejects_recycled_server(monkeypatch):
    first=call("identity.tmux", pid=42, started=1.0, session="$1", created="100")
    assert call("identity.tmux",pid=42,started=1.0,session="$1",created="100") == first
    assert call("identity.tmux",pid=42,started=2.0,session="$1",created="100") != first
