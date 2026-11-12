# backend/tests/fixtures/contract/gen_queue_follow.py
"""Fila ao vivo: o que o `PromptQueue.follow` do Python emite para cada sequência de gravações.

O hangar-server compara a fila que ele produz com este golden: nome do evento e `data` byte a byte,
na abertura e a cada gravação. O `follow` roda de verdade; só o relógio e o vigia de pasta são
trocados, para cada rodada ter um passo conhecido. Tudo aqui é inventado.

Cenário com nome terminado em `.esperado_rust` é a mesma situação já sem o defeito do Python que o
Jefferson mandou corrigir (decisoes.md D8: linha ruim é pulada, U+2028 não quebra linha): o Rust
compara a saída dele para o cenário base com a do `.esperado_rust`.

Uso, de backend/:  uv run python tests/fixtures/contract/gen_queue_follow.py
"""
import asyncio
import base64
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

# ISO sem fuso é lido em hora local pelo Python; com o processo em UTC o golden não depende da máquina.
os.environ["TZ"] = "UTC"
time.tzset()

HERE = Path(__file__).resolve().parent
BACKEND = HERE.parents[2]
sys.path.insert(0, str(BACKEND))

from app import pqueue  # noqa: E402

GOLDEN = HERE / "golden"
SOURCES = ["backend/app/pqueue.py", "backend/app/models.py"]
COMMAND = "uv run python tests/fixtures/contract/gen_queue_follow.py"
CLOCK = 1_790_000_000.0
C = CLOCK


class _Clock:
    """O `inicio` do follow (corte da nota local) sai deste relógio fixo."""

    def __getattr__(self, name):
        return getattr(time, name)

    def time(self):
        return CLOCK


pqueue.time = _Clock()


def origin_commit() -> str:
    root = BACKEND.parent
    dirty = subprocess.run(["git", "-C", str(root), "status", "--porcelain", "--", *SOURCES],
                           capture_output=True, text=True, check=True).stdout.strip()
    if dirty:
        sys.exit(f"fontes da fila com mudança local; o golden ficaria sem commit de origem:\n{dirty}")
    return subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"],
                          capture_output=True, text=True, check=True).stdout.strip()


def py(row) -> str:
    """Linha como o PromptQueue grava (dumps_safe)."""
    return row if isinstance(row, str) else json.dumps(row, ensure_ascii=False)


def rs(row) -> str:
    """Linha como o Rust grava a projeção (serde compacto)."""
    return row if isinstance(row, str) else json.dumps(row, ensure_ascii=False, separators=(",", ":"))


def file_of(rows, style=py) -> str:
    return "".join(style(r) + "\n" for r in rows)


def write(rows, style=py) -> dict:
    return {"write": file_of(rows, style)}


def raw(text: str) -> dict:
    return {"write": text}


def raw_bytes(data: bytes) -> dict:
    return {"write_b64": base64.b64encode(data).decode("ascii")}


DELETE = {"delete": True}
TICK = {"tick": True}       # rodada do batimento sem gravação
OTHER = {"other": True}     # gravação de outra sessão na mesma pasta


def user(eid: Any, text: Any = "oi", ts: Any = C - 10, delivered: Any = False, **extra) -> dict:
    return {"id": eid, "text": text, "ts": ts, "delivered": delivered, **extra}


def local(eid: Any, text: Any = "saída", ts: Any = C - 5) -> dict:
    return {"id": eid, "text": text, "ts": ts, "delivered": True, "confirmed": True, "papel": "assistant"}


