"""Captura mecanismos no baseline congelado, sem iniciar o servidor.

Uso: python gen_mechanisms.py <checkout-baseline> <fixture-destino>.
"""
import json,os,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
baseline=Path(sys.argv[1])
sys.path.insert(0,str(baseline/'backend'))
from app import omp_plugin_sync as m
out={'origin':subprocess.check_output(['git','rev-parse','HEAD'],cwd=baseline,text=True).strip(),'platform':sys.platform,'command':'python gen_mechanisms.py <checkout-baseline> <fixture-destino>','operations':['PluginSynchronizer.__init__','PluginSynchronizer.reconcile','PluginSynchronizer.import_marketplaces','_write']}
def norm(value,home):
 if isinstance(value,str):return value.replace(str(home),'<HOME>')
 if isinstance(value,list):return [norm(v,home) for v in value]
 if isinstance(value,dict):return {k:norm(v,home) for k,v in value.items()}
 return value
with tempfile.TemporaryDirectory() as directory:
 home=Path(directory)/'home';home.mkdir();claude=home/'.claude'
 env={'PATH':os.environ.get('PATH',''),'HOME':'ignored-home','USERPROFILE':'ignored-profile','CLAUDE_CONFIG_DIR':'ignored-claude','GIT_DIR':'host-repository','GIT_WORK_TREE':'host-worktree','GIT_CONFIG_GLOBAL':'host-global','GIT_CONFIG_SYSTEM':'host-system','GIT_CONFIG_COUNT':'1','GIT_CONFIG_KEY_0':'alias.host','GIT_CONFIG_VALUE_0':'host-value','GIT_TERMINAL_PROMPT':'1','GIT_OPTIONAL_LOCKS':'1','GIT_SYNTHETIC':'host-extra','UNCHANGED':'kept'}
 with patch.dict(os.environ,env,clear=True):
  sync=m.PluginSynchronizer(home=home,claude_dir=claude,runner=lambda *a,**kw:(_ for _ in()).throw(AssertionError('CLI inesperado')))
 out['environment_input']={k:v for k,v in env.items() if k!='PATH'}
 out['environment_expected']=norm({k:v for k,v in sync.env.items() if k!='PATH'},home)
 out['lock_cases']=[]
 for target in ['ledger','lock']:
  for operation in ['reconcile','import_marketplaces']:
   sync=m.PluginSynchronizer(home=home,claude_dir=claude,runner=lambda *a,**kw:(_ for _ in()).throw(AssertionError('CLI inesperado')))
   destination=home/'sentinel';destination.write_text('preservar\n'); path=sync.ledger_path if target=='ledger' else sync.lock_path
   path.parent.mkdir(parents=True,exist_ok=True);path.symlink_to(destination)
   report=getattr(sync,operation)();out['lock_cases'].append({'target':target,'operation':operation,'report':report,'target_text':destination.read_text(),'link_preserved':path.is_symlink()})
   path.unlink()
 with patch.object(m.peers,'fcntl',None),patch.object(m.peers,'msvcrt',None):
  out['backend_unavailable']={op:getattr(sync,op)() for op in ['reconcile','import_marketplaces']}
 target=home/'atomic.json';target.write_text('anterior\n');events=[];real_fsync=m.os.fsync;real_replace=m.atomico.substituir
 def fsync(fd):events.append('fsync');return real_fsync(fd)
 def replace(source,dest):events.append('replace');assert Path(source).parent==Path(dest).parent;assert Path(source).read_text().endswith('\n');return real_replace(source,dest)
 with patch.object(m.os,'fsync',fsync),patch.object(m.atomico,'substituir',replace):m._write(target,{'ação':'migração','itens':[1,2]})
 out['atomic_success']={'bytes':target.read_text(),'order':events}
 before=target.read_bytes()
 with patch.object(m.atomico,'substituir',side_effect=OSError('synthetic replace failure')):
  try:m._write(target,{'ação':'nova'})
  except OSError:pass
  else:raise AssertionError('falha não propagada')
 out['atomic_failure']={'unchanged':target.read_bytes()==before,'temporary_files':sorted(p.name for p in home.glob('atomic.json.*.tmp'))}

