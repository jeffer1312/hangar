"""Captura o adaptador JSON original sem SDK, rede ou configuração pessoal."""
import ast
import json
from pathlib import Path
import subprocess
import sys
import shlex
import tempfile

ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'
source = subprocess.check_output(['git', 'show', f'{COMMIT}:scripts/codex-hook-json.py'], cwd=ROOT, text=True)
tree = ast.parse(source)
namespace = {'json': json}
exec(compile(ast.Module(body=[node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'normalizar'], type_ignores=[]), 'codex_hook_json_original.py', 'exec'), namespace)
inputs = [b'', b'texto\n', b'\xff', b'null', b'[]', b'true', b'42', b'{}\n',
          b'{"continue":false,"stopReason":"pare"}', b'{"decision":"block"}\n{"decision":"block"}',
          b'{"async":true,"asyncTimeout":180000}\n{"metrics":{"sdk_bootstrap":0}}\n',
          b'{"async":true,"extra":1}\n{"metrics":{}}', b'{"async":1}\n{"metrics":{}}',
          b'\n {"metrics":{}} \r\n', b'{"metrics":{}}\v', b'{"metrics":{}}\x85',
          b'{"metrics":NaN,"number":Infinity}', b'{"metrics":{},"rewakeSummary":"\\ud800"}',
          b'{"metrics":{},"x":1,"x":2}', b'{"rewakeSummary":null}', b'{"metrics":null}']
for summary in ['', 'Aviso', None, False, [], {}]:
    for message in ['', 'Confira', None, False, [], {}]:
        inputs.append(json.dumps({'metrics': {}, 'rewakeSummary': summary, 'systemMessage': message}).encode())
inputs += [json.dumps({'metrics': {}, 'rewakeSummary': 'Instruções'}).encode(encoding)
           for encoding in ['utf-16', 'utf-32', 'utf-8-sig']]
cases = [dict(input_hex=raw.hex(), stdout_hex=namespace['normalizar'](raw).hex()) for raw in inputs]
process_cases = []
with tempfile.TemporaryDirectory() as directory:
    wrapper = Path(directory) / 'codex-hook-json.py'
    wrapper.write_text(source)
    helper = Path(directory) / 'hook com espaço.py'
    helper.write_text("import os,signal,sys\nassert sys.stdin.buffer.read()==b'entrada\\x00\\xff'\n"
                      "assert sys.argv[3:]==['a;b','a|b','']\n"
                      "sys.stdout.buffer.write(bytes.fromhex(sys.argv[1]));sys.stdout.buffer.flush()\n"
                      "sys.stderr.buffer.write(b'aviso\\xff\\n');sys.stderr.buffer.flush()\n"
                      "if sys.argv[2]=='signal': os.kill(os.getpid(),signal.SIGTERM)\n"
                      "sys.exit(int(sys.argv[2]))\n")
    outcomes = [('0', b'{"metrics":{},"rewakeSummary":"Aviso"}'),
                ('2', b'{"metrics":{},"decision":"block"}'), ('7', b'texto\xff\n')]
    if sys.platform != 'win32': outcomes.append(('signal', b'{"metrics":{}}'))
    for outcome, output in outcomes:
        arguments = [sys.executable, str(helper), output.hex(), outcome, 'a;b', 'a|b', '']
        command = subprocess.list2cmdline(arguments) if sys.platform == 'win32' else shlex.join(arguments)
        result = subprocess.run([sys.executable, str(wrapper), '--', command], input=b'entrada\x00\xff', capture_output=True, timeout=10)
        process_cases.append(dict(outcome=outcome, output_hex=output.hex(), input_hex=b'entrada\x00\xff'.hex(),
                                  code=result.returncode, stdout_hex=result.stdout.hex(), stderr_hex=result.stderr.hex()))
Path(__file__).with_name('hook-json.json').write_text(json.dumps({
    'source_commit': COMMIT,
    'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_hook_json.py',
    'cases': cases, 'process_cases': process_cases}, ensure_ascii=True, indent=2) + '\n')
print(len(cases))
