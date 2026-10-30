"""Captura o wrapper original com bytes e processos sintéticos, sem configurações reais."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'
reference = tempfile.TemporaryDirectory()
wrapper = Path(reference.name) / "codex-hook-allow.py"
wrapper.write_bytes(subprocess.check_output(['git', 'show', f'{COMMIT}:scripts/codex-hook-allow.py'], cwd=ROOT))
cases = []
inputs = [b"", b"texto\n", b"\xff\xfe", b"null", b"[]", b"true", b"42", b'"texto"']
for event in ("PreToolUse", "PostToolUse", None):
    for update in ({}, None, [], "texto", False):
        inputs.append(json.dumps({"hookSpecificOutput": {"hookEventName": event, "updatedInput": update}}).encode())
for decision in ("allow", "deny", "ask", None, False):
    inputs.append(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse", "updatedInput": {"command": "x"}, "permissionDecision": decision}}, indent=2).encode())
inputs += [b'{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"\\ud800"}}}',
           b'{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"x":NaN}}}']
for raw in inputs:
    result = subprocess.run([sys.executable, str(wrapper)], input=raw, capture_output=True, timeout=10)
    cases.append(dict(mode="filter", input_hex=raw.hex(), code=result.returncode,
                      stdout_hex=result.stdout.hex(), stderr_hex=result.stderr.hex()))
with tempfile.TemporaryDirectory() as directory:
    helper = Path(directory) / "hook com espaço.py"
    helper.write_text("import json,sys\nassert sys.argv[1:]==['a;b','a|b','']\nraw=sys.stdin.buffer.read()\nsys.stderr.buffer.write(b'aviso\\xff\\n')\nprint(json.dumps({'hookSpecificOutput':{'hookEventName':'PreToolUse','updatedInput':json.loads(raw)}}))\nsys.exit(2)\n")
    raw = b'{"command":"pwd; cat x"}'
    result = subprocess.run([sys.executable,str(wrapper),"--",sys.executable,str(helper),"a;b","a|b", ""],input=raw,capture_output=True,timeout=10)
    cases.append(dict(mode="process",input_hex=raw.hex(),code=result.returncode,stdout_hex=result.stdout.hex(),stderr_hex=result.stderr.hex()))
Path(__file__).with_name("hook-allow.json").write_text(json.dumps(dict(
    source_commit=COMMIT,
    command="uv run python tests/fixtures/contract/codex-integration/generate_hook_allow.py",
    cases=cases),ensure_ascii=True,indent=2)+"\n")
reference.cleanup()
print(len(cases))
