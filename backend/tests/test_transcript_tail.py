import asyncio
import json

import pytest

# _BACKFILL_LINES importado de proposito: garante que a const existe (o test monkeypatcha por
# string path); noqa pq nao e referenciado por nome aqui.
from app.transcript import TranscriptTailer, _BACKFILL_LINES  # noqa: F401


def _user(uid: str, text: str) -> str:
    return json.dumps({"type": "user", "uuid": uid,
                       "message": {"role": "user", "content": text}}) + "\n"


def test_tail_offset_zero_when_few_lines(tmp_path):
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("u2", "b"))
    # <= max_lines linhas -> backfill do inicio (offset 0): sessao curta mantem o backfill completo.
    assert TranscriptTailer(f)._tail_offset(10) == 0


def test_tail_offset_returns_kth_from_last(tmp_path):
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("u2", "b") + _user("u3", "c"))
    t = TranscriptTailer(f)
    pos = t._tail_offset(2)                 # so as 2 ultimas linhas (u2, u3)
    evs, _ = t._read_from(pos)
    assert [e.id for e in evs] == ["u2", "u3"]


def test_tail_offset_ignores_partial_last_line(tmp_path):
    f = tmp_path / "s.jsonl"
    # ultima linha sem \n = append em voo: nao conta nem desloca o tail (espelha _read_from).
    f.write_text(_user("u1", "a") + _user("u2", "b") + '{"type":"user","uuid":"u3"')
    t = TranscriptTailer(f)
    pos = t._tail_offset(1)                 # 2 linhas completas; parcial ignorada -> tail = u2
    evs, _ = t._read_from(pos)
    assert [e.id for e in evs] == ["u2"]


def test_tail_offset_missing_file_is_zero(tmp_path):
    assert TranscriptTailer(tmp_path / "nope.jsonl")._tail_offset(5) == 0


@pytest.mark.asyncio
async def test_follow_backfills_only_tail(tmp_path, monkeypatch):
    monkeypatch.setattr("app.transcript._BACKFILL_LINES", 2)
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("u2", "b") + _user("u3", "c"))
    got: list[str] = []

    async def consume():
        async for ev in TranscriptTailer(f).follow():
            got.append(ev.id)
            if len(got) == 2:
                return

    await asyncio.wait_for(consume(), timeout=5)
    assert got == ["u2", "u3"]             # u1 (fora do tail) NAO veio no backfill