SCENARIOS = [
    {"name": "ciclo_de_uma_entrada", "min_ts": 0.0, "steps": [
        DELETE,
        write([user("a1")]),
        write([user("a1", delivered=True)]),
        write([user("a1", text="oi editado", ts=C - 9, delivered=True)]),
        write([user("a1", delivered=True, confirmed=True)]),
        write([user("a1", delivered=True, confirmed=True, consumed=True)]),
        TICK,
        OTHER,
        DELETE,
        write([user("a1", delivered=True, confirmed=True, consumed=True)]),
    ]},
    {"name": "abertura_com_fila_existente", "min_ts": 0.0, "steps": [
        write([user("p1", "pendente"), user("e1", "entregue", delivered=True),
               user("c1", "confirmada", delivered=True, confirmed=True),
               user("d1", "desistida", desistiu=True)]),
        TICK,
    ]},
    {"name": "projecao_compacta_do_rust", "min_ts": 0.0, "steps": [
        write([user("r1", "do rust"), local("l1", ts=C)], style=rs),
        write([user("r1", "do rust", delivered=True), local("l1", ts=C)], style=rs),
    ]},
    {"name": "corte_por_inicio_da_conversa", "min_ts": C - 100, "steps": [
        write([user("v1", "velha", ts=C - 200), user("n1", "nova", ts=C - 50),
               user("b1", "bastao no limite", ts=C - 1000, pre_transcript=True),
               user("b2", "bastao vencido", ts=C - 1001, pre_transcript=True),
               user("s1", "sem ts", ts=None), {"id": "s2", "text": "ts ausente", "delivered": False},
               user("t1", "ts texto", ts=str(C)), user("z1", "ts zero", ts=0),
               user("x1", "pre_transcript texto", ts=C - 500, pre_transcript="false")]),
        write([user("v1", "velha", ts=C - 200, delivered=True), user("n1", "nova", ts=C - 50, delivered=True)]),
        write([user("v1", "velha", ts=C - 200, delivered=True), user("n1", "nova", ts=C - 50, delivered=True),
               user("n2", "depois", ts=C + 5)]),
    ]},
    {"name": "corte_desligado", "min_ts": 0.0, "steps": [
        write([user("v1", "velha", ts=C - 200), user("b2", "bastao vencido", ts=C - 1001, pre_transcript=True),
               user("s1", "sem ts", ts=None), user("z1", "ts zero", ts=0)]),
    ]},
    {"name": "nota_local", "min_ts": 0.0, "steps": [
        write([local("l1", "antiga", ts=C - 31), local("l2", "recente", ts=C - 29), local("l3", "agora", ts=C),
               local("l4", "sem ts", ts=None), user("u1", "pergunta", ts=C - 40, delivered=True, confirmed=True)]),
        write([local("l1", "antiga editada", ts=C - 31), local("l2", "recente", ts=C - 29), local("l3", "agora", ts=C),
               local("l4", "sem ts", ts=None), user("u1", "pergunta", ts=C - 40, delivered=True, confirmed=True)]),
        write([local("l1", "antiga editada", ts=C - 31), local("l2", "recente", ts=C - 29), local("l3", "agora", ts=C),
               local("l4", "sem ts", ts=None), user("u1", "pergunta", ts=C - 40, delivered=True, confirmed=True),
               local("l5", "nova com ts velho", ts=C - 3600)]),
    ]},
    {"name": "nota_local_com_corte", "min_ts": C - 60, "steps": [
        write([local("l1", "recente", ts=C - 20), local("l2", "antes do corte", ts=C - 90)]),
        write([local("l1", "recente", ts=C - 20), local("l2", "antes do corte", ts=C - 90),
               local("l3", "nova antes do corte", ts=C - 61), local("l4", "nova depois", ts=C - 59)]),
    ]},
    {"name": "reemissao_por_assinatura", "min_ts": 0.0, "steps": [
        write([user("a1")]),
        write([user("a1", delivered=True)]),
        write([user("a1", delivered=False)]),
        write([user("a1", delivered=False, desistiu=True)]),
        write([user("a1", delivered=False)]),
        write([user("a1", delivered=1)]),
        write([user("a1", delivered=True, desistiu=0)]),
        write([user("a1", delivered=True, consumed=False)]),
        write([user("a1", delivered=True, consumed=1)]),
        write([user("a1", delivered=True, consumed=True)]),
        write([user("a1", delivered=True, consumed=True, confirmed=True)]),
        write([user("a1", delivered=True, consumed=True, confirmed=True, desistiu=True)]),
    ]},
    {"name": "ids", "min_ts": 0.0, "steps": [
        write([user("", "id vazio"), {"text": "sem id", "ts": C - 10, "delivered": False},
               user(None, "id nulo"), user(5, "id inteiro"), user(5.0, "id float"), user(True, "id bool"),
               user("dup", "primeira"), user("dup", "segunda", delivered=True), user("dup", "terceira", delivered=True)]),
        write([user(5, "id inteiro", delivered=True)]),
    ]},
    {"name": "tipos_dos_campos", "min_ts": 0.0, "steps": [
        write([user("d1", delivered=1), user("d2", delivered="sim"), user("d3", delivered=None),
               user("c1", consumed=1), user("c2", consumed="true"),
               user("x1", desistiu="x"), user("x2", desistiu=0), user("x3", desistiu=[]),
               user("f1", confirmed="false"), user("f2", confirmed=0), user("f3", confirmed=[0]),
               user("t1", ts=True), user("t2", ts="123"), user("t3", ts=1790000000),
               user("t4", ts=10 ** 20), user("t5", ts=1.5e-7), user("t6", ts=-0.0), user("t7", ts=0.1),
               user("t8", ts=123456789.123456789), user("t9", ts=1e16),
               user("t10", ts=1e-4), user("t11", ts=1e-5), user("t12", ts=1e-6), user("t13", ts=1e15),
               user("t14", ts=9999999999999998.0), user("t15", ts=1.5e16), user("t16", ts=1.2345678901234567e21),
               user("t17", ts=-1e20), user("t18", ts=2 ** 63), user("t19", ts=-(2 ** 63)), user("t20", ts=False),
               {"id": "m1", "ts": C - 10, "delivered": False},
               user("p1", papel="user"), user("p2", papel="Assistant"),
               user("e1", "extra", attempts=3, extra={"k": [1, 2]})]),
    ]},
    {"name": "texto", "min_ts": 0.0, "steps": [
        write([user("u1", "ção ✓ 😀 \"aspas\" \\ barra\ttab\nquebra"),
               user("u2", "<b>&amp;</b> \u0000 controle \u001f"),
               user("u4", ""), user("u5", "   ")]),
        raw('{"id": "s1", "text": "meio \\ud83d emoji", "ts": 1789999990.0, "delivered": false}\n'
            '{"id": "s2", "text": "par \\ud83d\\ude00 inteiro", "ts": 1789999990.0, "delivered": false}\n'
            '{"id": "s3", "text": "escapado \\u00e7\\u2713", "ts": 1789999990.0, "delivered": false}\n'),
    ]},
    {"name": "linhas_ignoradas", "min_ts": 0.0, "steps": [
        raw("\n   \n" + py(user("a1", "depois de brancos")) + "\n"
            "isto não é json\n"
            '{"id": "q1", "text": "cortada"\n'
            "\t" + py(user("a2", "com tab na frente")) + "   \n"
            + py(user("a3", "sem quebra no fim"))),
        raw("﻿" + py(user("bom", "linha com BOM")) + "\n" + py(user("a1", "depois do BOM")) + "\n"),
        raw(py(user("cr", "fim CRLF")) + "\r\n" + py(user("a1", "outra")) + "\r\n"),
        raw_bytes(b'{"id": "inv", "text": "byte \xff\xfe inv\xc3lido", "ts": 1789999990.0, "delivered": false}\n'),
    ]},
    {"name": "arquivo_esvaziado_e_recriado", "min_ts": 0.0, "steps": [
        write([user("a1"), user("a2", "outra")]),
        raw(""),
        write([user("a3", "depois do clear")]),
        DELETE,
        TICK,
        write([user("a1"), user("a4", "volta")]),
    ]},
    {"name": "ids_escalares", "min_ts": 0.0, "steps": [
        write([user(0, "id zero"), user(False, "id falso"), user(1e20, "id float grande"), user(0.5, "id meio"),
               user(-3, "id negativo"), user("ção", "id unicode"), user(" ", "id espaço")])]},
    {"name": "corte_com_ts_texto", "min_ts": C - 100, "steps": [
        write([user("s1", ts=str(int(C))), user("s2", ts=f"  {C}  "), user("s3", ts="1.79e9"),
               user("s4", ts="1_790_000_000"), user("s5", ts="nan"), user("s6", ts="inf"), user("s7", ts="-inf"),
               user("s8", ts="Infinity"), user("s9", ts=True), user("s10", ts={}), user("s11", ts=""),
               user("s12", ts=[]), user("s13", ts="+1790000000"), user("s14", ts=" 1790000000 ")])]},
    {"name": "ordem_de_muitas", "min_ts": 0.0, "steps": [
        write([user(f"m{i:02d}", f"mensagem {i}", ts=C - 100 + i, delivered=i % 3 == 0, confirmed=i % 5 == 0)
               for i in range(40)]),
        write([user(f"m{i:02d}", f"mensagem {i}", ts=C - 100 + i, delivered=True, confirmed=i % 5 == 0)
               for i in range(40)]),
    ]},
    # B9: o Python cai; o Rust pula a linha e segue (decisoes.md D8).
    {"name": "linha_numero", "min_ts": 0.0, "steps": [
        raw(py(user("a1")) + "\n5\n" + py(user("a2", "depois")) + "\n")]},
    {"name": "linha_numero.esperado_rust", "min_ts": 0.0, "steps": [
        write([user("a1"), user("a2", "depois")])]},
    {"name": "linha_lista", "min_ts": 0.0, "steps": [
        write([user("a1")]),
        raw(py(user("a1")) + "\n[1, 2]\n" + py(user("a2", "depois")) + "\n")]},
    {"name": "linha_lista.esperado_rust", "min_ts": 0.0, "steps": [
        write([user("a1")]),
        write([user("a1"), user("a2", "depois")])]},
    {"name": "linha_null", "min_ts": 0.0, "steps": [
        raw("null\n" + py(user("a1")) + "\n")]},
    {"name": "linha_null.esperado_rust", "min_ts": 0.0, "steps": [
        write([user("a1")])]},
    {"name": "linha_texto_json", "min_ts": 0.0, "steps": [
        raw('"so texto"\n' + py(user("a1")) + "\n")]},
    {"name": "linha_texto_json.esperado_rust", "min_ts": 0.0, "steps": [
        write([user("a1")])]},
    {"name": "texto_numero", "min_ts": 0.0, "steps": [
        write([user("a1"), user("a2", 123), user("a3", "depois")])]},
    {"name": "texto_numero.esperado_rust", "min_ts": 0.0, "steps": [
        write([user("a1"), user("a3", "depois")])]},
    {"name": "texto_lista_em_nota", "min_ts": 0.0, "steps": [
        write([local("l1", ["a"], ts=C)])]},
    {"name": "texto_lista_em_nota.esperado_rust", "min_ts": 0.0, "steps": [
        raw("")]},
    {"name": "ts_texto_com_corte", "min_ts": C - 100, "steps": [
        write([user("a1", ts=C - 10), user("a2", ts="abc"), user("a3", "depois", ts=C - 5)])]},
    {"name": "ts_texto_com_corte.esperado_rust", "min_ts": C - 100, "steps": [
        write([user("a1", ts=C - 10), user("a3", "depois", ts=C - 5)])]},
    {"name": "ts_lista_com_corte", "min_ts": C - 100, "steps": [
        write([user("a1", ts=C - 10), user("a2", ts=[1]), user("a3", "depois", ts=C - 5)])]},
    {"name": "ts_lista_com_corte.esperado_rust", "min_ts": C - 100, "steps": [
        write([user("a1", ts=C - 10), user("a3", "depois", ts=C - 5)])]},
    {"name": "ts_texto_em_nota_na_abertura", "min_ts": 0.0, "steps": [
        write([local("l1", ts="ontem"), user("a1")])]},
    {"name": "ts_texto_em_nota_na_abertura.esperado_rust", "min_ts": 0.0, "steps": [
        write([user("a1")])]},
    # B8: U+2028/U+2029/U+0085 crus partem a linha no splitlines do Python e a entrada some.
    {"name": "separador_de_linha_cru", "min_ts": 0.0, "steps": [
        raw(py(user("a1", "antes depois")) + "\n" + py(user("a2", "par agrafo")) + "\n"
            + py(user("a3", "nel\u0085aqui")) + "\n" + py(user("a4", "normal")) + "\n")]},
    {"name": "separador_de_linha_cru.esperado_rust", "min_ts": 0.0, "steps": [
        raw(json.dumps(user("a1", "antes depois")) + "\n" + json.dumps(user("a2", "par agrafo")) + "\n"
            + json.dumps(user("a3", "nel\u0085aqui")) + "\n" + py(user("a4", "normal")) + "\n")]},
    # NaN/Infinity: o json do Python lê, o serde recusa a linha; o Rust a pula (decisoes.md D9).
    {"name": "ts_nan_e_infinito", "min_ts": 0.0, "steps": [
        raw('{"id": "n1", "text": "nan", "ts": NaN, "delivered": false}\n'
            '{"id": "n2", "text": "inf", "ts": Infinity, "delivered": false}\n'
            + py(user("a1", "normal")) + "\n")]},
    {"name": "ts_nan_e_infinito.esperado_rust", "min_ts": 0.0, "steps": [
        write([user("a1", "normal")])]},
    # id objeto ou lista: o Python usa o repr dele no id da bolha; o Rust pula a linha (D9).
    {"name": "id_estruturado", "min_ts": 0.0, "steps": [
        write([user({"a": 1}, "id objeto"), user([1, "x"], "id lista"), user("a1", "normal")])]},
    {"name": "id_estruturado.esperado_rust", "min_ts": 0.0, "steps": [
        write([user("a1", "normal")])]},
]

