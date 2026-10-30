"""Captura cancelamento real na abertura e na escrita administrativa original."""
import ast,asyncio,json,os,sys,tempfile
from pathlib import Path
from types import SimpleNamespace
source=Path(__file__).with_name('generate_native_cli.py')
tree=ast.parse(source.read_text());tree.body=[node for node in tree.body if not(isinstance(node,ast.Expr) and isinstance(node.value,ast.Call) and isinstance(node.value.func,ast.Attribute) and node.value.func.attr=='run')]
loaded={'__file__':str(source)};exec(compile(tree,str(source),'exec'),loaded)
namespace=loaded['namespace'];Native=namespace['CodexNativo']
files={};tree=ast.parse(loaded['original']('backend/app/codex_arquivos.py'));tree.body=[node for node in tree.body if not(isinstance(node,ast.ImportFrom) and node.module=='app')];exec(compile(tree,'codex_arquivos_original.py','exec'),files);files['atomico']=SimpleNamespace(substituir=os.replace)
PROGRAM='''
import json,os,sys,time
from pathlib import Path
for line in sys.stdin:
 message=json.loads(line)
 method=message.get('method')
 if method==sys.argv[2]:
  Path(sys.argv[1]).write_text(json.dumps({'pid':os.getpid(),'cwd':os.getcwd()}))
  time.sleep(30)
 elif 'id' in message:
  print(json.dumps({'id':message['id'],'result':{}}),flush=True)
'''
async def capture(method,writer):
 with tempfile.TemporaryDirectory() as raw:
  root=Path(raw);home=root/'home';codex=home/'.codex';codex.mkdir(parents=True);config=codex/'config.toml';config.write_text('model="fixture"\n');script=root/'program.py';script.write_text(PROGRAM);seen=root/'seen.json';work=root/'work';confirmed=[]
  def native(h,c,b='codex'):
   client=Native(h,c,b,timeout=3,close_timeout=.02);client._comando=lambda:[sys.executable,str(script),str(seen),method];return client
  async def operation():
   if writer:await files['editar_config'](config,root/'backups',work,native,lambda current:([{'keyPath':'model','value':'changed','mergeStrategy':'replace'}],lambda:confirmed.append(True)))
   else:
    async with native(home,codex):pass
  task=asyncio.create_task(operation())
  async def ready():
   while not seen.exists():await asyncio.sleep(.002)
  await asyncio.wait_for(ready(),3);observed=json.loads(seen.read_text());task.cancel()
  try:await task;result='ok'
  except asyncio.CancelledError:result='CancelledError'
  try:os.kill(observed['pid'],0);alive=True
  except ProcessLookupError:alive=False
  return {'method':method,'writer':writer,'result':result,'alive':alive,'cwd_removed':not Path(observed['cwd']).exists(),'work_empty':not work.exists() or not list(work.iterdir()),'config':config.read_text(),'confirmed':confirmed}
async def main():
 with tempfile.TemporaryDirectory() as logs:
  namespace['log_paths']=SimpleNamespace(base=lambda:Path(logs))
  cases=[await capture('initialize',False),await capture('initialize',True),await capture('config/batchWrite',True)]
 Path(__file__).with_name('native-cancellation.json').write_text(json.dumps({'source_commit':loaded['COMMIT'],'command':'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_native_cancellation.py','program':PROGRAM,'cases':cases},ensure_ascii=True,indent=2)+'\n');print(len(cases))
asyncio.run(main())
