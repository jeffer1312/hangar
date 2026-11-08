"""Prende a ordem das áreas por ferramenta às três sementes de hash."""

from datetime import datetime


from app import costs, costs_cache as cc, uso_areas
from app.uso_claude import UsoLinha


class SavedAreasFold:
    def __init__(self, cwd):
        self.cwd = cwd

    def linha(self, raw):
        pass

    def fechar(self):
        regs = [("P", self.cwd, self.cwd, ("a.css", "a.py"))]
        units = [("d", self.cwd, "m", False, 11, 7, 5, 3, 2)]
        tool = UsoLinha(dia="d", cwd=self.cwd, model="m", tipo="tool", nome="Read", chamadas=1)
        cost = costs.UsageRow(ts=datetime.fromisoformat("2026-09-30T12:00:00-03:00"),
                             source="synthetic", provider="p", model="m", project=self.cwd,
                             session_id="s", input=11, output=7, cache_write=5, cache_read=3,
                             cache_write_1h=2)
        return [cost], [tool], ({}, [(regs, units)])


def test_division_signature_refolds_only_saved_areas(tmp_path, monkeypatch):
    monkeypatch.setattr(cc, "_CACHE_DIR", tmp_path / "cache")
    monkeypatch.setattr(uso_areas, "_arquivo", lambda: tmp_path / "areas.json")
    current_version = uso_areas._DIVISAO
    transcript = tmp_path / "synthetic.jsonl"
    transcript.write_bytes(b"synthetic\n")
    monkeypatch.setattr(uso_areas, "_DIVISAO", 2)
    uso_areas.recarregar()
    try:
        old_signature = uso_areas.assinatura()
        cc.sincronizar("synthetic", [transcript], lambda p: SavedAreasFold(str(tmp_path)), "v1")
        conn = cc._abrir()
        try:
            metadata = conn.execute("SELECT versao,dev,ino,size,mtime_ns,offset,cauda,estado,areas FROM files").fetchall()
            other_usage = conn.execute("SELECT * FROM uso WHERE tipo<>'area' ORDER BY rowid").fetchall()
            costs_before = conn.execute("SELECT * FROM custo ORDER BY rowid").fetchall()
            assert costs_before and other_usage
        finally:
            conn.close()
        monkeypatch.setattr(uso_areas, "_DIVISAO", current_version)
        uso_areas.recarregar()
        new_signature = uso_areas.assinatura()
        assert new_signature != old_signature

        reads = []

        def forbid_read(*args):
            reads.append(args)
            raise AssertionError("a troca de assinatura não pode reler o transcript")

        monkeypatch.setattr(cc, "_ler_novo", forbid_read)
        cc.sincronizar("synthetic", [transcript], forbid_read, "v1")
        assert reads == []
        conn = cc._abrir()
        try:
            assert conn.execute("SELECT versao,dev,ino,size,mtime_ns,offset,cauda,estado,areas FROM files").fetchall() == metadata
            assert conn.execute("SELECT * FROM uso WHERE tipo<>'area' ORDER BY rowid").fetchall() == other_usage
            assert conn.execute("SELECT * FROM custo ORDER BY rowid").fetchall() == costs_before
        finally:
            conn.close()
    finally:
        uso_areas.recarregar()