START_TS_CASES = [
    {"name": "iso_z_na_primeira", "content": '{"type":"user","timestamp":"2026-09-21T14:13:20.000Z"}\n'},
    {"name": "pula_linha_sem_ts", "content": '{"type":"permission-mode"}\n{"timestamp":"2026-09-21T14:13:20Z"}\n'},
    {"name": "iso_com_fuso", "content": '{"timestamp":"2026-09-21T11:13:20-03:00"}\n'},
    {"name": "iso_sem_fuso_vale_utc_aqui", "content": '{"timestamp":"2026-09-21T14:13:20"}\n'},
    {"name": "iso_invalido_segue_para_a_proxima", "content": '{"timestamp":"ontem"}\n{"timestamp":"2026-09-21T14:13:20Z"}\n'},
    {"name": "iso_invalido_nao_olha_message", "content":
        '{"timestamp":"ontem","message":{"timestamp":1790000000000}}\n{"time":1790000001000}\n'},
    {"name": "pi_message_ms", "content": '{"type":"message","message":{"timestamp":1790000000500}}\n'},
    {"name": "pi_message_texto_ignorado", "content": '{"message":{"timestamp":"1790000000500"}}\n{"time":1790000002000}\n'},
    {"name": "kimi_time_ms", "content": '{"time":1790000000250}\n'},
    {"name": "kimi_created_at_ms", "content": '{"created_at":1790000000750}\n'},
    {"name": "time_vence_created_at", "content": '{"created_at":1790000000750,"time":1790000003000}\n'},
    {"name": "ts_zero_segue", "content": '{"time":0}\n{"time":1790000004000}\n'},
    {"name": "ts_negativo_segue", "content": '{"time":-5}\n{"time":1790000004000}\n'},
    {"name": "linhas_nao_json", "content": "lixo\n\n[1]\n{\"time\":1790000005000}\n"},
    {"name": "sem_ts_nenhum", "content": '{"type":"x"}\n{"type":"y"}\n'},
    {"name": "vazio", "content": ""},
    {"name": "ausente", "content": None},
    {"name": "bool_nao_conta", "content": '{"time":true}\n{"time":1790000006000}\n'},
    {"name": "utf8_invalido_na_linha", "content_b64":
        base64.b64encode(b'{"x":"\xff"}\n{"time":1790000007000}\n').decode("ascii")},
]


