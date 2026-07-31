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


# --- tail-read reverso: mesmo resultado da varredura ingenua ---------------------------------
# _tail_offset le do FIM pra tras (janela que cresce) em vez de varrer o arquivo do comeco. Estes
# testes travam as duas coisas que a otimizacao poderia quebrar: o offset tem que ser IDENTICO ao
# da varredura ingenua e tem que cair EXATAMENTE numa fronteira de linha (offset no meio de uma
# linha entrega JSON cortado pro _read_from).

def _naive_tail_offset(path, max_lines: int) -> int:
    """Varredura do comeco (o algoritmo antigo), como referencia."""
    try:
        starts: list[int] = []
        with open(path, "rb") as fh:
            while True:
                start = fh.tell()
                line = fh.readline()
                if not line:
                    break
                if not line.endswith(b"\n"):
                    break                      # ultima linha incompleta: nao conta
                starts.append(start)
        return starts[-max_lines] if len(starts) >= max_lines else 0
    except OSError:
        return 0


def _assert_matches_naive(f, max_lines: int):
    got = TranscriptTailer(f)._tail_offset(max_lines)
    assert got == _naive_tail_offset(f, max_lines)
    blob = f.read_bytes() if f.exists() else b""
    # fronteira de linha: ou o inicio do arquivo, ou logo depois de um \n.
    assert got == 0 or blob[got - 1:got] == b"\n"
    return got


@pytest.fixture(params=[None, 16, 4096], ids=["window-real", "window-16b", "window-4k"])
def janela(request, monkeypatch):
    # Janela minuscula forca varias rodadas de crescimento nos mesmos dados (linha > janela).
    if request.param is not None:
        monkeypatch.setattr("app.transcript._TAIL_WINDOW", request.param)


@pytest.mark.parametrize("max_lines", [1, 2, 3, 5, 200])
def test_tail_offset_igual_a_varredura_ingenua(tmp_path, janela, max_lines):
    f = tmp_path / "s.jsonl"
    f.write_text("".join(_user(f"u{i}", "x" * (i % 7 + 1)) for i in range(20)))
    pos = _assert_matches_naive(f, max_lines)
    evs, _ = TranscriptTailer(f)._read_from(pos)
    assert len(evs) == min(max_lines, 20)      # mesma quantidade de backfill de sempre


def test_tail_offset_arquivo_vazio(tmp_path, janela):
    f = tmp_path / "s.jsonl"
    f.write_bytes(b"")
    assert _assert_matches_naive(f, 3) == 0


def test_tail_offset_sem_newline_final(tmp_path, janela):
    # Ultima linha ainda sendo escrita: nao pode contar nem deslocar o tail.
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("u2", "b") + _user("u3", "c") + '{"type":"user"')
    for max_lines in (1, 2, 3, 9):
        pos = _assert_matches_naive(f, max_lines)
        evs, _ = TranscriptTailer(f)._read_from(pos)
        assert [e.id for e in evs] == ["u1", "u2", "u3"][max(0, 3 - max_lines):]


def test_tail_offset_linha_maior_que_a_janela(tmp_path, janela):
    # Transcript real tem linha gigante (base64 de imagem colada): a janela precisa crescer ate
    # caber, sem loop infinito e sem devolver offset no meio dela.
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("big", "Q" * 400_000) + _user("u3", "c"))
    pos = _assert_matches_naive(f, 2)
    evs, _ = TranscriptTailer(f)._read_from(pos)
    assert [e.id for e in evs] == ["big", "u3"]


def test_guard_reprova_offset_fora_de_fronteira(tmp_path, monkeypatch):
    # O guard acima so vale se reprovar de fato um offset no meio de uma linha.
    f = tmp_path / "s.jsonl"
    f.write_text(_user("u1", "a") + _user("u2", "b"))
    monkeypatch.setattr(TranscriptTailer, "_tail_offset", lambda self, n: 3)
    with pytest.raises(AssertionError):
        _assert_matches_naive(f, 1)


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


