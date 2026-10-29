"""Captura o leitor original de chaves sem importar Computer Use nem ler configuração pessoal."""
import ast
import json
import re
import subprocess
import tempfile
from pathlib import Path
from types import SimpleNamespace
ROOT = Path(__file__).resolve().parents[5]
node = next(n for n in ast.parse((ROOT/'backend/app/computer_control.py').read_text()).body if isinstance(n,ast.FunctionDef) and n.name == '_cliproxy_keys')
cases=[]
for name, text in [
    ('missing',None), ('empty',''), ('root','api-keys:\n - root\n - second\n'),
    ('access_priority','api-keys:\n - root\naccess:\n api-keys:\n  - nested\n'),
    ('empty_access','access:\n api-keys:\n  - " "\napi-keys:\n - root\n'),
    ('numeric','api-keys:\n - 00123\n - true\n - null\n'),
    ('quotes','api-keys:\n - " alpha " # comment\n - \'beta\'\n'),
    ('inline','api-keys: [inline]\n'), ('invalid_yaml','invalid: [\napi-keys:\n - accepted\n'),
    ('invalid_port','host: false\nport: not-a-port\napi-keys:\n - accepted\n'),
    ('provider_object','api-keys:\n - name: provider\n - ignored\n'),
    ('other_section','providers:\n api-keys:\n  - ignored\n'),
    ('duplicates','api-keys:\n - first\napi-keys:\n - second\n'),
    ('same_indent','access:\n  api-keys:\n  - accepted\n'),
    ('short_indent','access:\n  api-keys:\n - ignored\n'),
    ('comments','api-keys:\n # comment\n\n - first#comment\n - second # comment\n'),
    ('escaped','api-keys:\n - "literal\\nvalue"\n'),
    ('empty_first','api-keys:\n - ""\napi-keys:\n - later\n'),
    ('trailing','api-keys: # comment\n - ignored\n'),
    ('unicode','api-keys:\n\u2003- "\u001fkey\u001f"\n'),
    ('line_separators','api-keys:\u0085 - first\u2028 - second\u2029'),
    ('invalid_utf8',b'api-keys:\n - \xff\n'),
]:
    with tempfile.TemporaryDirectory() as temporary:
        home=Path(temporary)
        if text is not None:
            path=home/'.cli-proxy-api/config.yaml';path.parent.mkdir();path.write_bytes(text if isinstance(text,bytes) else text.encode())
        ns={'Path':SimpleNamespace(home=lambda:home),'re':re,'_log':SimpleNamespace(warning=lambda *_:None)}
        exec(compile(ast.Module(body=[node],type_ignores=[]),'original-client-keys','exec'),ns)
        cases.append({'name':name,'text':text if not isinstance(text,bytes) else None,'invalid_utf8':isinstance(text,bytes),'keys':ns['_cliproxy_keys']()})
Path(__file__).with_name('client-keys.json').write_text(json.dumps({'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'python backend/tests/fixtures/contract/engines/generate_client_keys.py','cases':cases},ensure_ascii=False,indent=2)+'\n')
