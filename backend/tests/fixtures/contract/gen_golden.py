# backend/tests/fixtures/contract/gen_golden.py
"""Transcripts sintéticos do contrato da conversa e o resultado que os parsers Python dão para eles.

Os testes do hangar-server comparam campo a campo com o que este script grava em golden/. Tudo aqui
é inventado: nunca copie conversa real para cá.

Uso, de backend/:  uv run python tests/fixtures/contract/gen_golden.py
"""
import json
import logging
import os
import shutil
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path

# Relógio sem fuso: o RewriteFilter e o merged_history do Python leem como hora LOCAL, o Rust como
# UTC. Com o processo em UTC os dois concordam no golden.
os.environ["TZ"] = "UTC"
time.tzset()

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))

from app import pqueue, registry, sse, state  # noqa: E402
from app.adapters.codex.rollout import parse_rollout_line  # noqa: E402
from app.models import StateEvent, scrub_surrogates  # noqa: E402
from app.sse import _ask_question_event  # noqa: E402
from app.transcript import RewriteFilter, TranscriptTailer, _ts, parse_obj  # noqa: E402

TRANSCRIPTS = HERE / "transcripts"
QUEUE = HERE / "queue"
GOLDEN = HERE / "golden"

# O recado nativo resolveria o pid pelo tmux desta máquina; o golden não pode depender disso.
registry.name_of_pid = lambda pid: None
pqueue._queue_dir = lambda: QUEUE


def j(obj, ascii=False) -> bytes:
    # Separadores compactos, como o Claude e o Codex gravam; ascii=True escreve o surrogate solto
    # como "\ud83d", que é como ele aparece no arquivo.
    return json.dumps(obj, ensure_ascii=ascii, separators=(",", ":")).encode("utf-8")


def T(sec: int, ms: int = 0) -> str:
    return f"2026-09-01T10:{sec // 60:02d}:{sec % 60:02d}.{ms:03d}Z"


def C(sec: int) -> str:
    return f"2026-09-02T14:{sec // 60:02d}:{sec % 60:02d}.000Z"


def epoch(iso: str) -> float:
    return datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp()


BLOCKED = ('UserPromptSubmit operation blocked by hook:\n["/h/bloqueia.sh"]: falhou\n\n'
           "Original prompt: [de: outra] recado preso")

CLAUDE = [
    j({"type": "permission-mode", "permissionMode": "bypassPermissions", "sessionId": "s-claude"}),
    j({"parentUuid": None, "isSidechain": False, "type": "user", "uuid": "u-001", "timestamp": T(0),
       "message": {"role": "user", "content": "Olá, mundo — teste ✓"}}),
    j({"type": "assistant", "uuid": "a-001", "timestamp": T(1, 250), "message": {
        "role": "assistant",
        "content": [
            {"type": "thinking", "thinking": "Pensando no pedido…", "signature": "x"},
            {"type": "text", "text": "Vou listar os arquivos."},
            {"type": "tool_use", "id": "toolu_01", "name": "Bash",
             "input": {"command": "ls -la", "timeout": 120000, "ratio": 0.1, "tiny": 1e-05,
                       "huge": 1e16, "exact": 9007199254740993, "neg": -0.0, "um": 1.0}},
            {"type": "thinking", "thinking": "", "signature": "y"},
        ],
        "usage": {"input_tokens": 10, "cache_read_input_tokens": 1200,
                  "cache_creation": {"ephemeral_1h_input_tokens": 50, "ephemeral_5m_input_tokens": 0}}}}),
    j({"type": "user", "uuid": "u-002", "timestamp": T(2), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_01",
         "content": [{"type": "text", "text": "a.txt"}, {"type": "text", "text": "b.txt"}], "is_error": False},
        {"type": "tool_result", "tool_use_id": "toolu_02", "content": "falhou", "is_error": True},
        {"type": "tool_result", "tool_use_id": "toolu_03", "content": {"exit": 0, "msg": "it's"}}]}}),
    j({"type": "user", "uuid": "u-003", "timestamp": T(3), "message": {"role": "user", "content": [
        {"type": "text", "text": "[Image #1] olha isto"},
        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}}]}}),
    j({"type": "user", "uuid": "u-004", "timestamp": T(3, 10),
       "message": {"role": "user", "content": "[Image: source: /tmp/x.png]"}}),
    j({"type": "user", "uuid": "u-005", "timestamp": T(4), "isMeta": True,
       "message": {"role": "user", "content": "Continue from where you left off."}}),
    j({"type": "user", "uuid": "u-006", "timestamp": T(4, 500),
       "message": {"role": "user", "content": "<command-name>/model</command-name>"}}),
    j({"type": "user", "uuid": "u-007", "timestamp": T(5),
       "message": {"role": "user", "content": "[Request interrupted by user]"}}),
    j({"type": "user", "uuid": "u-008", "timestamp": T(6), "message": {"role": "user", "content":
        '<system-reminder>lembrete</system-reminder>\n<pasted_content id="p1">\ntexto colado\n</pasted_content id="p1">'}}),
    # Surrogate solto numa linha sem relógio: o RewriteFilter deixa passar e o parse_obj recebe o
    # texto cru, que só vira U+FFFD na borda do ChatEvent.
    j({"type": "user", "uuid": "u-009", "message": {"role": "user", "content": "meio emoji \ud83d aqui"}},
      ascii=True),
    j({"type": "queue-operation", "operation": "enqueue", "timestamp": T(7),
       "content": "msg no meio \ud83d do turno"}, ascii=True),
    # O id leva o md5 do texto cru: o surrogate entra como "?" (encode com "replace").
    j({"type": "queue-operation", "operation": "remove", "timestamp": T(8),
       "content": "msg no meio \ud83d do turno"}, ascii=True),
    j({"type": "queue-operation", "operation": "enqueue", "timestamp": T(9),
       "content": "<task-notification><task-id> tsk-1 </task-id><status>completed</status></task-notification>"}),
    j({"type": "system", "subtype": "informational", "timestamp": T(10), "content": BLOCKED}),
    j({"type": "system", "subtype": "informational", "timestamp": T(11), "content": BLOCKED}),
    j({"type": "system", "timestamp": T(11, 500), "content": "Held peer message [de: x] sem âncora"}),
    j({"type": "attachment", "uuid": "att-1", "timestamp": T(12), "attachment": {
        "type": "queued_command", "prompt": [{"type": "text", "text": "orientação no meio"}]}}),
    # Anexo que nunca vira bolha: o /history lê só o relógio dele, sem json.loads.
    j({"parentUuid": "a-001", "isSidechain": False, "attachment": {"type": "hook_success", "content": "ok"},
       "type": "attachment", "uuid": "att-2", "timestamp": T(13)}),
    j({"type": "attachment", "uuid": "att-3", "timestamp": T(14), "attachment": {
        "type": "hook_additional_context", "hookEvent": "Stop", "content": ["Rode os testes.", 7]}}),
    j({"type": "user", "uuid": "u-010", "timestamp": T(15), "isCompactSummary": True,
       "message": {"role": "user", "content": "resumo longo"}}),
    j({"type": "user", "uuid": "u-011", "timestamp": T(16), "message": {"role": "user", "content":
        'Another Claude session sent a message:\n<teammate-message teammate_id="ana">\noi líder\n'
        '</teammate-message>\n<teammate-message teammate_id="bia">\n{"type": "idle"}\n</teammate-message>'}}),
    j({"type": "user", "uuid": "u-012", "timestamp": T(17), "message": {"role": "user", "content": [
        {"type": "text", "text": '<agent-message from="ag-1">[Subagent hand-back] pronto</agent-message>'}]}}),
    j({"type": "user", "uuid": "u-013", "timestamp": T(18), "isMeta": True,
       "origin": {"kind": "peer", "body": "olá par", "name": "Título da outra", "verifiedPeerPid": 999999},
       "message": {"role": "user", "content": '<cross-session-message from="x">…</cross-session-message>'}}),
    j({"type": "user", "uuid": "u-014", "timestamp": T(19), "message": {"role": "user", "content": "ok"}}),
    j({"type": "assistant", "uuid": "a-002", "timestamp": T(20), "message": {
        "role": "assistant", "content": [{"type": "text", "text": "Feito."}],
        "usage": {"cache_read_input_tokens": 3.9,
                  "cache_creation": {"ephemeral_1h_input_tokens": 0, "ephemeral_5m_input_tokens": 12}}}}),
    # `claude --resume` regravou: mesmo relógio e mesmo conteúdo, uuid novo.
    j({"parentUuid": None, "isSidechain": False, "type": "user", "uuid": "u-001-bis", "timestamp": T(0),
       "message": {"role": "user", "content": "Olá, mundo — teste ✓"}}),
    # Relógio uma hora atrás do maior já visto: reescrita.
    j({"type": "assistant", "uuid": "a-velho", "timestamp": "2026-09-01T09:00:00.000Z",
       "message": {"role": "assistant", "content": [{"type": "text", "text": "velho"}]}}),
    b'{"type":"user","message":',
    b"",
    b"   \t",
    j({"type": "user", "uuid": "u-015", "timestamp": T(21),
       "message": {"role": "user", "content": "byte @@FF@@ inválido"}}).replace(b"@@FF@@", b"\xff"),
    j({"type": "user", "uuid": "u-016", "timestamp": "2026-09-01T10:00:22",
       "message": {"role": "user", "content": "relógio sem fuso"}}),
    j({"type": "user", "uuid": "u-017", "timestamp": T(23), "message": {"role": "user", "content": "legenda da foto"}}),
    j({"type": "assistant", "uuid": "a-003", "timestamp": T(24),
       "message": {"role": "assistant", "content": [{"type": "text", "text": "Última resposta."}]}}),
    # Resultados de Edit/Write com o `structuredPatch` gravado ao lado (`toolUseResult`). Ficam no fim
    # para não deslocar o offset das linhas acima.
    j({"type": "user", "uuid": "u-018", "timestamp": T(25), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_10", "content": "The file /src/app.ts has been updated."}]},
       "toolUseResult": {"filePath": "/src/app.ts", "oldString": "b", "newString": "B",
                         "originalFile": "arquivo inteiro que não sai\n", "userModified": False,
                         "structuredPatch": [
                             {"oldStart": 6, "oldLines": 3, "newStart": 6, "newLines": 3,
                              "lines": [" linha a", "-linha b", "+linha B", " linha c"]},
                             {"oldStart": 40, "oldLines": 2, "newStart": 40, "newLines": 2,
                              "lines": ["-fim sem quebra", "\\ No newline at end of file",
                                        "+fim com acentuação ✓", "\\ No newline at end of file"]}]}}),
    j({"type": "user", "uuid": "u-019", "timestamp": T(26), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_11", "content": "The file /src/app.ts has been updated."}]},
       "toolUseResult": {"structuredPatch": [
           {"oldStart": 1, "newStart": 1, "lines": [" ok"]},
           {"oldStart": "3", "newStart": 3, "lines": ["+quebrado"]}]}}),
    j({"type": "user", "uuid": "u-020", "timestamp": T(27), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_12", "content": "Edit recusado.", "is_error": True}]},
       "toolUseResult": {"structuredPatch": [
           {"oldStart": 2, "oldLines": 1, "newStart": 2, "newLines": 1, "lines": ["-x", "+y"]}]}}),
    j({"type": "user", "uuid": "u-021", "timestamp": T(28), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_13", "content": "File created successfully."}]},
       "toolUseResult": {"type": "create", "filePath": "/src/novo.ts", "content": "novo\n",
                         "structuredPatch": []}}),
    # Dois resultados na mesma linha: o `toolUseResult` é um só e não se sabe de quem é.
    j({"type": "user", "uuid": "u-022", "timestamp": T(29), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_14", "content": "ok"},
        {"type": "tool_result", "tool_use_id": "toolu_15", "content": "ok"}]},
       "toolUseResult": {"structuredPatch": [{"oldStart": 1, "newStart": 1, "lines": ["+z"]}]}}),
    # Posição acima do teto do u32 do tipo compartilhado do Rust: o Python recusa igual.
    j({"type": "user", "uuid": "u-023", "timestamp": T(30), "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_16", "content": "ok"}]},
       "toolUseResult": {"structuredPatch": [{"oldStart": 4294967296, "newStart": 1, "lines": ["+z"]}]}}),
]