# ── parser com memoria (Pi): o par mensagem+marcador de hook nao pode ser partido pelo lote ─────
def _pi_user(node_id: str, text: str) -> str:
    return json.dumps({"type": "message", "id": node_id,
                       "message": {"role": "user", "content": [{"type": "text", "text": text}]}}) + "\n"


def _pi_hook_ctx(parent: str, content: str) -> str:
    return json.dumps({"type": "custom_message", "customType": "claude-hook-context",
                       "content": content, "id": "ctx1", "parentId": parent}) + "\n"


def test_pi_stream_flushes_the_held_message_at_the_end_of_the_batch(tmp_path):
    # Mensagem do usuario e a ULTIMA linha do arquivo (o Pi so escreve a resposta minutos depois):
    # sem o flush do lote a bolha dele nao apareceria no chat ate o turno terminar.
    from app.adapters.pi.transcript import Stream
    f = tmp_path / "s.jsonl"
    f.write_text(_pi_user("n1", "oi"))
    evs, _ = TranscriptTailer(f, parse_line=Stream().parse_line)._read_from(0)
    assert [(e.kind, e.text) for e in evs] == [("user_msg", "oi")]


def test_pi_stream_waits_for_the_hook_sibling_written_right_after(tmp_path, monkeypatch):
    # O caso que a espera curta existe pra cobrir: o watcher acorda ENTRE as duas escritas (elas
    # nascem com 1ms de diferenca, mas nada garante o lote). Sem a releitura, a bolha saia com o
    # "[hook] ..." colado e o marcador chegava orfao no ciclo seguinte — o bug que a feature
    # inteira conserta, voltando calado.
    from app.adapters.pi.transcript import Stream
    ctx = "[skill-suggester] usa a skill X"
    f = tmp_path / "s.jsonl"
    f.write_text(_pi_user("n1", f"{ctx}\n\nmuda o botao"))
    t = TranscriptTailer(f, parse_line=Stream().parse_line)
    # A irmã aparece DURANTE a espera (é o que o sleep do tailer cobre).
    def _escreve_irma(_s):
        with open(f, "a", encoding="utf-8") as fh:
            fh.write(_pi_hook_ctx("n1", ctx))
    monkeypatch.setattr("app.transcript.time.sleep", _escreve_irma)

    evs, pos = t._read_from(0)
    assert [(e.kind, e.text) for e in evs] == [("user_msg", "muda o botao")]
    assert pos == f.stat().st_size      # as DUAS linhas foram consumidas, nada re-lido depois


def test_pi_stream_does_not_wait_when_nothing_is_held(tmp_path, monkeypatch):
    # A espera so pode ser paga quando ha algo retido — senao todo lote de toda sessao Pi
    # (e o backfill inicial, que le o arquivo inteiro) ficaria mais lento a toa.
    from app.adapters.pi.transcript import Stream
    f = tmp_path / "s.jsonl"
    f.write_text(json.dumps({"type": "message", "id": "n1", "message": {
        "role": "assistant", "content": [{"type": "text", "text": "ola"}]}}) + "\n")
    chamou = []
    monkeypatch.setattr("app.transcript.time.sleep", lambda s: chamou.append(s))
    evs, _ = TranscriptTailer(f, parse_line=Stream().parse_line)._read_from(0)
    assert [e.kind for e in evs] == ["assistant_msg"]
    assert chamou == []


def test_claude_tailer_never_pays_the_wait(tmp_path, monkeypatch):
    # Parser sem memoria (Claude/Codex) nao tem flush_events: o caminho novo nem e tocado.
    chamou = []
    monkeypatch.setattr("app.transcript.time.sleep", lambda s: chamou.append(s))
    f = tmp_path / "c.jsonl"
    f.write_text(_user("u1", "oi"))
    evs, _ = TranscriptTailer(f)._read_from(0)
    assert [e.kind for e in evs] == ["user_msg"]
    assert chamou == []
