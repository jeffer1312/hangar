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


def test_read_from_restarts_after_shrink(tmp_path):
    # Arquivo REESCRITO menor (truncamento): o offset antigo cairia alem do EOF e, quando o arquivo
    # voltasse a crescer, a leitura retomaria no meio de linha nova = lixo/eventos perdidos. O guard
    # detecta size < pos e recomeca do zero.
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "primeira mensagem bem comprida") + _user("u2", "segunda"))
    t = TranscriptTailer(f)
    _, pos = t._read_from(0)
    f.write_text(_user("u9", "novo"))          # rewrite menor que o pos antigo
    evs, _ = t._read_from(pos)
    assert [e.id for e in evs] == ["u9"]


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


# --- retomada exata via Last-Event-ID (offset por evento) -----------------------------------
# A janela de backfill (200 linhas) cobre so ~2 min de trabalho pesado — medido nesta sessao:
# mediana 44 linhas/min, pico 133. Uma queda de celular mais longa perdia o miolo do buraco.
# Cada evento agora carrega o offset da SUA linha, que vira o `id:` do SSE.

def test_read_from_marca_offset_do_inicio_da_linha(tmp_path):
    p = tmp_path / "t.jsonl"
    linhas = [_user("a", "um"), _user("b", "dois"), _user("c", "tres")]
    p.write_text("\n".join(linhas) + "\n", encoding="utf-8")
    evs, _ = TranscriptTailer(p)._read_from(0)
    assert [e.id for e in evs] == ["a", "b", "c"]
    # offset do INICIO da linha (nao do fim): retomar dali RELE a linha inteira, entao eventos
    # irmaos da mesma linha nunca somem — o front descarta o duplicado por id.
    esperado, acc = [], 0
    for ln in linhas:
        esperado.append(acc)
        acc += len(ln.encode()) + 1
    assert [e.offset for e in evs] == esperado


def test_read_from_offset_retoma_exatamente_dali(tmp_path):
    p = tmp_path / "t.jsonl"
    p.write_text("\n".join([_user("a", "um"), _user("b", "dois"), _user("c", "tres")]) + "\n",
                 encoding="utf-8")
    tailer = TranscriptTailer(p)
    evs, _ = tailer._read_from(0)
    # retoma no offset do 2o evento -> reentrega b e c, nada de a
    evs2, _ = tailer._read_from(evs[1].offset)
    assert [e.id for e in evs2] == ["b", "c"]


async def test_follow_ignora_offset_alem_do_fim(tmp_path, monkeypatch):
    # Transcript trocado/truncado sob o cliente: o offset antigo nao significa mais nada. Tem que
    # cair no backfill do tail, NAO retomar num ponto errado nem reler o arquivo inteiro.
    p = tmp_path / "t.jsonl"
    p.write_text(_user("a", "um") + "\n", encoding="utf-8")
    vistos = []

    async def consume(offset):
        async for ev in TranscriptTailer(p).follow(offset):
            vistos.append(ev.id)
            break

    await asyncio.wait_for(consume(10_000_000), timeout=5)
    assert vistos == ["a"]
