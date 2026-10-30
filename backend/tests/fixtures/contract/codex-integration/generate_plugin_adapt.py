"""Captura a adaptação original de plugins com arquivos e links temporários."""
import ast,json,os,subprocess,tempfile,hashlib,shlex,re,copy
from pathlib import Path,PureWindowsPath
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5];COMMIT='8a3ce81c1'
def extract(path,names,ns):
 source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
 nodes=[node for node in ast.parse(source).body if isinstance(node,(ast.FunctionDef,ast.ClassDef)) and node.name in names]
 exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),ns)
ns=dict(Path=Path,PureWindowsPath=PureWindowsPath,json=json,os=os,tempfile=tempfile,hashlib=hashlib,shlex=shlex,re=re,deepcopy=copy.deepcopy,Callable=object,atomico=SimpleNamespace(substituir=os.replace))
extract('backend/app/codex_arquivos.py',{'ler','json_obj','json_bytes','hash_bytes','backup','gravar','transformar','AlteradoExternamente'},ns)
extract('backend/app/codex_compat.py',{'_split_windows','_nome_binario','_comando','normalizar_security_guidance','adaptar_hooks_plugin'},ns)
cases=[]
for name in ['empty','root-hooks','referenced','embedded','security','outside-root','outside-hook','bad-refs','bad-list','ecc-ok','ecc-idle','ecc-failure','dangling-root']:
 with tempfile.TemporaryDirectory() as raw:
  home=Path(raw);account=home/'account';plugin=account/'plugins/cache/fixture/v1';plugin.mkdir(parents=True);backups=home/'backups'
  repo=home/'repo';(repo/'scripts').mkdir(parents=True);(repo/'scripts/codex-hook-json.py').write_bytes(b'fixture wrapper')
  external=home/'outside.json';external.write_bytes(b'original outside')
  hooks={'hooks':{'PreToolUse':[{'hooks':[{'type':'command','command':'echo fixture'}]}]}}
  if name not in ('empty','outside-root','dangling-root'): (plugin/'hooks.json').write_text(json.dumps(hooks))
  manifest={}
  if name=='referenced': manifest={'hooks':'custom.json'};(plugin/'custom.json').write_text(json.dumps(hooks))
  if name=='embedded':manifest={'hooks':hooks['hooks']}
  if name=='security':manifest={'name':'security-guidance'}
  if name.startswith('ecc'):manifest={'name':'ecc'}
  if name=='bad-refs':manifest={'hooks':123}
  if name=='bad-list':manifest={'hooks':['hooks.json',2]}
  if manifest:(plugin/'.codex-plugin').mkdir();(plugin/'.codex-plugin/plugin.json').write_text(json.dumps(manifest))
  if name=='outside-hook':(plugin/'hooks.json').unlink();(plugin/'hooks.json').symlink_to(external)
  if name=='dangling-root':plugin.rmdir();plugin.symlink_to(home/'missing-outside')
  source={str(path.relative_to(plugin)):path.read_bytes().replace(raw.encode(),b'$ROOT').hex() for path in plugin.rglob('*') if path.is_file() and not path.is_symlink()}
  count=[0];calls=[]
  def changed():count[0]+=1
  def normalize(data):return {**data,'fixture':True}
  def ecc(root,backups):
   calls.append('ecc')
   if name=='ecc-failure':raise RuntimeError('fixture ECC')
   return name=='ecc-ok'
  ns.update(_REPO=repo,_preparar_runtime_ecc=ecc,install_root=SimpleNamespace(interpreter=lambda:'/fixture/python'))
  try:
   ns['adaptar_hooks_plugin'](home if name=='outside-root' else plugin,account,backups,normalizar=normalize,ao_alterar=changed)
   result={'ok':True}
  except Exception as error:result={'error':type(error).__name__,'message':str(error)}
  files={str(path.relative_to(plugin)):path.read_bytes().replace(raw.encode(),b'$ROOT').hex() for path in plugin.rglob('*') if path.is_file() and not path.is_symlink()}
  saved=[]
  if backups.exists():
   for path in backups.glob('*.json'):saved.append(json.loads(path.read_text()))
  wrapper=account/'.hangar-hooks/codex-hook-json.py'
  case=dict(name=name,source=source,result=result,changed=count[0],calls=calls,files=files,backups=sorted(saved,key=lambda value:value['path']),wrapper_hex=wrapper.read_bytes().hex() if wrapper.exists() else None,outside_hex=external.read_bytes().hex())
  cases.append(json.loads(json.dumps(case).replace(raw,'$ROOT')))
Path(__file__).with_name('plugin-adapt.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_plugin_adapt.py',cases=cases),indent=2)+'\n')
print(len(cases))
