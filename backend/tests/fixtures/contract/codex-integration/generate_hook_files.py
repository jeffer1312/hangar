"""Captura materialização e atualização dos arquivos de hooks referidos pelo Codex."""
import io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
cases=[]
scenarios=[dict(name='missing-source'),dict(name='file',source='736f75726365'),dict(name='nested',source='736f75726365',namepath='nested/x.sh'),dict(name='source-link',source='736f75726365',source_link=True),dict(name='target-broken',source='736f75726365',target_broken=True),dict(name='current',source='736f75726365',target='736f75726365'),dict(name='changed',source='6e6577',target='6f6c64'),dict(name='unrelated',source='736f75726365',outside=True),dict(name='relative',source='736f75726365',relative=True),dict(name='source-directory',source_directory=True),dict(name='invalid-document',invalid=True),dict(name='document-file',source='736f75726365',file='valid'),dict(name='document-utf8',source='736f75726365',file='utf8'),dict(name='document-json',source='736f75726365',file='json'),dict(name='document-bom',source='736f75726365',file='bom')]
with tempfile.TemporaryDirectory() as raw:
 archive=Path(raw)/'archive';archive.mkdir()
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','archive',COMMIT,'backend/app'],cwd=ROOT))) as tar:tar.extractall(archive,filter='data')
 os.environ['HOME']=str(Path(raw)/'import-home');Path(os.environ['HOME']).mkdir();sys.path.insert(0,str(archive/'backend'))
 from app import codex_hooks_arquivos as original
 for windows in [False,True]:
  original._WINDOWS=windows
  for scenario in scenarios:
   with tempfile.TemporaryDirectory() as folder:
    root=Path(folder);home=root/'home';account=root/'account';name=scenario.get('namepath','x.sh');source=home/'.claude/hooks'/name;target=account/'hooks'/name;source.parent.mkdir(parents=True);target.parent.mkdir(parents=True)
    if 'source' in scenario:
     if scenario.get('source_link'):
      actual=root/'real.sh';actual.write_bytes(bytes.fromhex(scenario['source']));source.symlink_to(actual)
     else:source.write_bytes(bytes.fromhex(scenario['source']))
    if scenario.get('source_directory'):source.mkdir()
    if 'target' in scenario:target.write_bytes(bytes.fromhex(scenario['target']))
    if scenario.get('target_broken'):target.symlink_to(root/'missing')
    command='x.sh' if scenario.get('relative') else str(root/'outside.sh') if scenario.get('outside') else str(target)
    doc=[] if scenario.get('invalid') else {'hooks':{'PreToolUse':[{'hooks':[{'command':f'python "{command}"'},{'command':f"python '{command}'"}]}]}}
    if scenario.get('file'):
     raw=json.dumps(doc).encode()
     raw={'utf8':b'\xff','json':b'{','bom':b'\xef\xbb\xbf'+raw}.get(scenario['file'],raw)
     (account/'hooks.json').write_bytes(raw);doc=None
    created,orphans=original.materializar(account,home,doc)
    files={}
    for path in sorted((account/'hooks').rglob('*')):
     key=str(path.relative_to(account))
     if path.is_symlink():files[key]={'link':str(path.readlink()).replace(str(root),'ROOT')}
     elif path.is_file():files[key]={'hex':path.read_bytes().hex()}
    cases.append(dict(input=scenario,windows=windows,result=dict(created=created,orphans=orphans,files=files)))
Path(__file__).with_name('hook-files.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_hook_files.py',platform='POSIX real e seam _WINDOWS original; sem VM',cases=cases),indent=2)+'\n')
print(len(cases))