def _apply(step: dict, path: Path, qdir: Path):
    if "write" in step or "write_b64" in step:
        data = base64.b64decode(step["write_b64"]) if "write_b64" in step else step["write"].encode("utf-8")
        tmp = path.with_suffix(".jsonl.tmp")
        tmp.write_bytes(data)
        os.replace(tmp, path)
        return {(2, str(path))}
    if "delete" in step:
        path.unlink(missing_ok=True)
        return {(3, str(path))}
    if "other" in step:
        other = qdir / "outra-sessao.jsonl"
        other.write_text(py(user("o1")) + "\n", encoding="utf-8")
        return {(2, str(other))}
    return set()


async def _follow(min_ts: float, steps: list[dict]) -> list[dict]:
    with tempfile.TemporaryDirectory() as d:
        qdir = Path(d)
        pqueue._queue_dir = lambda: qdir
        queue = pqueue.PromptQueue("fila")
        collected: list[dict] = []
        boundaries: list[int] = []
        _apply(steps[0], queue.path, qdir)

        async def changes(*args, **kwargs):
            for step in steps[1:]:
                boundaries.append(len(collected))
                yield _apply(step, queue.path, qdir)
            boundaries.append(len(collected))

        pqueue.awatch = changes
        error = None
        try:
            async for event in queue.follow(min_ts=min_ts, emit_confirmed=True):
                # O mesmo nome e corpo que o `pump` do sse.py põe no SSE.
                name = "queue_confirmed" if event.queued_confirmed else "message"
                collected.append({"event": name, "data": event.model_dump_json()})
        except Exception as exc:
            error = type(exc).__name__
        ends = boundaries + [len(collected)]
        out = []
        start = 0
        for index, step in enumerate(steps):
            if index >= len(ends):
                break
            entry = {**step, "events": collected[start:ends[index]]}
            start = ends[index]
            if error is not None and index == len(ends) - 1:
                entry["error"] = error
            out.append(entry)
        return out


def _start_ts(case: dict) -> dict:
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "conversa.jsonl"
        if "content_b64" in case:
            path.write_bytes(base64.b64decode(case["content_b64"]))
        elif case["content"] is not None:
            path.write_text(case["content"], encoding="utf-8")
        try:
            return {"result": pqueue._transcript_start_ts(str(path))}
        except Exception as exc:
            return {"result": None, "error": type(exc).__name__}


def main():
    meta = {"commit": origin_commit(), "command": COMMAND, "sources": SOURCES}
    scenarios = [{"name": s["name"], "min_ts": s["min_ts"], "clock": CLOCK,
                  "steps": asyncio.run(_follow(s["min_ts"], s["steps"]))} for s in SCENARIOS]
    start_ts = [{**case, **_start_ts(case)} for case in START_TS_CASES]
    GOLDEN.mkdir(exist_ok=True)
    (GOLDEN / "queue_follow.json").write_text(
        json.dumps({"_meta": meta, "scenarios": scenarios}, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    (GOLDEN / "queue_start_ts.json").write_text(
        json.dumps({"_meta": meta, "cases": start_ts}, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{len(scenarios)} cenários da fila e {len(start_ts)} inícios de conversa gravados em {GOLDEN}")


if __name__ == "__main__":
    main()