# Surrogate solto numa mensagem COM relógio: o RewriteFilter do Python estoura no md5
# (UnicodeEncodeError) e derruba a leitura. O Rust segue; o golden marca a linha.
CLAUDE_REWRITE = [
    j({"type": "user", "uuid": "r-001", "timestamp": T(0), "message": {"role": "user", "content": "antes"}}),
    j({"type": "assistant", "uuid": "r-002", "timestamp": T(1), "message": {
        "role": "assistant", "content": [{"type": "text", "text": "meio \ud83d emoji"}]}}, ascii=True),
    j({"type": "user", "uuid": "r-003", "timestamp": T(2), "message": {"role": "user", "content": "depois"}}),
]


def item(payload, sec, ascii=False):
    return j({"timestamp": C(sec), "type": "response_item", "payload": payload}, ascii=ascii)


def user_text(text, sec):
    return item({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}, sec)


def tool_call(name, code, call, sec):
    return item({"type": "custom_tool_call", "name": name, "call_id": call, "input": code}, sec)


def tool_output(call, output, sec, kind="custom_tool_call_output"):
    return item({"type": kind, "call_id": call, "output": output}, sec)


SCRIPT_HEADER = "Script completed\nWall time 0.1 seconds\nOutput:\n"

CODEX = [
    j({"timestamp": C(0), "type": "session_meta", "payload": {
        "id": "019f0000-0000-7000-8000-000000000001", "cwd": "/tmp/proj", "cli_version": "0.151.0"}}),
    item({"type": "message", "role": "developer",
          "content": [{"type": "input_text", "text": "<permissions instructions>x</permissions instructions>"}]}, 1),
    user_text("<environment_context>\n<cwd>/tmp/proj</cwd>\n</environment_context>", 1),
    user_text("# AGENTS.md instructions for /tmp/proj\n\n<INSTRUCTIONS>\nseja breve\n</INSTRUCTIONS>", 1),
    # Surrogate solto e números que o `_event_id` serializa: o sha1 tem que sair igual.
    item({"type": "message", "role": "user",
          "content": [{"type": "input_text", "text": "Oi Codex — meio emoji \ud83d aqui"}],
          "meta": {"exato": 9007199254740993, "f": 0.1, "e": 1e16, "neg": -2, "tiny": 1e-05, "um": 1.0,
                   "s": "ç\u007f", "lista": [True, None]}}, 2, ascii=True),
    user_text("Rode os testes", 3),
    item({"type": "message", "role": "assistant", "content": [
        {"type": "output_text", "text": "Rodando "}, {"type": "output_text", "text": "agora."},
        {"type": "reasoning_text", "text": "ignorar"}]}, 4),
    item({"type": "function_call", "name": "shell", "call_id": "call_1",
          "arguments": '{"cmd": ["ls"], "um": 1.0, "tiny": 1e-05}'}, 5),
    item({"type": "function_call", "name": "shell", "call_id": "call_1b", "arguments": "não é json"}, 5),
    tool_output("call_1", "a.txt\nb.txt", 6, kind="function_call_output"),
    tool_call("exec", 'const r = await tools.exec_command({cmd:"echo \\"oi\\"","workdir":"/tmp"}); text(r.output);',
              "call_2", 7),
    tool_output("call_2", [{"type": "input_text", "text": SCRIPT_HEADER},
                           {"type": "input_text", "text": json.dumps({"output": "oi\n", "chunk_id": "c1",
                                                                      "wall_time_seconds": 0.1, "exit_code": 0,
                                                                      "session_id": 7})}], 8),
    tool_call("exec", 'await tools.apply_patch("*** Begin Patch\\n*** Update File: src/a.py\\n@@\\n-x = 1\\n+x = 2\\n'
                      '*** Add File: src/b.py\\n+novo\\n*** End Patch\\n");', "call_3", 9),
    tool_output("call_3", "Script failed\nWall time 2 seconds\nOutput:\n"
                + json.dumps({"output": "erro", "chunk_id": "c2", "wall_time_seconds": 2, "exit_code": 2}), 10),
    tool_call("exec", 'await tools.update_plan({explanation:"x",plan:[{step:"lidar com {config}",status:"completed"},'
                      '{step:"testar \\"tudo\\"",status:"in_progress"},{step:"sem status"}]});', "call_4", 11),
    tool_call("exec", 'await tools.exec_command({cmd:"git status"}); await tools.exec_command({cmd:"git diff"});',
              "call_5", 12),
    tool_call("exec", 'await tools.write_stdin({session_id:1,chars:"y"}); await tools.view_image({path:"/a.png"}); '
                      'await tools.write_stdin({session_id:1,chars:"n"});', "call_6", 13),
    tool_call("exec", 'await tools.write_stdin({session_id:1,chars:"q"});', "call_7", 14),
    tool_call("apply_patch", "*** Begin Patch\n*** Delete File: velho.txt\n*** End Patch", "call_8", 15),
    tool_output("call_8", [{"type": "input_text", "text": SCRIPT_HEADER + json.dumps([
        {"status": "fulfilled", "value": {"output": "a", "chunk_id": "c", "wall_time_seconds": 0}},
        {"status": "rejected", "reason": {"msg": "não", "code": 1}}])}], 16),
    tool_output("call_9", {"exit": 0, "msg": "it's"}, 17, kind="function_call_output"),
    tool_output("call_10", SCRIPT_HEADER + "texto solto\n", 18),
    item({"type": "function_call_output", "call_id": "call_11"}, 18),
    user_text("<turn_aborted>\nThe user interrupted.\n</turn_aborted>", 19),
    user_text('<hook_prompt hook_run_id="h1">\n  Rode o lint antes.\n</hook_prompt>', 20),
    user_text("<skill>\n<name> handoff </name>\n<path>/home/x/.codex/skills/handoff/SKILL.md</path>\n# Handoff\n"
              "corpo\n</skill>", 21),
    user_text("<skill><name>sem-caminho</name>corpo curto</skill>", 22),
    item({"type": "reasoning", "summary": [], "encrypted_content": "gAAAA"}, 23),
    j({"timestamp": C(23), "type": "event_msg", "payload": {"type": "token_count"}}),
    b'{"timestamp":"x","type":"response_item","payload":',
    b"",
    user_text("Rode os testes", 24),
]

CLAUDE_QUEUE = [
    {"id": "q-absorvida", "text": "ok", "ts": epoch(T(18, 500)), "delivered": True},
    {"id": "q-segundo-ok", "text": "ok", "ts": epoch(T(19, 500)), "delivered": True},
    {"id": "q-legenda", "text": "legenda da foto — 📎 imagem: /tmp/foto.png", "ts": epoch(T(22, 900)),
     "delivered": True},
    {"id": "q-confirmada", "text": "já confirmada", "ts": epoch(T(5)), "delivered": True, "confirmed": True},
    {"id": "q-local", "text": "Saída do /cost", "ts": epoch(T(6, 500)), "delivered": True, "confirmed": True,
     "papel": "assistant"},
    {"id": "q-velha", "text": "de outra vida", "ts": epoch(T(0)) - 3600, "delivered": True},
    {"id": "q-bastao", "text": "kick-off do bastão", "ts": epoch(T(0)) - 600, "delivered": False,
     "pre_transcript": True},
    {"id": "q-desistiu", "text": "não chegou", "ts": epoch(T(9)), "delivered": True, "desistiu": True,
     "desistiu_ts": epoch(T(20))},
    {"id": "q-sem-ts", "text": "sem relógio", "delivered": False},
    {"id": "q-vazia", "text": "   ", "ts": epoch(T(10))},
    "não é json {",
    # O splitlines do Python quebra no U+2028 cru: a entrada some, e no Rust tem que sumir também.
    {"id": "q-quebrada", "text": "antes depois", "ts": epoch(T(12)), "delivered": False},
    {"id": 42, "text": "id numérico", "ts": epoch(T(12)), "delivered": "sim"},
    {"id": "q-pendente", "text": "ainda na fila", "ts": epoch(T(24, 100)), "delivered": False},
]

CODEX_QUEUE = [
    {"id": "c-absorvida", "text": "Rode os testes", "ts": epoch(C(3)) - 0.5, "delivered": True},
    {"id": "c-pendente", "text": "e depois o lint", "ts": epoch(C(30)), "delivered": True},
    {"id": "c-confirmada", "text": "x", "ts": epoch(C(4)), "delivered": True, "confirmed": True},
]

PYJSON = [
    '{"b": 1, "a": [1, 1.0, 1e-05, 1e16, 1e15, 0.1, -0.0, 1.5e-07, 123.456, 9007199254740993, '
    '-9223372036854775808, 18446744073709551615, 5e-324, 1e300, 2.5e-05, 100.0, 0.0001]}',
    '{"txt": "é ç ã 😀 \\u007f \\u001f \\" \\\\ \\n \\t \\b \\f /", "sur": "meio \\ud83d emoji", '
    '"par": "\\ud83d\\ude00", "baixo": "\\ude00 só", "alto_e_A": "\\ud83d\\u0041"}',
    '{"z": {"y": [], "x": {}}, "a": null, "m": true, "n": false, "": "vazio"}',
    '{"a": 1, "\\ud83d": 2, "\\ue000": 3, "😀": 4, "\\u00e9": 5}',
    '["\\u2028\\u2029", {"k": "\\ud800"}]',
    '"só texto"',
    "12345678901234567890",
]

ISO = [
    "2026-10-02T10:00:00Z", "2026-10-02T10:00:00.1Z", "2026-10-02T10:00:00.1234567Z",
    "2026-10-02T10:00:00.123456789+00:00", "2026-10-02", "2026-10-02 10:00", "20261002T100000Z",
    "2026-10-02T10:00:00+0300", "2026-10-02T10:00:00-03:00", "2026-10-02T10Z", "2026-10-02T10:00:00,5Z",
    "2026-10-02x10:00:00Z", "1969-12-31T23:59:59.5Z", "2026-02-30T10:00:00Z", "2026-13-01T00:00:00Z", "",
    "não é data", "2026-10-02T25:00:00Z", "2026-10-02T10:00:00.Z", "2026-10-02T1000Z",
    "2026-10-02T24:00:00Z", "2026-12-31T24:00:00Z", "2026-10-02T24:00:01Z", "2026-10-02T10:00:00+03",
    "2026-10-02T10:00:00+05:30:15", "2026-10-02T10:00:00+05:30:15.5", "2026-10-02T10:00:00.5",
    "2026-10-02T10:00:60Z", "2026-10-02T10:00:00 ", " 2026-10-02T10:00:00", "2026-10-02T10:00:00+24:00",
    "2026-1-02", "2026-10-02T10:0", "0001-01-01T00:00:00Z", "2026-10-02T10:00:00z", "2026-10-02T",
    "2026-10-0210:00", "20261002", "2026-10-02T10:00:00.12Z", "2026-10-02T100000.5Z",
    "2026-10-02T10:00:00-0000", "2024-02-29T12:00:00Z", "2026-09-01T10:00:01.250Z",
]

# Janela pequena exercita a leitura de trás para frente sem arquivo de 256 KB no repositório; com 9
# a janela para no meio do arquivo e o `--resume` regravado escapa do filtro, como no Python.
VARIANTS = [("full", None, 256 * 1024), ("limit2", 2, 256 * 1024), ("limit200", 200, 256 * 1024),
            ("limit2_w512", 2, 512), ("limit9_w512", 9, 512), ("limit200_w512", 200, 512)]

