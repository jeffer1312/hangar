"""Captura reconciliação de artefatos com disco real e corridas controladas."""
import ast, hashlib, json, os, subprocess, tempfile
from copy import deepcopy
from pathlib import Path
from types import SimpleNamespace
ROOT = Path(__file__).resolve().parents[5]
COMMIT = '8a3ce81c1'
ns = dict(Path=Path, hashlib=hashlib, json=json, os=os, tempfile=tempfile,
          deepcopy=deepcopy, atomico=SimpleNamespace(substituir=os.replace))
def extract(path, names):
    source = subprocess.check_output(['git', 'show', f'{COMMIT}:{path}'], cwd=ROOT, text=True)
    nodes = [node for node in ast.parse(source).body
             if isinstance(node, (ast.FunctionDef, ast.ClassDef)) and node.name in names]
    exec(compile(ast.Module(body=nodes, type_ignores=[]), path, 'exec'), ns)
source = subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_msgs.py'],cwd=ROOT,text=True)
exec(compile(source,'backend/app/codex_msgs.py','exec'),ns)
extract('backend/app/codex_arquivos.py', {'AlteradoExternamente','ler','json_bytes','hash_bytes','gravar','backup'})
extract('backend/app/codex_fragmentos.py', {'reconciliar_arquivos'})
read, write = ns['ler'], ns['gravar']
cases = []
for name in ['new','same-unmanaged','different-unmanaged','managed-local-change','trusted-local',
             'stale-same','stale-changed','stale-missing','malformed-record','retry-ok',
             'retry-exhausted','stale-race','managed-symlink','stale-symlink','hash-false']:
    with tempfile.TemporaryDirectory() as raw:
        root=Path(raw); path=root/'account'/'artifact.md'; path.parent.mkdir(); backups=root/'backups'
        stale=name.startswith('stale-'); original=None if name in ('new','stale-missing') else b'personal'
        if name in ('same-unmanaged','stale-same','stale-symlink','stale-race'): original=b'source'
        external=root/'external.md'
        if 'symlink' in name:
            external.write_bytes(original); path.symlink_to(external)
        elif original is not None: path.write_bytes(original)
        desired={} if stale else {path:b'source'}
        previous={}
        if name not in ('new','same-unmanaged','different-unmanaged','trusted-local'):
            previous[str(path)]={'hash':hashlib.sha256(b'source').hexdigest(),'extra':{'keep':True}}
        if name=='malformed-record': previous[str(path)]=[]
        if name=='hash-false': previous[str(path)]={'hash':False,'extra':'keep'}
        trusted={path} if name=='trusted-local' else set()
        writes=0; reads=0
        def controlled_write(target, data, expected, backup_root):
            global writes
            writes+=1
            if name in ('retry-ok','retry-exhausted') and writes <= (2 if name=='retry-ok' else 3):
                target.write_bytes(f'race-{writes}'.encode())
                raise ns['AlteradoExternamente']('fixture conflict')
            return write(target,data,expected,backup_root)
        def controlled_read(target):
            global reads
            reads+=1
            if name=='stale-race' and reads==2: target.write_bytes(b'race')
            return read(target)
        ns.update(gravar=controlled_write,ler=controlled_read)
        manifest,warnings=ns['reconciliar_arquivos'](desired,previous,backups,confiaveis=trusted)
        saved=[json.loads(p.read_bytes()) for p in sorted(backups.glob('*.json'))]
        result=dict(manifest=manifest,warnings=[ns['serializar'](warning) for warning in warnings],
                    final_hex=path.read_bytes().hex() if path.exists() else None,
                    link=os.readlink(path) if path.is_symlink() else None,
                    external_hex=external.read_bytes().hex() if external.exists() else None,
                    backups=saved,writes=writes)
        case=dict(name=name,original_hex=original.hex() if original is not None else None,
                  desired=not stale,previous=previous,trusted=bool(trusted),result=result)
        cases.append(json.loads(json.dumps(case).replace(raw,'$ROOT')))
        ns.update(gravar=write,ler=read)
Path(__file__).with_name('artifacts.json').write_text(json.dumps(dict(source_commit=COMMIT,
    command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_artifacts.py',
    cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