with tempfile.TemporaryDirectory() as directory:
 root=Path(directory);git=root/'.git';git.mkdir();(git/'config').write_text('[remote "origin"]\n url = https://git.example.invalid/team/comet.git\n')
 revision='a'*40;reference='refs/heads/work'
 (git/'HEAD').write_text('ref: '+reference+'\n');(git/'packed-refs').write_text('# pack-refs with: peeled fully-peeled sorted\n'+'b'*40+' refs/heads/other\n'+revision+' '+reference+'\n')
 out['packed_refs']={'files':{str(p.relative_to(root)):p.read_text() for p in git.iterdir()},'expected':m._git_identity(root)}
 ref=git/reference;ref.parent.mkdir(parents=True);outside=root/'outside';outside.write_text(revision+'\n');ref.symlink_to(outside)
 try:m._git_identity(root)
 except m.InventoryError as error:out['packed_refs']['outside_ref_error']=str(error)
 ref.unlink();git.rename(root/'real-git');git.symlink_to(root/'real-git',target_is_directory=True)
 out['packed_refs']['symbolic_git_expected']=m._git_identity(root)
 name='@fixture/comet';url='https://git.example.invalid/team/comet.git#'+revision
 jsonc='{\n // comment before packages\n "notice":"https://example.invalid/*keep*/,} // keep \\\"quoted\\\"",\n "packages": {"'+name+'": ["'+name+'@'+url+'", {"preserve":"a,}/* b */ // c"},],},\n}\n'
 (root/'bun.lock').write_text(jsonc)
 out['bun_jsonc']={'text':jsonc,'name':name,'expected':m._bun_revision(root,name)}


if sys.platform=='linux':
 import time
 with tempfile.TemporaryDirectory() as directory:
  root=Path(directory);real_popen=m.subprocess.Popen;seen=[]
  class InterruptedProcess(real_popen):
   calls=0
   def communicate(self,*args,**kwargs):
    self.calls+=1
    if self.calls==1:
     deadline=time.monotonic()+5
     while not (root/'descendant').exists():
      if time.monotonic()>deadline:raise AssertionError('filho não iniciou')
      time.sleep(.005)
     seen.append(self);raise KeyboardInterrupt('synthetic cancellation')
    return super().communicate(*args,**kwargs)
  with patch.object(m.subprocess,'Popen',InterruptedProcess):
   try:m._run(['/bin/sh','-c','sleep 30 & echo $! > descendant; wait'],cwd=root,env={'PATH':'/usr/bin:/bin'},timeout=30)
   except KeyboardInterrupt:pass
   else:raise AssertionError('interrupção não propagada')
  pid=int((root/'descendant').read_text());proc=Path('/proc')/str(pid)/'stat'
  try:alive=not proc.read_text().rsplit(')',1)[1].lstrip().startswith('Z')
  except (FileNotFoundError,ProcessLookupError):alive=False
  if alive:os.kill(pid,9)
  out['runner_cancel']={'exception':'KeyboardInterrupt','descendant_alive':alive,'reaped':seen[0].returncode is not None,'communicate_calls':seen[0].calls}


urls=['https://GIT.EXAMPLE.INVALID:443/team/comet.git','http://git.example.invalid/team/comet.git','ssh://git@git.example.invalid:22/team/comet.git','git@git.example.invalid:team/comet.git','git+https://git.example.invalid/team/comet.git','https://user:pass@git.example.invalid/team/comet.git','https://git.example.invalid/team/comet.git?q=1','https://git.example.invalid/team/comet.git#part','https://git.example.invalid/./comet.git','https://git.example.invalid/../comet.git','https://git.example.invalid/team /comet.git','https://git.example.invalid/team'+chr(28)+'/comet.git']
urls.extend('https://git.example.invalid/team'+chr(code)+'/comet.git' for code in range(29,32))
out_cases=[]
for url in urls:
 try:out_cases.append({'input':url,'value':m._identity(url)})
 except (m.InventoryError,ValueError) as error:out_cases.append({'input':url,'error':str(error)})
out['origins']=out_cases


import base64
out['records']=[]
for kind in ['invalid_utf8','symbolic_record']:
 with tempfile.TemporaryDirectory() as directory,patch.dict(os.environ,{},clear=True):
  home=Path(directory);claude=home/'.claude';record=claude/'plugins/installed_plugins.json';record.parent.mkdir(parents=True)
  if kind=='invalid_utf8':record.write_bytes(bytes([255]))
  else:
   target=home/'record-target';target.write_text('sentinel\n');record.symlink_to(target)
  sync=m.PluginSynchronizer(home=home,claude_dir=claude,runner=lambda *a,**kw:(_ for _ in()).throw(AssertionError('CLI inesperado')))
  result=sync.reconcile()
  out['records'].append({'kind':kind,'report':result,'target_text':(home/'record-target').read_text() if kind=='symbolic_record' else None})

Path(sys.argv[2]).write_text(json.dumps(out,ensure_ascii=False,indent=2)+'\n')