# Linhas que não são JSON, de propósito. Erro de digitação aqui viraria linha pulada calada.
INVALID_LINES = {"claude.jsonl": 1, "claude_rewrite_surrogate.jsonl": 0, "codex.jsonl": 1}


def write_jsonl(path: Path, lines: list[bytes]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(line + b"\n" for line in lines))
    bad = 0
    for line in lines:
        text = line.decode("utf-8", "replace").strip()
        if not text:
            continue
        try:
            json.loads(text)
        except ValueError:
            bad += 1
    assert bad == INVALID_LINES[path.name], f"{path.name}: {bad} linhas inválidas"


def write_queue(path: Path, entries: list) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    # Mesmo formato do PromptQueue._write_atomic (json.dumps com ensure_ascii=False).
    text = "".join((e if isinstance(e, str) else json.dumps(e, ensure_ascii=False)) + "\n" for e in entries)
    path.write_text(text, encoding="utf-8")


def write_golden(name: str, data) -> None:
    GOLDEN.mkdir(parents=True, exist_ok=True)
    (GOLDEN / name).write_text(json.dumps(data, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")


def event_dict(ev) -> dict:
    return ev.model_dump(mode="json")


def tail(path: Path, codex: bool) -> list[dict]:
    # O que o TranscriptTailer emite lendo do início (o backfill com offset 0).
    tailer = TranscriptTailer(path, parse_line=parse_rollout_line) if codex else TranscriptTailer(path)
    evs, _ = tailer._read_from(0)
    return [{"offset": ev.offset, "event": event_dict(ev)} for ev in evs]


def tail_tolerant(path: Path) -> list[dict]:
    # O laço do _read_from, com a linha que derruba o RewriteFilter lida pelo parse_obj puro.
    rewrite = RewriteFilter()
    out = []
    with path.open("rb") as fh:
        while True:
            start = fh.tell()
            raw = fh.readline()
            if not raw:
                break
            text = raw.decode("utf-8", "replace")
            error = None
            try:
                evs = rewrite.parse_line(text)
            except UnicodeEncodeError as e:
                error = type(e).__name__
                evs = parse_obj(json.loads(text))
            for ev in evs:
                rec = {"offset": start, "event": event_dict(ev)}
                if error:
                    rec["python_error"] = error
                out.append(rec)
    return out


def history(path: Path, provider: str, queue_name: str) -> dict:
    out = {}
    for name, limit, window in VARIANTS:
        for suffix, session in (("", "sem-fila"), ("+queue", queue_name)):
            pqueue._TAIL_WINDOW = window
            evs = pqueue.merged_history(session, str(path), provider, limit)
            if limit is not None and limit > 0:
                evs = evs[-limit:]          # o corte que a rota /history faz (api.py:2918)
            out[name + suffix] = [event_dict(ev) for ev in evs]
    return out


def _ask_q(header, question, labels, multi=False, previews=None):
    previews = previews or {}
    return {"header": header, "question": question, "multiSelect": multi,
            "options": [{"label": lbl, "description": f"sobre {lbl}", "preview": previews.get(lbl, "")} for lbl in labels]}


PANE_PREVIEW = (
    "   Como deixo o meu?\n"
    "\n"
    " ❯ 1. System no topo (igual aos     ╭──────────────────────────────╮\n"
    "      irmãos)                        │ using System.Reflection;     │\n"
    "   2. Alfabético (obedece           │ using Xunit;                 │\n"
    "      .editorconfig)                 ╰──────────────────────────────╯\n"
    "   3. Type something.\n"
)
PANE_MULTI = (
    "   Quais cores?\n"
    "\n"
    " ❯ 1. [ ] Alfa\n"
    "   2. [x] Bravo\n"
    "   3. [ ] Type something\n"
    "   4. Chat about this\n"
)
PREVIEW_Q = _ask_q("Ordem", "Como deixo?", ["System no topo (igual aos irmãos)", "Alfabético (obedece .editorconfig)"],
                   previews={"System no topo (igual aos irmãos)": "using System.Reflection;"})

# Casamento do sidecar do AskUserQuestion com o menu do pane (`_ask_question_event`). `pane`: o
# estado e as opções saem do `classify` do Python (o Rust usa o `analyze` dele); sem `pane`, vêm
# prontos. `sidecar`: o arquivo cru, para os casos malformados.
ASK_CASES = [
    dict(name="match", state="awaiting_input", options=["A", "B", "Type something."], questions=[_ask_q("Cor", "Escolha", ["A", "B"], True), _ask_q("Fruta", "Escolha fruta", ["X", "Y"])]),
    dict(name="working", state="working", options=["A", "B"], questions=[_ask_q("Cor", "Escolha", ["A", "B"])]),
    dict(name="mismatch", state="awaiting_input", options=["Sim", "Nao"], questions=[_ask_q("Cor", "Escolha", ["A", "B"])]),
    dict(name="extra_real_option", state="awaiting_input", options=["Cancelar", "Sim", "Nao"], questions=[_ask_q("Confirma", "Vai?", ["Sim", "Nao"])]),
    dict(name="tui_extras", state="awaiting_input", options=["A", "B", "Type something.", "Chat about this"], questions=[_ask_q("Cor", "Escolha", ["A", "B"])]),
    dict(name="single", state="awaiting_input", options=["A"], questions=[_ask_q("Cor", "Escolha", ["A"])]),
    dict(name="no_options", state="awaiting_input", options=None, questions=[_ask_q("Cor", "Escolha", ["A"])]),
    dict(name="empty_first", state="awaiting_input", options=["A"], questions=[_ask_q("Cor", "Escolha", [])]),
    dict(name="preview_truncated", state="awaiting_input", options=["System no topo (igual aos", "Alfabético (obedece", "Type something.", "Chat about this"], questions=[PREVIEW_Q]),
    dict(name="preview_count_differs", state="awaiting_input", options=["System no topo (igual aos"], questions=[PREVIEW_Q]),
    dict(name="preview_pane_prefix_ok", state="awaiting_input", options=["Yes", "No"], questions=[_ask_q("P", "Q", ["Yes, and bypass", "No"], previews={"No": "x"})]),
    # O inverso é o cruzamento que o casamento proíbe: rótulo curto do sidecar contra o longo do pane.
    dict(name="preview_sidecar_short_no_cross", state="awaiting_input", options=["Yes, and bypass", "No"], questions=[_ask_q("P", "Q", ["Yes", "No"], previews={"No": "x"})]),
    dict(name="preview_same_count_not_prefix", state="awaiting_input", options=["Alfa", "Bravo"], questions=[_ask_q("P", "Q", ["Xis", "Bravo"], previews={"Xis": "x"})]),
    dict(name="preview_only_second_question", state="awaiting_input", options=["A trunc", "B"], questions=[_ask_q("P", "Q", ["A truncado", "B"]), _ask_q("R", "S", ["C"], previews={"C": "x"})]),
    dict(name="only_tui_extras", state="awaiting_input", options=["Type something.", "Chat about this"], questions=[_ask_q("Cor", "Escolha", ["A"])]),
    dict(name="extra_box_and_period", state="awaiting_input", options=["[ ] A", "[ ] Type something."], questions=[_ask_q("Cor", "Escolha", ["A"], True)]),
    dict(name="options_empty_list", state="awaiting_input", options=[], questions=[_ask_q("Cor", "Escolha", ["A"])]),
    dict(name="preview_empty_pane_label", state="awaiting_input", options=["", "B"], questions=[_ask_q("P", "Q", ["A", "B"], previews={"A": "x"})]),
    dict(name="pane_preview", pane=PANE_PREVIEW, questions=[PREVIEW_Q]),
    dict(name="pane_multi_box", pane=PANE_MULTI, questions=[_ask_q("Cores", "Quais cores?", ["Alfa", "Bravo"], True)]),
    dict(name="no_sidecar", state="awaiting_input", options=["A"], questions=None),
    dict(name="malformed_no_header", state="awaiting_input", options=["A"], sidecar={"tool_input": {"questions": [{"question": "Q", "options": [{"label": "A"}]}]}}),
    dict(name="malformed_not_json", state="awaiting_input", options=["A"], sidecar="{"),
    dict(name="malformed_no_tool_input", state="awaiting_input", options=["A"], sidecar={"questions": []}),
    dict(name="malformed_questions_not_list", state="awaiting_input", options=["A"], sidecar={"tool_input": {"questions": {"a": 1}}}),
    dict(name="malformed_option_no_label", state="awaiting_input", options=["A"], sidecar={"tool_input": {"questions": [{"header": "H", "question": "Q", "options": [{"description": "d"}]}]}}),
    dict(name="minimal_fields", state="awaiting_input", options=["A"], sidecar={"tool_input": {"questions": [{"header": "H", "question": "Q", "options": [{"label": "A"}]}]}}),
]


def ask_rows() -> list[dict]:
    # Os casos malformados fariam o leitor imprimir o traceback a cada geração.
    logging.getLogger("hangar.askquestion").disabled = True
    rows = []
    for case in ASK_CASES:
        if "pane" in case:
            status, _label, _question, options = state.classify(case["pane"])
        else:
            status, options = case["state"], case["options"]
        sidecar = case.get("sidecar")
        if sidecar is None and case.get("questions") is not None:
            sidecar = {"tool_input": {"questions": case["questions"]}}
        with tempfile.TemporaryDirectory() as tmp:
            jsonl = Path(tmp) / "projects" / "p" / "sid.jsonl"
            if sidecar is not None:
                (Path(tmp) / ".hangar-askq").mkdir()
                raw = sidecar if isinstance(sidecar, str) else json.dumps(sidecar, ensure_ascii=False)
                (Path(tmp) / ".hangar-askq" / "sid.json").write_text(raw, encoding="utf-8")
            ev = _ask_question_event(StateEvent(session="s", state=status, options=options).model_dump_json(), str(jsonl))
        row = {k: v for k, v in case.items() if k != "questions"}
        row.update(state=status, options=options, sidecar=sidecar,
                   expected=None if ev is None else json.loads(ev["data"]))
        rows.append(row)
    return rows


RULE = "─" * 60
# Painel ancorado à direita da conversa, a partir da coluna 40: sem o corte, a borda │ dele e o ●
# do mod entram na prévia.
DOCK_PANE = "\n".join([
    "❯ resume o arquivo".ljust(40) + "│ Mod",
    "".ljust(40) + "│ ● Progresso 3/5",
    "● Resposta em andamento com texto".ljust(40) + "│ item um",
    "  segunda linha da resposta".ljust(40) + "│ item dois",
    RULE, "❯ ", RULE, "  🤖 modelo"])
WIDE_PANE = "\n".join([
    "❯ traduz",
    "● 日本語のテキストです🤖 e mais texto depois",
    "  linha curta",
    RULE, "❯ ", RULE])
BAND_PANE = "\n".join([
    "❯ faça a tarefa",
    "● Texto da resposta em voo",
    "  continuação do texto",
    "",
    "● Progresso do mod: 3 de 5",
    "  ▓▓▓░░",
    RULE, "❯ ", RULE])
PREVIEW_PANES = [
    ("dock_sem_corte", DOCK_PANE, None, None),
    ("dock_cortado", DOCK_PANE, 40, None),
    ("largos_cortados", WIDE_PANE, 12, None),
    ("largos_borda_no_meio", WIDE_PANE, 7, None),
    ("faixa_sem_ancora", BAND_PANE, None, None),
    ("faixa_com_ancora", BAND_PANE, None, "Progresso do mod: 3 de 5"),
    ("ancora_ausente", BAND_PANE, None, "nada disso aparece"),
    ("corte_e_ancora", DOCK_PANE, 40, "Progresso"),
]
NOW = 1_000_000.0
# (nome, arquivos por pasta de config — texto cru ou None —, marcador (estado, ts) ou None)
PREVIEW_SIDECARS = [
    ("novo", [json.dumps({"text": "abc", "ts": NOW - 1})], None),
    ("ausente", [None], None),
    ("velho_marcador_trabalhando", [json.dumps({"text": "abc", "ts": NOW - 700})], ("working", NOW - 10)),
    ("velho_marcador_velho", [json.dumps({"text": "abc", "ts": NOW - 700})], ("working", NOW - 700)),
    ("velho_marcador_parado", [json.dumps({"text": "abc", "ts": NOW - 700})], ("idle", NOW - 10)),
    ("velho_sem_marcador", [json.dumps({"text": "abc", "ts": NOW - 700})], None),
    ("vazio_velho_honrado", [json.dumps({"text": "", "ts": NOW - 7000})], None),
    ("json_quebrado", ["{nao"], None),
    ("nao_objeto", ["[1, 2]"], None),
    ("texto_nao_str", [json.dumps({"text": 3, "ts": NOW})], None),
    ("sem_ts", [json.dumps({"text": "sem relogio"})], None),
    ("ts_texto", [json.dumps({"text": "x", "ts": "123"})], None),
    ("ts_inteiro_velho", [json.dumps({"text": "x", "ts": int(NOW) - 601})], None),
    ("segunda_pasta", [json.dumps({"text": "a", "ts": NOW - 700}), json.dumps({"text": "b", "ts": NOW})], None),
    ("primeira_vence", [json.dumps({"text": "a", "ts": NOW}), json.dumps({"text": "b", "ts": NOW})], None),
]
PREVIEW_COMMITTED = [
    ("curta", "abc def", "abc def ghi jkl mno"),
    ("contida_com_markdown", "Confirma a mudança no arquivo agora",
     "**Confirma** a mudança no `arquivo` agora e depois sigo"),
    ("gravado_e_prefixo", "Texto já gravado aqui completo\nMaking 1 scratchpad edit…", "Texto já gravado aqui completo"),
    ("outra_coisa", "Um texto que não tem nada a ver", "Resposta completamente diferente desta"),
    ("lista_pintada", "• item um bem longo\n• item dois", "- item um bem longo\n- item dois"),
    ("sem_gravado", "Um texto qualquer bem comprido", ""),
    ("quebra_da_tela", "linha que o terminal\n  quebrou no meio da frase", "linha que o terminal quebrou no meio da frase."),
]


def preview_rows() -> dict:
    import tempfile
    from unittest import mock

    from app import preview

    panes = []
    for name, pane, columns, anchor in PREVIEW_PANES:
        cropped = pane if columns is None else "\n".join(preview.crop_cells(ln, columns) for ln in pane.split("\n"))
        panes.append({"name": name, "pane": pane, "columns": columns, "anchor": anchor,
                      "expected": preview.extract_assistant_text(cropped, "claude", anchor)})
    sidecars = []
    with mock.patch.object(preview.time, "time", return_value=NOW):
        for name, files, marker in PREVIEW_SIDECARS:
            with tempfile.TemporaryDirectory() as tmp:
                dirs = []
                for i, raw in enumerate(files):
                    d = Path(tmp) / str(i)
                    (d / ".hangar-preview").mkdir(parents=True)
                    if raw is not None:
                        (d / ".hangar-preview" / "sid.json").write_text(raw, encoding="utf-8")
                    dirs.append(d)
                # A ordem das pastas aqui é entrada do caso; em produção ela vem de um conjunto.
                with (mock.patch.object(preview, "_config_dirs", return_value=dirs),
                      mock.patch.object(preview.hook_state, "get_state", return_value=marker)):
                    sidecars.append({"name": name, "files": files, "marker": marker, "now": NOW,
                                     "expected": preview.read_sidecar("sid")})
    committed = [{"name": name, "preview": text, "committed": raw, "norm": preview._norm(raw),
                  "expected": sse.preview_is_committed(text, preview._norm(raw))}
                 for name, text, raw in PREVIEW_COMMITTED]
    return {"panes": panes, "sidecars": sidecars, "committed": committed}


# ── /input e /steer: o corpo HTTP que o Python devolve para cada resposta do ator ─────────────────
# Chama o `input_prompt` e o `steer_session` de verdade, com um coordenador falso que responde a
# disposição do caso. O Rust lê estes arquivos e tem de responder o mesmo corpo e o mesmo código.
# Divergências deliberadas, já espelhadas na rota do Python:
# - /steer sem terminal recusado pelo ator vira 409 `erro_sem_turno` (antes o ValueError virava 500);
# - /steer com terminal, falha do runtime no controle ou no confirm vira 502 `erro_envio_falhou` (antes 500).
# Diferenças que o golden NÃO compara: o `msg` de falha do runtime (Rust: `codigo: mensagem`; Python
# real: "IPC recusou a operação (...)") e o `codigo` do diário `runtime.send_failed` (Rust: código do
# runtime; Python: nome da classe da exceção). Aqui a exceção do ator já traz o texto `codigo: mensagem`.
INPUT_OP = "OP"
# (nome, terminal, texto, steer, resposta, fila-promovida). Resposta é (disposição, payload) ou o
# texto da exceção que o ator levanta; fila-promovida é a resposta do `steer_queue`, quando ele roda.
INPUT_CASES = [
    ("headless_accepted", False, "oi", False, ("accepted", {}), None),
    ("headless_accepted_native", False, "oi", False, ("accepted", {"native": True}), None),
    ("headless_accepted_slash", False, "/model opus", False, ("accepted", {}), None),
    ("headless_deferred", False, "oi", False, ("deferred", {}), None),
    ("headless_unknown", False, "oi", False, ("unknown", {}), None),
    ("headless_unknown_transport_lost", False, "oi", False,
     ("unknown", {"transport_lost": True, "code": "transport_lost"}), None),
    ("headless_rejected", False, "oi", False, ("rejected", {}), None),
    ("headless_op_error", False, "oi", False, "runtime_closed: ator saiu", None),
    ("headless_steer_accepted", False, "oi", True, ("accepted", {}), None),
    ("headless_steer_deferred_promoted", False, "oi", True, ("deferred", {}), ("accepted", {"ids": [INPUT_OP]})),
    ("headless_steer_deferred_other_ids", False, "oi", True, ("deferred", {}), ("accepted", {"ids": ["outro"]})),
    ("headless_steer_deferred_rejected", False, "oi", True, ("deferred", {}), ("rejected", {"error": "sem turno"})),
    ("headless_steer_deferred_unknown", False, "oi", True, ("deferred", {}), ("unknown", {})),
    ("headless_steer_deferred_op_error", False, "oi", True, ("deferred", {}), "runtime_closed: ator saiu"),
    ("headless_steer_slash_deferred", False, "/model", True, ("deferred", {}), None),
    ("headless_steer_unknown", False, "oi", True, ("unknown", {}), None),
    ("terminal_accepted", True, "oi", False, ("accepted", {}), None),
    ("terminal_accepted_native", True, "oi", False, ("accepted", {"native": True}), None),
    ("terminal_accepted_slash", True, "/model", False, ("accepted", {}), None),
    ("terminal_deferred_text", True, "oi", False, ("deferred", {}), None),
    ("terminal_deferred_slash_coded", True, "/model opus", False, ("deferred", {"code": "busy"}), None),
    ("terminal_deferred_slash_plain", True, "  /model", False, ("deferred", {}), None),
    ("terminal_unknown_text", True, "oi", False, ("unknown", {}), None),
    ("terminal_unknown_text_coded", True, "oi", False, ("unknown", {"code": "pane_gone"}), None),
    ("terminal_unknown_text_steer", True, "oi", True, ("unknown", {}), None),
    ("terminal_unknown_slash", True, "/model", False, ("unknown", {}), None),
    ("terminal_rejected", True, "oi", False, ("rejected", {}), None),
    ("terminal_steer_accepted", True, "oi", True, ("accepted", {}), None),
    ("terminal_steer_deferred", True, "oi", True, ("deferred", {}), None),
    ("terminal_op_error", True, "oi", False, "terminal_closed: pane saiu", None),
]

# (nome, terminal, corpo, resposta do control, resposta do confirm). Corpo None = pedido sem corpo.
STEER_CASES = [
    ("headless_text_accepted", False, "oriente", ("accepted", {}), None),
    ("headless_text_rejected_coded", False, "oriente", ("rejected", {"error": "sem turno"}), None),
    ("headless_text_rejected_plain", False, "oriente", ("rejected", {}), None),
    ("headless_text_unknown", False, "oriente", ("unknown", {}), None),
    ("headless_text_deferred", False, "oriente", ("deferred", {}), None),
    ("headless_text_op_error", False, "oriente", "runtime_closed: ator saiu", None),
    ("headless_queue_accepted", False, None, ("accepted", {"ids": ["a", "b"]}), None),
    ("headless_queue_empty", False, None, ("accepted", {"ids": []}), None),
    ("headless_queue_rejected", False, None, ("rejected", {}), None),
    ("headless_queue_op_error", False, None, "runtime_closed: ator saiu", None),
    ("terminal_promoted", True, None, ("accepted", {}), {"confirmed": 2}),
    ("terminal_not_promoted", True, None, ("accepted", {"promoted": False}), {"confirmed": 0}),
    ("terminal_no_confirmed_count", True, None, ("accepted", {}), {}),
    ("terminal_body_is_ignored", True, "oriente", ("accepted", {}), {"confirmed": 1}),
    ("terminal_deferred", True, None, ("deferred", {}), None),
    ("terminal_rejected", True, None, ("rejected", {}), None),
    ("terminal_unknown", True, None, ("unknown", {}), None),
    ("terminal_op_error", True, None, "terminal_closed: pane saiu", None),
    ("terminal_confirm_error", True, None, ("accepted", {}), "terminal_closed: pane saiu"),
]

# Codex sem terminal: qualquer recusa do ator é 409 `erro_codex_controle` com a frase do Codex.
CODEX_STEER_CASES = [
    ("codex_text_accepted", False, "oriente", ("accepted", {}), None),
    ("codex_text_rejected_coded", False, "oriente", ("rejected", {"error": "sem turno"}), None),
    ("codex_text_unknown", False, "oriente", ("unknown", {}), None),
    ("codex_text_deferred", False, "oriente", ("deferred", {}), None),
    ("codex_text_op_error", False, "oriente", "runtime_closed: ator saiu", None),
    ("codex_queue_accepted", False, None, ("accepted", {"ids": ["a", "b"]}), None),
    ("codex_queue_rejected", False, None, ("rejected", {"error": "sem turno"}), None),
    ("codex_queue_op_error", False, None, "runtime_closed: ator saiu", None),
]


def session_write_rows() -> tuple[list, list]:
    import asyncio
    import threading
    from types import SimpleNamespace

    from fastapi import HTTPException
    from app import api, runtime_coordinator
    from app.runtime_adapter import RuntimeAdapter, RuntimeView

    # O envio com terminal roda numa thread e fala com o coordenador pelo loop dele (`run_sync`).
    loop = asyncio.new_event_loop()
    threading.Thread(target=loop.run_forever, daemon=True).start()

    class Owner:
        instance = "runtime"
        legacy = object()

        def __init__(self, terminal, main, queue=None, confirm=None):
            self.terminal, self.main, self.queue, self.confirm, self.loop = terminal, main, queue, confirm, loop

        def slot(self, name):
            return SimpleNamespace(binding=SimpleNamespace(provider="claude", meta={"terminal": {"pane": "%1"}} if self.terminal else {}))

        def managed_runtime(self, name):
            return True

        async def prepare_session(self, name, provider, *, launch=False, engine_models=None):
            return True

        async def op(self, name, command, operation_id):
            if command["kind"] == "confirm":
                if isinstance(self.confirm, str):
                    raise RuntimeError(self.confirm)
                return self.confirm or {}
            chosen = self.queue if command["kind"] == "control" and command["control"] == "steer_queue" and self.queue else self.main
            if isinstance(chosen, str):
                raise RuntimeError(chosen)
            return {"operation_id": operation_id, "disposition": chosen[0], "payload": chosen[1]}

    diary = []
    # Só os eventos da rota: sem tmux no ambiente, o módulo dele também escreve no diário (`mux.*`).
    api.diag.registrar = lambda event, level="ok", **fields: (event.startswith(("runtime.", "opcao."))
        and diary.append({"event": event, "code": fields.get("codigo")}))
    api.uuid = SimpleNamespace(uuid4=lambda: SimpleNamespace(hex=INPUT_OP))
    api._recusa_orq = lambda name: None
    api._session_exists = lambda name: True
    api._provider_of = lambda name: "claude"
    api._pane_info = lambda name: ("claude", "%1")

    def as_json(value):
        return None if value is None else value if isinstance(value, str) else {"disposition": value[0], "payload": value[1]}

    async def run(call, terminal, owner, provider="claude"):
        runtime_coordinator._current = owner
        api._headless = lambda name: not terminal
        api._provider_of = lambda name: provider
        adapter = RuntimeAdapter(provider)
        adapter.view = lambda name, mutating=False: RuntimeView("k", 1, 1, {})
        api.get_adapter = lambda key: SimpleNamespace(
            steer=lambda name, text: adapter.dispatch("steer", name, {"text": text}),
            steer_queue=lambda name, *, entry_id=None: adapter.dispatch("steer_queue", name, {"entry_id": entry_id}))
        diary.clear()
        try:
            return {"status": 200, "body": await call()}
        except HTTPException as exc:
            return {"status": exc.status_code, "body": {"detail": exc.detail}}
        except api.TerminalControlError as exc:
            response = await api.terminal_control_failed(None, exc)
            return {"status": response.status_code, "body": json.loads(response.body)}

    async def main():
        inputs, steers = [], []
        for name, terminal, text, steer, reply, queue in INPUT_CASES:
            expect = await run(lambda: api.input_prompt("s", api.InputBody(text=text, steer=steer)),
                               terminal, Owner(terminal, reply, queue))
            inputs.append({"name": name, "terminal": terminal, "text": text, "steer": steer, "reply": as_json(reply),
                           "queue": as_json(queue), "expect": expect, "diary": list(diary)})
        cases = [(*case, "claude") for case in STEER_CASES] + [(*case, "codex") for case in CODEX_STEER_CASES]
        for name, terminal, text, reply, confirm, provider in cases:
            body = None if text is None else api.InputBody(text=text)
            expect = await run(lambda: api.steer_session("s", body), terminal, Owner(terminal, reply, None, confirm), provider)
            steers.append({"name": name, "provider": provider, "terminal": terminal, "text": text, "reply": as_json(reply),
                           "confirm": confirm, "expect": expect})
        return inputs, steers

    return asyncio.run(main())


# ── /interrupt, /keys, /term-input, /select, /select/submit e DELETE …/queue/{id} ──────────────────
# Mesmo método: a rota do Python de verdade, com o ator falso. Cada caso diz a rota, o que o plugin
# segura (`pending`), se o painel está aberto e a resposta do ator (`reply`): uma disposição real, ou
# "!erro: msg" para a falha do runtime. Divergências deliberadas, já espelhadas nas rotas do Python:
# - /select com terminal: ator `unknown` -> 409 `erro_sem_confirmacao_resposta`; `rejected` -> 409
#   "não consegui marcar essa opção…" com `detalhe` e diário `opcao.nao_convergiu` (antes: "Não foi
#   possível confirmar o controle…" sem `detalhe`, que o app mostrava como "(undefined)");
# - /select/submit com terminal: `rejected` -> 409 "não consegui enviar as opções marcadas…" com
#   `detalhe` e diário `opcao.envio_falhou`;
# - /select sem terminal: ator sem permissão pendente (erro `no_pending_permission`) -> 409 "nenhum
#   pedido de permissão pendente" (antes: 503); recusa, adiamento ou incerteza seguem 503 com o motivo;
# - falha do runtime em /keys, /term-input, /select/submit, /interrupt com terminal e DELETE …/queue
#   -> 502 `erro_envio_falhou` (antes: 500);
# - /interrupt sem terminal: ator recusou, incerto ou falhou -> 409 `erro_sem_turno` com o motivo (antes: 500).
# Diferença que o golden NÃO compara: o texto de `params.detalhe`.
def _c(name, route, **kw):
    return {"name": name, "route": route, "provider": "claude", "terminal": True, "pending": None, "panel_open": False,
            "reply": ("accepted", {}), "rust_reply": None, "args": {}, **kw}


ACC = ("accepted", {})
CONTROL_CASES = [
    _c("select_accepted", "select", args={"option": 2}),
    _c("select_perm_yes", "select", args={"option": 1}, pending={"id": "perm:1"}),
    _c("select_perm_no_with_panel_open", "select", args={"option": 2}, pending={"id": "perm:1"}, panel_open=True),
    _c("select_perm_option_3", "select", args={"option": 3}, pending={"id": "perm:1"}),
    _c("select_ask_pending", "select", args={"option": 2}, pending={"id": "ask:1"}),
    _c("select_panel_open_no_pending", "select", args={"option": 1}, panel_open=True),
    _c("select_panel_open_ask", "select", args={"option": 1}, pending={"id": "ask:1"}, panel_open=True),
    _c("select_deferred", "select", args={"option": 1}, reply=("deferred", {})),
    _c("select_uncertain", "select", args={"option": 1}, reply=("unknown", {})),
    _c("select_runtime_error", "select", args={"option": 1}, reply="!erro: runtime_closed: ator saiu"),
    _c("select_refused", "select", args={"option": 1}, reply=("rejected", {"code": "cursor"})),
    _c("select_headless_accepted", "select", terminal=False, args={"option": 1}),
    _c("select_headless_no_permission", "select", terminal=False, args={"option": 1}, reply="!no_pending"),
    _c("select_headless_rejected", "select", terminal=False, args={"option": 1}, reply=("rejected", {"error": "opção inválida"})),
    _c("select_headless_deferred", "select", terminal=False, args={"option": 1}, reply=("deferred", {})),
    _c("select_headless_unknown", "select", terminal=False, args={"option": 1}, reply=("unknown", {})),
    _c("select_headless_error", "select", terminal=False, args={"option": 1}, reply="!erro: runtime_closed: ator saiu"),
    _c("submit_accepted", "select_submit"),
    _c("submit_panel_open", "select_submit", panel_open=True),
    _c("submit_deferred", "select_submit", reply=("deferred", {})),
    _c("submit_unknown", "select_submit", reply=("unknown", {})),
    _c("submit_refused", "select_submit", reply=("rejected", {"code": "tab"})),
    _c("submit_runtime_error", "select_submit", reply="!erro: runtime_closed: ator saiu"),
    _c("interrupt_headless_accepted", "interrupt", terminal=False),
    _c("interrupt_headless_no_turn", "interrupt", terminal=False, reply=("accepted", {"interrupted": False})),
    _c("interrupt_headless_rejected", "interrupt", terminal=False, reply=("rejected", {"error": "sem turno"})),
    _c("interrupt_headless_rejected_plain", "interrupt", terminal=False, reply=("rejected", {})),
    _c("interrupt_headless_deferred", "interrupt", terminal=False, reply=("deferred", {})),
    _c("interrupt_headless_unknown", "interrupt", terminal=False, reply=("unknown", {})),
    _c("interrupt_headless_error", "interrupt", terminal=False, reply="!erro: runtime_closed: ator saiu"),
    _c("interrupt_terminal_runtime_error", "interrupt", pending={"id": "ask:9"}, reply="!erro: runtime_closed: ator saiu"),
    _c("interrupt_terminal_accepted", "interrupt"),
    _c("interrupt_terminal_with_question", "interrupt", pending={"id": "ask:9"}),
    _c("interrupt_terminal_clear", "interrupt", args={"clear": True}, pending={"id": "ask:9"}),
    _c("interrupt_terminal_deferred", "interrupt", pending={"id": "ask:9"}, reply=("deferred", {})),
    _c("interrupt_terminal_unknown", "interrupt", pending={"id": "ask:9"}, reply=("unknown", {})),
    _c("keys_accepted", "keys", args={"key": "Down"}),
    _c("keys_not_allowed", "keys", args={"key": "Nope"}, reply=None, rust_reply=("rejected", {"code": "key_not_allowed"})),
    _c("keys_deferred", "keys", args={"key": "Down"}, reply=("deferred", {})),
    _c("keys_unknown", "keys", args={"key": "Down"}, reply=("unknown", {})),
    _c("keys_runtime_error", "keys", args={"key": "Down"}, reply="!erro: runtime_closed: ator saiu"),
    _c("term_text", "term_input", args={"text": "ls"}),
    _c("term_key", "term_input", args={"key": "C-c"}),
    _c("term_text_and_key", "term_input", args={"text": "ls", "key": "Enter"}),
    _c("term_empty_text_and_key", "term_input", args={"text": "", "key": "Enter"}),
    _c("term_nothing", "term_input", args={}),
    _c("term_bad_text", "term_input", args={"text": "a\x01b"}, reply=None),
    _c("term_bad_key_after_text", "term_input", args={"text": "ls", "key": "Nope"}, rust_reply=("rejected", {"code": "key_not_allowed"})),
    _c("term_text_deferred", "term_input", args={"text": "ls", "key": "Enter"}, reply=("deferred", {})),
    _c("term_key_unknown", "term_input", args={"key": "Enter"}, reply=("unknown", {})),
    _c("term_runtime_error", "term_input", args={"text": "ls"}, reply="!erro: runtime_closed: ator saiu"),
    _c("queue_removed", "queue_remove", terminal=False, args={"removed": True}),
    _c("queue_not_found", "queue_remove", terminal=False, args={"removed": False}),
    _c("queue_runtime_error", "queue_remove", terminal=False, args={"error": "runtime_closed: ator saiu"}),
    # Codex sem terminal: interrupção sem turno ou recusada é 409 `erro_codex_controle` (falha antes: 500).
    _c("codex_interrupt_accepted", "interrupt", provider="codex", terminal=False),
    _c("codex_interrupt_no_turn", "interrupt", provider="codex", terminal=False, reply=("accepted", {"interrupted": False})),
    _c("codex_interrupt_rejected", "interrupt", provider="codex", terminal=False, reply=("rejected", {"error": "sem turno"})),
    _c("codex_interrupt_unknown", "interrupt", provider="codex", terminal=False, reply=("unknown", {})),
    _c("codex_interrupt_error", "interrupt", provider="codex", terminal=False, reply="!erro: runtime_closed: ator saiu"),
    _c("codex_select_accepted", "select", provider="codex", terminal=False, args={"option": 1}),
    _c("codex_select_no_permission", "select", provider="codex", terminal=False, args={"option": 1}, reply="!no_pending"),
    _c("codex_select_rejected", "select", provider="codex", terminal=False, args={"option": 1}, reply=("rejected", {"error": "opção inválida"})),
    _c("codex_select_error", "select", provider="codex", terminal=False, args={"option": 1}, reply="!erro: runtime_closed: ator saiu"),
    _c("codex_queue_removed", "queue_remove", provider="codex", terminal=False, args={"removed": True}),
    _c("codex_queue_not_found", "queue_remove", provider="codex", terminal=False, args={"removed": False}),
]


def control_rows() -> list:
    import asyncio
    import threading
    from types import SimpleNamespace

    from fastapi import HTTPException
    from app import api, runtime_coordinator, termsock
    from app.rust_server import RustOpError
    from app.runtime_adapter import RuntimeAdapter, RuntimeView

    loop = asyncio.new_event_loop()
    threading.Thread(target=loop.run_forever, daemon=True).start()
    sent, notified, diary = [], [], []

    class Owner:
        instance = "runtime"
        legacy = object()

        def __init__(self, terminal, reply):
            self.terminal, self.reply, self.loop = terminal, reply, loop

        def slot(self, name):
            return SimpleNamespace(binding=SimpleNamespace(provider="claude", meta={"terminal": {"pane": "%1"}} if self.terminal else {}))

        def managed_runtime(self, name):
            return True

        async def prepare_session(self, name, provider, *, launch=False, engine_models=None):
            return True

        async def op(self, name, command, operation_id):
            if command["kind"] == "control":
                sent.append({"control": command["control"], "payload": command["payload"]})
            if self.reply == "!no_pending":
                raise RustOpError("IPC recusou a operação (400: no_pending_permission)", 400, "no_pending_permission")
            if isinstance(self.reply, str):
                raise RuntimeError(self.reply.removeprefix("!erro: "))
            return {"operation_id": operation_id, "disposition": self.reply[0], "payload": self.reply[1]}

    api.diag.registrar = lambda event, level="ok", **fields: event.startswith(("runtime.", "opcao.")) and diary.append(event)
    api._recusa_orq = lambda name: None
    api._session_exists = lambda name: True
    api._provider_of = lambda name: "claude"
    api._cached_info_sync = lambda name: SimpleNamespace(provider="claude", headless=False)
    api._loop_servidor = loop
    api.plugin_bridge.interrompeu = lambda name, ident: notified.append(ident)

    async def call(case):
        args = case["args"]
        if case["route"] == "select":
            return await asyncio.to_thread(api.select, "s", api.SelectBody(**args))
        if case["route"] == "select_submit":
            return await asyncio.to_thread(api.select_submit, "s")
        if case["route"] == "interrupt":
            return await api.interrupt("s", **args)
        if case["route"] == "keys":
            return await asyncio.to_thread(api.keys, "s", api.KeyBody(**args))
        if case["route"] == "term_input":
            return await asyncio.to_thread(api.term_input, "s", api.TermInputBody(**args))
        class Queue:
            def __init__(self, name):
                pass

            def remove(self, entry_id):
                if "error" in args:
                    raise RuntimeError(args["error"])
                return args["removed"]
        api.PromptQueue = Queue
        return await api.descartar_da_fila("s", "e1")

    async def run(case):
        reply = case["reply"]
        runtime_coordinator._current = Owner(case["terminal"], reply)
        api._headless = lambda name: not case["terminal"] and case["provider"] == "claude"
        api._provider_of = lambda name: case["provider"]
        api._cached_info_sync = lambda name: SimpleNamespace(provider=case["provider"], headless=not case["terminal"])
        api._codex_sem_terminal = lambda name: case["provider"] == "codex" and not case["terminal"]
        adapter = RuntimeAdapter(case["provider"])
        adapter.view = lambda name, mutating=False: RuntimeView("k", 1, 1, {})
        api.get_adapter = lambda key: SimpleNamespace(select=lambda name, option: adapter.dispatch("select", name, {"option": option}),
                                                      interrupt=lambda name: adapter.dispatch("interrupt", name, {}))
        api.plugin_bridge.pergunta_pendente = lambda name: case["pending"]
        termsock.painel_aberto = lambda name: case["panel_open"]
        sent.clear(), notified.clear(), diary.clear()
        try:
            return {"status": 200, "body": await call(case)}
        except HTTPException as exc:
            return {"status": exc.status_code, "body": {"detail": exc.detail}}
        except api.TerminalControlError as exc:
            response = await api.terminal_control_failed(None, exc)
            return {"status": response.status_code, "body": json.loads(response.body)}

    def as_json(value):
        return None if value is None else value if isinstance(value, str) else {"disposition": value[0], "payload": value[1]}

    async def main():
        rows = []
        for case in CONTROL_CASES:
            expect = await run(case)
            rows.append({"name": case["name"], "route": case["route"], "provider": case["provider"], "terminal": case["terminal"], "args": case["args"],
                         "pending": case["pending"], "panel_open": case["panel_open"], "reply": as_json(case["reply"]),
                         "rust_reply": as_json(case["rust_reply"]), "expect": expect,
                         "sent": list(sent), "notified": list(notified), "diary": list(diary)})
        return rows

    return asyncio.run(main())


# ── /answer ─────────────────────────────────────────────────────────────────────────────────────────
# Mesmo método: a rota do Python de verdade (`api.answer` → `answer_sync` → coordenador falso com o
# Rust de pé). Único substituto: `run_admin`, que empresta o teclado só para o Esc do "Conversar"; o
# Rust não empresta (manda `interrupt` e depois `submit` pelo ator), então o Esc entra em `sent` como
# o controle `interrupt`. Divergências deliberadas, já espelhadas no Python:
# - falha do runtime com terminal (op do coordenador) -> 502 `erro_envio_falhou` (antes: 500);
# - "Conversar" com o texto não confirmado (qualquer disposição fora de accepted/deferred) -> 502
#   `erro_envio_falhou` "a pergunta foi fechada, mas a resposta por texto não foi confirmada — confira
#   na sessão antes de responder de novo" (antes: 500, ou 500 com outro texto quando o ator
#   recusava/ficava incerto). Não é 409: o app lê 409 como "nada digitado", e aqui a pergunta já
#   fechou e o texto pode ter entrado;
# - Esc do "Conversar" não aceito (`interrupt_reply`): adiado/incerto/recusado -> 409
#   `erro_opcao_nao_convergiu` (como o `TerminalControlError` do Python), falha do runtime -> 502;
#   nada é digitado;
# - headless sem `request_id`: o Python aceita; o Rust repassa o pedido a ele (`relays_headless`).
# - sem terminal, o ator recusa a resposta com o código `claude_command` -> 409
#   `erro_codex_resposta_invalida` (antes: 503). Diferença que o golden NÃO compara: `params.detalhe`.
ANS_OPT = {"kind": "option", "indices": [0], "labels": ["A"]}
ANS_MULTI = {"kind": "option", "indices": [0, 2], "multi": True, "labels": ["A", "C"]}
ANS_TEXT = {"kind": "text", "value": "oi", "type_index": 3}
ANS_CHAT = {"kind": "chat", "chat_index": 4}
ASK_SIDECAR = ["Cor?", "Tamanho?"]


def _ans(name, **kw):
    return {"name": name, "provider": "claude", "terminal": True, "answers": [ANS_OPT], "request_id": None, "pending": None,
            "panel_open": False, "sidecar": None, "reply": ACC, "submit_reply": ACC, "interrupt_reply": ACC, **kw}


ANSWER_CASES = [
    _ans("ok_option", pending={"id": "ask:1"}, request_id="ask:1"),
    _ans("ok_without_pending_or_id"),
    _ans("ok_all_kinds_with_pending", pending={"id": "ask:1"}, answers=[ANS_OPT, ANS_MULTI, ANS_TEXT, ANS_CHAT]),
    _ans("ok_id_without_prefix_gets_ask", pending={"id": "7"}, request_id="7"),
    _ans("ok_id_given_without_ask_prefix", pending={"id": "ask:7"}, request_id="7"),
    _ans("ok_permission_id_kept", pending={"id": "perm:1"}),
    _ans("ok_id_from_body_without_pending", request_id="ask:9"),
    _ans("ok_empty_id_falls_back_to_pending", pending={"id": "ask:1"}, request_id=""),
    _ans("ok_panel_open_but_question_held", pending={"id": "ask:1"}, panel_open=True),
    _ans("panel_open_without_pending", panel_open=True),
    _ans("id_changed", pending={"id": "ask:1"}, request_id="ask:2"),
    _ans("numeric_id_never_matches", pending={"id": "ask:1"}, request_id=1),
    _ans("numeric_id_without_pending", request_id=5),
    _ans("empty_answers", answers=[]),
    _ans("text_without_value", answers=[{"kind": "text", "type_index": 1}]),
    _ans("text_with_control_character", answers=[{"kind": "text", "value": "a\x01b", "type_index": 1}]),
    _ans("text_with_newline", answers=[{"kind": "text", "value": "a\nb", "type_index": 1}]),
    _ans("text_blank", answers=[{"kind": "text", "value": "  ", "type_index": 1}]),
    _ans("text_with_c1_control", answers=[{"kind": "text", "value": "a\u0085b", "type_index": 1}]),
    _ans("text_without_type_index", answers=[{"kind": "text", "value": "oi"}]),
    _ans("text_position_out_of_range", answers=[{"kind": "text", "value": "oi", "type_index": 100}]),
    _ans("chat_without_index", answers=[{"kind": "chat"}]),
    _ans("option_without_indices", answers=[{"kind": "option", "labels": ["A"]}]),
    _ans("option_with_empty_indices", answers=[{"kind": "option", "indices": [], "labels": ["A"]}]),
    _ans("option_negative_index", answers=[{"kind": "option", "indices": [-1], "labels": ["A"]}]),
    _ans("option_index_out_of_range", answers=[{"kind": "option", "indices": [100], "labels": ["A"]}]),
    _ans("option_without_labels", answers=[{"kind": "option", "indices": [0]}]),
    _ans("option_two_indices_not_multi", answers=[{"kind": "option", "indices": [0, 1], "labels": ["A", "B"]}]),
    _ans("unknown_kind", answers=[{"kind": "foo"}]),
    _ans("unknown_kind_with_quote", answers=[{"kind": "it's"}]),
    _ans("deferred", reply=("deferred", {})),
    _ans("rejected", reply=("rejected", {"code": "cursor"})),
    _ans("uncertain", reply=("unknown", {})),
    _ans("runtime_error", reply="!erro: runtime_closed: ator saiu"),
    _ans("chat_with_answers", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR),
    _ans("chat_with_text_answer_no_sidecar", answers=[ANS_TEXT, ANS_CHAT]),
    _ans("chat_nothing_to_preserve", answers=[ANS_CHAT], sidecar=ASK_SIDECAR),
    _ans("chat_with_pending_goes_to_the_control", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, pending={"id": "ask:1"}),
    _ans("chat_submit_deferred", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, submit_reply=("deferred", {})),
    _ans("chat_submit_rejected", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, submit_reply=("rejected", {"code": "x"})),
    _ans("chat_submit_uncertain", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, submit_reply=("unknown", {})),
    _ans("chat_submit_runtime_error", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, submit_reply="!erro: runtime_closed: ator saiu"),
    _ans("chat_panel_open", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, panel_open=True),
    _ans("chat_interrupt_deferred", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, interrupt_reply=("deferred", {})),
    _ans("chat_interrupt_uncertain", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, interrupt_reply=("unknown", {})),
    _ans("chat_interrupt_rejected", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, interrupt_reply=("rejected", {"code": "x"})),
    _ans("chat_interrupt_runtime_error", answers=[ANS_OPT, ANS_CHAT], sidecar=ASK_SIDECAR, interrupt_reply="!erro: runtime_closed: ator saiu"),
    _ans("headless_without_request_id", terminal=False),
    _ans("headless_accepted", terminal=False, request_id="r1", answers=[ANS_OPT, ANS_TEXT, ANS_CHAT]),
    _ans("headless_rejected", terminal=False, request_id="r1", reply=("rejected", {"error": "x"})),
    _ans("headless_deferred", terminal=False, request_id="r1", reply=("deferred", {})),
    _ans("headless_uncertain", terminal=False, request_id="r1", reply=("unknown", {})),
    _ans("headless_refused_by_the_actor", terminal=False, request_id="r1", reply="!erro: claude_command: a pergunta mudou"),
    _ans("headless_runtime_error", terminal=False, request_id="r1", reply="!erro: runtime_closed: ator saiu"),
    # Codex sem terminal: recusa do ator (`codex_command`) e falha são 503 com a frase do Codex.
    _ans("codex_accepted", provider="codex", terminal=False, request_id="r1", answers=[ANS_OPT, ANS_TEXT]),
    _ans("codex_rejected", provider="codex", terminal=False, request_id="r1", reply=("rejected", {"error": "x"})),
    _ans("codex_deferred", provider="codex", terminal=False, request_id="r1", reply=("deferred", {})),
    _ans("codex_uncertain", provider="codex", terminal=False, request_id="r1", reply=("unknown", {})),
    _ans("codex_refused_by_the_actor", provider="codex", terminal=False, request_id="r1", reply="!erro: codex_command: a pergunta mudou"),
    _ans("codex_runtime_error", provider="codex", terminal=False, request_id="r1", reply="!erro: runtime_closed: ator saiu"),
]

# Linhas do "Conversar sobre isso": respostas dadas + o que o sidecar sabe das perguntas.
CHAT_TEXT_CASES = [
    ("one_chat_one_answer", [ANS_OPT, ANS_CHAT], ["Cor?", "Tamanho?"]),
    ("several_chats", [ANS_OPT, ANS_CHAT, {"kind": "chat", "chat_index": 5}], ["Cor?", "Tamanho?", "Forma?"]),
    ("no_sidecar", [ANS_OPT, ANS_CHAT], None),
    ("sidecar_shorter_than_answers", [ANS_OPT, ANS_TEXT, ANS_CHAT], ["Cor?"]),
    ("chat_without_question_text", [ANS_OPT, ANS_CHAT], ["Cor?", ""]),
    ("only_chat", [ANS_CHAT], ["Cor?"]),
    ("only_empty_answers", [{"kind": "text", "value": "", "type_index": 1}, ANS_CHAT], ["Cor?"]),
    ("text_answer", [ANS_TEXT, ANS_CHAT], ["Cor?", "Tamanho?"]),
    ("multi_labels_joined", [ANS_MULTI, ANS_CHAT], ["Cor?", "Tamanho?"]),
    ("option_without_labels_is_skipped", [{"kind": "option", "indices": [0]}, ANS_OPT, ANS_CHAT], ["Cor?", "Tamanho?", "Forma?"]),
    ("answer_without_question_is_a_bare_line", [ANS_OPT, ANS_TEXT], None),
    ("no_chat_at_all", [ANS_OPT, ANS_TEXT], ["Cor?", "Tamanho?"]),
    ("accents_and_quotes", [{"kind": "text", "value": "não «sei»", "type_index": 1}, ANS_CHAT], ["Qual é o ônus?", "Tamanho?"]),
]


def chat_text_rows() -> list:
    from types import SimpleNamespace
    from app import api

    rows = []
    for name, answers, questions in CHAT_TEXT_CASES:
        full = [{"kind": a["kind"], "value": a.get("value"), "labels": a.get("labels") or []} for a in answers]
        api.read_pending_askq = lambda jsonl, q=questions: None if q is None else SimpleNamespace(
            questions=[SimpleNamespace(question=x) for x in q])
        rows.append({"name": name, "answers": full, "questions": questions,
                     "expect": api._askq_conversar_text(full, "/c/projects/p/sid.jsonl")})
    return rows


def answer_rows() -> list:
    import asyncio
    import threading
    from types import SimpleNamespace

    from fastapi import HTTPException
    from app import api, runtime_terminal, termsock, terminal_input as ti
    from app.adapters import CLAUDE_HEADLESS
    from app.rust_server import RustOpError
    from app.runtime_adapter import RuntimeAdapter, RuntimeView
    from app.runtime_coordinator import Phase
    from app import runtime_coordinator

    loop = asyncio.new_event_loop()
    threading.Thread(target=loop.run_forever, daemon=True).start()
    sent, cleared = [], []

    class Owner:
        instance = "runtime"
        legacy = object()

        def __init__(self, case):
            self.case, self.loop = case, loop

        def slot(self, name):
            return SimpleNamespace(phase=Phase.Rust, binding=SimpleNamespace(
                provider="claude", meta={"terminal": {"pane": "%1"}} if self.case["terminal"] else {}))

        def managed_runtime(self, name):
            return True

        async def prepare_session(self, name, provider, *, launch=False, engine_models=None):
            return True

        async def op(self, name, command, operation_id):
            if command["kind"] == "submit":
                sent.append({"submit": command["text"]})
                reply = self.case["submit_reply"]
            else:
                sent.append({"control": command["control"], "payload": command["payload"]})
                reply = self.case["reply"]
            if isinstance(reply, str):
                code, _, message = reply.removeprefix("!erro: ").partition(": ")
                if code in {"claude_command", "codex_command"}:
                    raise RustOpError(f"IPC recusou a operação (400: {code})", 400, code)
                raise RuntimeError(f"{code}: {message}")
            return {"operation_id": operation_id, "disposition": reply[0], "payload": reply[1]}

    async def stand_in_for_keyboard_loan(owner, name, operation, payload, action, **kw):
        await asyncio.to_thread(action)

    runtime_terminal.run_admin = stand_in_for_keyboard_loan
    current = {}

    def interrupt(self, name, *a, **k):
        sent.append({"control": "interrupt", "payload": {}})
        reply = current["case"]["interrupt_reply"]
        if isinstance(reply, str):
            raise RuntimeError(reply.removeprefix("!erro: "))
        if reply[0] != "accepted":
            raise api.TerminalControlError("interrupt", reply[0], None)

    ti.TerminalInput.interrupt = interrupt
    api._espera_picker_fechar = lambda name, *a, **k: True
    api.clear_pending_askq = lambda jsonl: cleared.append(jsonl)
    api._recusa_orq = lambda name: None
    api._session_exists = lambda name: True
    api._provider_of = lambda name: "claude"
    api._cached_info_sync = lambda name: SimpleNamespace(provider="claude", jsonl="/c/projects/p/sid.jsonl")
    api._loop_servidor = loop

    async def run(case):
        runtime_coordinator._current = Owner(case)
        api._headless = lambda name: not case["terminal"] and case["provider"] == "claude"
        api._cached_info_sync = lambda name: SimpleNamespace(provider=case["provider"], jsonl="/c/projects/p/sid.jsonl")
        adapter = RuntimeAdapter(case["provider"])
        adapter.view = lambda name, mutating=False: RuntimeView("k", 1, 1, {})
        api.get_adapter = lambda key: SimpleNamespace(
            answer_questions=lambda name, request_id, answers: adapter.dispatch(
                "answer_questions", name, {"request_id": request_id, "answers": answers}))
        api.plugin_bridge.pergunta_pendente = lambda name: case["pending"]
        termsock.painel_aberto = lambda name: case["panel_open"]
        questions = case["sidecar"]
        api.read_pending_askq = lambda jsonl: None if questions is None else SimpleNamespace(
            questions=[SimpleNamespace(question=q) for q in questions])
        sent.clear(), cleared.clear()
        current["case"] = case
        body = api.AnswerBody(answers=[api.AnswerItem(**a) for a in case["answers"]], request_id=case["request_id"])
        try:
            return {"status": 200, "body": await asyncio.to_thread(api.answer, "s", body)}
        except HTTPException as exc:
            return {"status": exc.status_code, "body": {"detail": exc.detail}}
        except api.TerminalControlError as exc:
            response = await api.terminal_control_failed(None, exc)
            return {"status": response.status_code, "body": json.loads(response.body)}

    def as_json(value):
        return value if isinstance(value, str) else {"disposition": value[0], "payload": value[1]}

    async def main():
        rows = []
        for case in ANSWER_CASES:
            expect = await run(case)
            rows.append({"name": case["name"], "provider": case["provider"], "terminal": case["terminal"], "answers": case["answers"],
                         "request_id": case["request_id"], "pending": case["pending"], "panel_open": case["panel_open"],
                         "sidecar": case["sidecar"], "reply": as_json(case["reply"]),
                         "submit_reply": as_json(case["submit_reply"]), "interrupt_reply": as_json(case["interrupt_reply"]),
                         "expect": expect,
                         "sent": list(sent), "cleared": bool(cleared)})
        return rows

    return asyncio.run(main())


def write_session_write(out: Path | None = None) -> None:
    out = out or HERE / "session_write"
    out.mkdir(parents=True, exist_ok=True)
    inputs, steers = session_write_rows()
    for name, rows in (("input.json", inputs), ("steer.json", steers), ("control.json", control_rows()),
                       ("answer.json", answer_rows()), ("askq_chat_text.json", chat_text_rows())):
        (out / name).write_text(json.dumps(rows, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")


CASOS_PAIR = {
    "a": dict(peers=["b"], task="T-1 filtro", gid="ab12cd34", harness={"a": "claude", "b": "codex", "z": "pi"}),
    "orq-solo": dict(peers=[], task="", gid="cd34ef56", harness={}, orq=True),
    "nome com espaço": dict(peers=["a"], task="", gid="ab12cd34", harness={}),
}

# Escritos crus: o Python de hoje não grava `fed`, e os outros dois são formatos que ele só lê.
CRUS_PAIR = {
    "legado": {"peer": "x"},
    "quebrado": [1, 2],
    "fed": {"peers": ["b", "lab::c"], "task": "t", "gid": "ab12cd34", "harness": {},
            "fed": {"owner": "casa", "local": True, "version": 7}},
}


def write_pair_sidecars() -> None:
    """Sidecars de `.hangar-pair` como o `PairLink.set` os grava, e o que o `PairLink.get` lê deles."""
    from app import pair

    out = GOLDEN / "pair_sidecars"
    out.mkdir(parents=True, exist_ok=True)
    for old in out.glob("*.json"):
        old.unlink()
    antes = pair.settings.projects_dir
    try:
        with tempfile.TemporaryDirectory() as tmp:
            # Nunca o ~/.claude/.hangar-pair de verdade.
            pair.settings.projects_dir = Path(tmp) / "projects"
            for name, caso in CASOS_PAIR.items():
                pair.PairLink(name).set(**caso)
            for name, corpo in CRUS_PAIR.items():
                (pair._pair_dir() / f"{name}.json").write_text(json.dumps(corpo), encoding="utf-8")
            for f in sorted(pair._pair_dir().glob("*.json")):
                shutil.copy(f, out / f.name)
            nomes = [*CASOS_PAIR, *CRUS_PAIR]
            write_golden("pair_sidecars/expected.json", {n: pair.PairLink(n).get() for n in nomes})
    finally:
        pair.settings.projects_dir = antes


# Rotas de grupo de uma máquina, em sequência sobre as sessões Claude s0..s3 (as mesmas que o teste
# do Rust cria). `orchestrators`: quem o `_recusa_orq` recusa naquele passo; `contract`: texto gravado
# em grupo-<gid>.md antes do passo; `python_only`: o FastAPI responde (o Rust repassa o pedido).
GROUP_GID = "9a9a9a9a"
GROUP_STEPS = [
    dict(name="pair_two_loose", method="POST", path="s0/pair", body={"peers": ["s1"], "task": "t1"}),
    dict(name="pair_task_conflict", method="POST", path="s2/pair", body={"peer": "s0", "task": "outra"}),
    dict(name="pair_third_joins", method="POST", path="s2/pair", body={"peer": "s0"}),
    dict(name="pair_self", method="POST", path="s0/pair", body={"peers": ["s0"]}),
    dict(name="pair_no_peer", method="POST", path="s0/pair", body={}),
    dict(name="pair_missing_session", method="POST", path="s0/pair", body={"peers": ["ghost", "s1"]}),
    dict(name="pair_orchestrator", method="POST", path="s0/pair", body={"peers": ["g1-orq"]}, orchestrators=["g1-orq"]),
    dict(name="pair_extra_field", method="POST", path="s0/pair", body={"peers": ["s1"], "bogus": 1}, python_only=True),
    dict(name="group_message_ok", method="POST", path="s0/group-message", body={"text": "terminei"}),
    dict(name="group_message_slash", method="POST", path="s0/group-message", body={"text": "  /clear"}),
    dict(name="group_message_forward", method="POST", path="s0/group-message", body={"text": " [grupo: s1] oi"}),
    dict(name="group_message_no_group", method="POST", path="s3/group-message", body={"text": "oi"}),
    *[dict(name=f"group_message_{i}", method="POST", path="s1/group-message", body={"text": f"marco {i}"}) for i in range(2, 6)],
    dict(name="group_message_storm", method="POST", path="s2/group-message", body={"text": "marco 6"}),
    dict(name="contract_no_group", method="GET", path="s3/pair/contract"),
    dict(name="contract_with_group", method="GET", path="s0/pair/contract", contract="decisão: usar X\n"),
    dict(name="unpair_orchestrator", method="DELETE", path="g1-orq/pair", orchestrators=["g1-orq"]),
    dict(name="unpair", method="DELETE", path="s0/pair"),
    dict(name="unpair_not_grouped", method="DELETE", path="s3/pair"),
]


def group_route_rows() -> list:
    """Cada passo pela rota do Python de verdade (TestClient, sem lifespan), com sessões, envio e
    orquestrador falsos; `delivered` = quem recebeu recado no passo."""
    from types import SimpleNamespace
    from unittest.mock import patch

    from fastapi.testclient import TestClient
    from app import api, conversation_transfer, pair
    from app.config import settings
    from app.models import SessionInfo

    rows, sent, orchestrators = [], [], set()

    async def enviar(name, text):
        sent.append(name)
        return {"ok": True}

    names = ["s0", "s1", "s2", "s3"]
    antes = (pair.settings.projects_dir, settings.auth_token)
    try:
        with tempfile.TemporaryDirectory() as tmp, \
             patch.object(api.registry, "list", lambda: [SessionInfo(name=n, cwd="/p") for n in names]), \
             patch.object(api, "_enviar", enviar), \
             patch.object(api, "_session_exists", lambda n: n in names), \
             patch.object(api.orq_runs, "find", lambda n: n in orchestrators), \
             patch.object(conversation_transfer, "require_available", lambda n: None), \
             patch.object(pair, "_arquivo_dir", lambda: Path(tmp) / "arquivo"), \
             patch.object(pair.uuid, "uuid4", lambda: SimpleNamespace(hex=GROUP_GID + "0" * 24)):
            # Nunca o ~/.claude/.hangar-pair de verdade.
            pair.settings.projects_dir = Path(tmp) / "projects"
            settings.auth_token = "secret"
            api._group_envios.clear()
            client = TestClient(api.app)
            for step in GROUP_STEPS:
                orchestrators.clear()
                orchestrators.update(step.get("orchestrators", []))
                if "contract" in step:
                    (pair._pair_dir() / f"grupo-{GROUP_GID}.md").write_text(step["contract"], encoding="utf-8")
                sent.clear()
                r = client.request(step["method"], f"/api/sessions/{step['path']}", headers={"Authorization": "Bearer secret"},
                                   **({"json": step["body"]} if "body" in step else {}))
                body = json.loads(json.dumps(r.json()).replace(str(pair._pair_dir()), "<pair>"))
                rows.append({**step, "status": r.status_code, "response": body, "delivered": sorted(sent)})
    finally:
        pair.settings.projects_dir, settings.auth_token = antes
    return rows


# Par 1:1 entre máquinas: `casa` e `lab` (e `anon`, sem CP_SERVER_ID) com as sessões s0..s3 cada.
# `peers.json` de todas conhece `lab` e `off` (fora do ar); `at` é a máquina que recebe o pedido;
# `seed` grava sidecars crus nela antes do passo; `fail_delivery`: a máquina cujo envio falha.
CROSS_REFUSAL = {"code": "erro_fila_nao_digitada", "params": {}, "msg": "composer ilegível"}
CROSS_STEPS = [
    dict(name="no_server_id", at="anon", method="POST", path="s0/pair", body={"peer": "lab::s1"}),
    dict(name="cross_with_local", at="casa", method="POST", path="s0/pair", body={"peers": ["lab::s1", "s2"]}),
    dict(name="cross_missing_local", at="casa", method="POST", path="ghost/pair", body={"peer": "lab::s1"}),
    dict(name="cross_unknown_server", at="casa", method="POST", path="s0/pair", body={"peer": "nowhere::s1"}),
    dict(name="cross_refused", at="casa", method="POST", path="s0/pair", body={"peer": "lab::ghost"}),
    dict(name="cross_network_down", at="casa", method="POST", path="s0/pair", body={"peer": "off::s1", "task": "t0"}),
    dict(name="cross_pair", at="casa", method="POST", path="s0/pair", body={"peer": "lab::s1", "task": "t1"}),
    dict(name="cross_local_mix", at="casa", method="POST", path="s2/pair", body={"peer": "s0"}),
    dict(name="group_message_remote", at="casa", method="POST", path="s0/group-message", body={"text": "oi"}),
    dict(name="pair_remote_bad_initiator", at="lab", method="POST", path="s2/pair-remote", body={"initiator": "s0"}),
    dict(name="pair_remote_mix", at="lab", method="POST", path="s1/pair-remote", body={"initiator": "casa::s2"}),
    dict(name="pair_remote_notice_fails", at="lab", method="POST", path="s2/pair-remote",
         body={"initiator": "casa::s3", "task": "x"}, fail_delivery="lab"),
    dict(name="pair_remote_extra_field", at="lab", method="POST", path="s2/pair-remote",
         body={"initiator": "casa::s3", "bogus": 1}, python_only=True),
    dict(name="pair_remote_direct", at="lab", method="POST", path="s3/pair-remote", body={"initiator": "casa::s2", "task": "y"}),
    dict(name="unpair_remote_wrong_peer", at="lab", method="POST", path="s1/unpair-remote", body={"peer": "casa::s3"}),
    dict(name="unpair_remote_not_grouped", at="lab", method="POST", path="s0/unpair-remote", body={"peer": "casa::s0"}),
    dict(name="leave_cross", at="casa", method="DELETE", path="s0/pair"),
    dict(name="unpair_remote_after_leave", at="lab", method="POST", path="s1/unpair-remote", body={"peer": "casa::s0"}),
    dict(name="leave_peer_down", at="casa", method="DELETE", path="s3/pair",
         seed={"s3": {"peers": ["off::s1"], "task": "", "gid": "abababab", "harness": {}}}),
    dict(name="leave_no_server_id", at="anon", method="DELETE", path="s3/pair",
         seed={"s3": {"peers": ["lab::s1"], "task": "", "gid": "cdcdcdcd", "harness": {}}}),
]


def cross_route_rows() -> list:
    """Cada passo pela rota do Python de verdade, as três máquinas no mesmo processo: o `call_url`
    falso troca pasta de grupo e CP_SERVER_ID para `lab` e chama a rota dela; `off` é a rede caída.
    `calls` = o que saiu para outra máquina; `delivered` = `máquina:sessão` que recebeu recado."""
    from contextlib import contextmanager
    from unittest.mock import patch

    from fastapi.testclient import TestClient
    from app import api, conversation_transfer, external_pairs, pair, peers
    from app.config import settings
    from app.models import SessionInfo

    rows, sent, calls = [], [], []
    names = ["s0", "s1", "s2", "s3"]
    ids = {"casa": "casa", "lab": "lab", "anon": ""}
    here = {"at": "casa", "fail": None}
    auth = {"Authorization": "Bearer secret"}

    async def enviar(name, text):
        sent.append(f"{here['at']}:{name}")
        return {"ok": False, "error": CROSS_REFUSAL} if here["fail"] == here["at"] else {"ok": True}

    antes = (pair.settings.projects_dir, settings.auth_token, settings.server_id)
    with tempfile.TemporaryDirectory() as tmp:
        @contextmanager
        def machine(m):
            saved = (pair.settings.projects_dir, settings.server_id, here["at"])
            # Nunca o ~/.claude/.hangar-pair de verdade.
            pair.settings.projects_dir, settings.server_id, here["at"] = Path(tmp) / m / "projects", ids[m], m
            external_pairs._reset()
            try:
                yield
            finally:
                pair.settings.projects_dir, settings.server_id, here["at"] = saved
                external_pairs._reset()

        def call_url(base, token, method, path, body=None, timeout=8, label="", follow_redirects=False):
            calls.append({"to": label, "method": method, "path": path, "body": body})
            if label == "off":
                raise peers.PeerError(f"{label} inacessível: <rede>", transport=True)
            with machine("lab"):
                r = client.request(method, path, headers=auth, **({"json": body} if body is not None else {}))
            if r.status_code >= 300:
                try:
                    detail = json.loads(r.text).get("detail", r.text)
                except (ValueError, AttributeError):
                    detail = r.text
                raise peers.PeerError(f"{label} respondeu HTTP {r.status_code}: {detail}", status=r.status_code, detail=detail)
            return r.status_code, (r.json() if r.text.strip() else None)

        try:
            with patch.object(api.registry, "list", lambda: [SessionInfo(name=n, cwd="/p") for n in names]), \
                 patch.object(api, "_enviar", enviar), \
                 patch.object(api, "_session_exists", lambda n: n in names), \
                 patch.object(api.orq_runs, "find", lambda n: False), \
                 patch.object(conversation_transfer, "require_available", lambda n: None), \
                 patch.object(pair, "_arquivo_dir", lambda: Path(tmp) / here["at"] / "arquivo"), \
                 patch.object(peers, "peer_cfg", lambda s: {"lab": ("http://lab", "secret"), "off": ("http://off", "secret")}.get(s)), \
                 patch.object(peers, "call_url", call_url):
                settings.auth_token = "secret"
                api._group_envios.clear()
                client = TestClient(api.app)
                for step in CROSS_STEPS:
                    sent.clear()
                    calls.clear()
                    here["fail"] = step.get("fail_delivery")
                    with machine(step["at"]):
                        for name, sidecar in step.get("seed", {}).items():
                            pair._pair_dir().mkdir(parents=True, exist_ok=True)
                            (pair._pair_dir() / f"{name}.json").write_text(json.dumps(sidecar), encoding="utf-8")
                        r = client.request(step["method"], f"/api/sessions/{step['path']}", headers=auth,
                                           **({"json": step["body"]} if "body" in step else {}))
                    rows.append({**step, "status": r.status_code, "response": r.json(), "delivered": sorted(sent),
                                 "calls": list(calls)})
        finally:
            pair.settings.projects_dir, settings.auth_token, settings.server_id = antes
    return rows


def main() -> None:
    claude = TRANSCRIPTS / "claude.jsonl"
    rewrite = TRANSCRIPTS / "claude_rewrite_surrogate.jsonl"
    codex = TRANSCRIPTS / "codex.jsonl"
    write_jsonl(claude, CLAUDE)
    write_jsonl(rewrite, CLAUDE_REWRITE)
    write_jsonl(codex, CODEX)
    write_queue(QUEUE / "claude-fixture.jsonl", CLAUDE_QUEUE)
    write_queue(QUEUE / "codex-fixture.jsonl", CODEX_QUEUE)

    write_golden("claude.tail.json", tail(claude, codex=False))
    write_golden("claude_rewrite_surrogate.tail.json", tail_tolerant(rewrite))
    write_golden("codex.tail.json", tail(codex, codex=True))
    write_golden("claude.history.json", history(claude, "claude", "claude-fixture"))
    write_golden("codex.history.json", history(codex, "codex", "codex-fixture"))
    write_golden("pyjson.json", [{
        "raw": raw,
        "sorted": json.dumps(json.loads(raw), sort_keys=True),
        "unsorted": json.dumps(json.loads(raw)),
        "unicode": json.dumps(json.loads(raw), ensure_ascii=False),
        "scrubbed": json.dumps(scrub_surrogates(json.loads(raw)), sort_keys=True),
    } for raw in PYJSON])
    write_golden("isotime.json", [[s, _ts({"timestamp": s})] for s in ISO])
    write_golden("ask_question.json", ask_rows())
    write_golden("preview.json", preview_rows())
    write_session_write()
    write_pair_sidecars()
    write_golden("group_routes.json", group_route_rows())
    write_golden("group_cross_routes.json", cross_route_rows())


if __name__ == "__main__":
    main()
