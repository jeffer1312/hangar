# backend/tests/fixtures/contract/gen_golden.py
"""Transcripts sintéticos do contrato da conversa e o resultado que os parsers Python dão para eles.

Os testes do hangar-server comparam campo a campo com o que este script grava em golden/. Tudo aqui
é inventado: nunca copie conversa real para cá.

Uso, de backend/:  uv run python tests/fixtures/contract/gen_golden.py [--only pasta,...]
"""
import json
import os
import sys
import time
from datetime import datetime
from pathlib import Path

# Relógio sem fuso: o RewriteFilter e o merged_history do Python leem como hora LOCAL, o Rust como
# UTC. Com o processo em UTC os dois concordam no golden.
os.environ["TZ"] = "UTC"
time.tzset()

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))

from app import pqueue, registry  # noqa: E402
from app.adapters.codex.rollout import parse_rollout_line  # noqa: E402
from app.models import scrub_surrogates  # noqa: E402
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
    # Entradas da fila que chegaram durante o turno: a CLI as junta num `user` só, um bloco de texto por
    # entrada. Todas aparecem, e cada entrada (`q-juntada-*`) é absorvida pelo próprio bloco, inclusive a
    # de várias linhas. O bloco de marcação do harness no meio não entra.
    j({"type": "user", "uuid": "u-024", "timestamp": T(32), "message": {"role": "user", "content": [
        {"type": "text", "text": "fila A editada\n"},
        {"type": "text", "text": "<task-notification><task-id>t-9</task-id></task-notification>"},
        {"type": "text", "text": "linha um\nlinha dois"},
        {"type": "text", "text": "fila C terceira"}]}}),
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
    {"id": "q-juntada-a", "text": "fila A editada", "ts": epoch(T(31)), "delivered": True},
    {"id": "q-juntada-c", "text": "fila C terceira", "ts": epoch(T(31, 500)), "delivered": True},
    {"id": "q-juntada-varias", "text": "linha um\nlinha dois", "ts": epoch(T(31, 700)), "delivered": True},
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


# ── /input: o corpo HTTP que o Python devolve para cada resposta do ator ─────────────────
# Chama o `input_prompt` de verdade, com um coordenador falso que responde a
# disposição do caso. O Rust lê estes arquivos e tem de responder o mesmo corpo e o mesmo código.
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


def session_write_rows() -> list:
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
        inputs = []
        for name, terminal, text, steer, reply, queue in INPUT_CASES:
            expect = await run(lambda: api.input_prompt("s", api.InputBody(text=text, steer=steer)),
                               terminal, Owner(terminal, reply, queue))
            inputs.append({"name": name, "terminal": terminal, "text": text, "steer": steer, "reply": as_json(reply),
                           "queue": as_json(queue), "expect": expect, "diary": list(diary)})
        return inputs

    return asyncio.run(main())


def write_session_write(out: Path | None = None) -> None:
    from app import api, runtime_coordinator, runtime_terminal, terminal_input, termsock

    out = out or HERE / "session_write"
    out.mkdir(parents=True, exist_ok=True)
    # Os casos trocam o coordenador e funções do `api` por dublês; sem devolver os originais, as rotas de
    # grupo que vêm depois liam os dublês (`rust_owns_groups` quebrava, a recusa do orquestrador sumia).
    patched = (api, api.diag, api.plugin_bridge, runtime_coordinator, runtime_terminal, termsock,
               terminal_input.TerminalInput)
    saved = [(owner, dict(vars(owner))) for owner in patched]
    try:
        rows_by_name = (("input.json", session_write_rows()),)
    finally:
        for owner, before in saved:
            for name in set(vars(owner)) - set(before):
                delattr(owner, name)
            for name, value in before.items():
                # Apagado por um caso também volta: ausente não é "igual ao original".
                if name not in vars(owner) or vars(owner)[name] is not value:
                    setattr(owner, name, value)
    for name, rows in rows_by_name:
        (out / name).write_text(json.dumps(rows, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")


def write_transcript_golden() -> None:
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
    write_session_write()


AREAS = {"golden": write_transcript_golden}


def main() -> None:
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--only", help=f"pastas separadas por vírgula, entre {', '.join(AREAS)}")
    only = parser.parse_args().only
    names = only.split(",") if only else list(AREAS)
    unknown = [n for n in names if n not in AREAS]
    if unknown:
        parser.error(f"pasta desconhecida: {', '.join(unknown)}")
    for n in names:
        AREAS[n]()


if __name__ == "__main__":
    main()
