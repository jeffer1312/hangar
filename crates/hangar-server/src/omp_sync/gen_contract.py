"""Gera a referência pelo Python original isolado; execute antes de removê-lo.

Comando: cd backend && uv run python ../crates/hangar-server/src/omp_sync/gen_contract.py
"""
import base64,json,os,sys,tempfile,subprocess
from pathlib import Path
sys.path.insert(0,str(Path.cwd()))
import pytest
from tests.test_omp_plugin_sync import scenario,read_json,write_json,add_candidate
from app import omp_plugin_sync as module
OUT=Path(__file__).with_name('contract-windows.json' if os.name=='nt' else 'contract.json')
ORIGIN=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
real_run=module._run

def normalize(value,home,template=None):
 if isinstance(value,str):
  for path,marker in [(str(home),'<HOME>')]+([(str(template),'<TEMPLATE>')] if template else []):
   escaped=json.dumps(path,ensure_ascii=False)[1:-1]
   if escaped!=path:value=value.replace(escaped,marker[:-1]+'_JSON>')
   value=value.replace(path,marker)
  return value
 if isinstance(value,list):return [normalize(v,home,template) for v in value]
 if isinstance(value,dict):return {k:normalize(v,home,template) for k,v in value.items()}
 return value

def snapshot(home):
 out={}
 for p in sorted(home.rglob('*')):
  rel=str(p.relative_to(home)).replace(os.sep,'/')
  if '/objects/' in rel or '/logs/' in rel or '/hooks/' in rel:continue
  if p.is_symlink():out[rel]={'link':str(p.readlink()).replace(str(home),'<HOME>')}
  elif p.is_dir():out[rel]={'dir':True}
  else:
   data=p.read_bytes()
   try:out[rel]={'text':normalize(data.decode(),home)}
   except UnicodeDecodeError:out[rel]={'base64':base64.b64encode(data).decode()}
 return out

def case(kind):
 with tempfile.TemporaryDirectory() as d, pytest.MonkeyPatch.context() as mp:
  s=scenario.__wrapped__(Path(d),mp)
  if kind in ['unchanged','update','disable','remove','manual','corrupt-ledger','ambiguous','update-fails']:
   s.reconcile()
  if kind in ['adopt','inactive-manual','equivalent'] or kind.startswith(('features-','collision-','native-disabled','native-no-proof')):
   s.native.seed(s.repository,s.revision)
  if kind in ['disable','inactive','inactive-manual']:s.source(enabled=False)
  if kind in ['remove']:s.remove_source()
  if kind in ['update','update-fails']:s.advance()
  if kind=='update-fails':s.native.failure=('install','no-effect')
  if kind=='manual':
   lock=s.native.lock;lock['settings'][s.name]={'personal':True};write_json(s.native.root/'omp-plugins.lock.json',lock)
  if kind=='corrupt-ledger':
   ledger=read_json(s.home/'.hangar/omp-plugin-sync.json');ledger['items'][s.name]['native']['proof']=None;write_json(s.home/'.hangar/omp-plugin-sync.json',ledger)
  if kind=='ambiguous':add_candidate(s,'second',s.name);add_candidate(s,'third',s.name);add_candidate(s,'independent','independent')
  if kind=='scope':s.source(scope='project')
  if kind=='missing-sha':
   p=s.claude_dir/'plugins/installed_plugins.json';r=read_json(p);r['plugins'][s.plugin_id][0].pop('gitCommitSha');write_json(p,r)
  if kind=='invalid-registry':(s.claude_dir/'plugins/installed_plugins.json').write_text('{')
  if kind=='manifest':
   p=s.install_root/'package.json';r=read_json(p);r.pop('omp');write_json(p,r)
  if kind=='equivalent':
   origin='https://GIT.EXAMPLE.INVALID:443/team/comet.git';s.native.repositories[origin]=s.repository_path;s.native.seed(origin,s.revision)
  if kind=='subdir':s.source(subdir='packages/comet')
  if kind=='dry-run-absent':
   import shutil
   shutil.rmtree(s.home/'.omp')
  if kind.startswith('manual-'):
   s.reconcile();change=kind.removeprefix('manual-');lock=s.native.lock
   if change=='removed':s.native.remove(s.name)
   elif change=='enabled':lock['plugins'][s.name]['enabled']=False;write_json(s.native.root/'omp-plugins.lock.json',lock)
   elif change=='features':lock['plugins'][s.name]['enabledFeatures']=['extra'];write_json(s.native.root/'omp-plugins.lock.json',lock)
  if kind.startswith('features-'):
   selected={'null':None,'empty':[],'extra':['extra']}[kind.removeprefix('features-')]
   lock=s.native.lock;lock['plugins'][s.name]['enabledFeatures']=selected;lock['settings'][s.name]={'endpoint':'https://custom.example.invalid','attempts':2};write_json(s.native.root/'omp-plugins.lock.json',lock)
   s.reconcile();s.advance()
  if kind=='native-disabled':
   lock=s.native.lock;lock['plugins'][s.name]['enabled']=False;write_json(s.native.root/'omp-plugins.lock.json',lock)
  if kind=='native-no-proof':
   import shutil
   shutil.rmtree(s.native.path(s.name)/'.git')
  if kind.startswith('collision-'):
   origin={'scheme':'http://git.example.invalid/team/comet.git','host':'https://other.example.invalid/team/comet.git','port':'https://git.example.invalid:8443/team/comet.git','path':'https://git.example.invalid/other/comet.git','case':'https://git.example.invalid/team/Comet.git'}[kind.removeprefix('collision-')]
   s.native.repositories[origin]=s.repository_path;s.native.seed(origin,s.revision)
  if kind.startswith('bad-native-'):(s.native.root/kind.removeprefix('bad-native-')).write_text('{')
  if kind.startswith('bad-ledger-'):
   s.reconcile();field=kind.removeprefix('bad-ledger-');ledger=read_json(s.home/'.hangar/omp-plugin-sync.json');target=ledger['items'][s.name]
   values={'source.id':None,'source.name':'other','source.origin':[],'source.url':'https://other.example.invalid/repo.git','source.revision':'bad','source.enabled':1,'status':'unknown','native':None,'native.proof':None,'native.enabled':'true','native.features':[3],'native.settings':[],'native.digest':'bad','native.path':'/other/project','native.spec':'https://other.example.invalid/repo.git#'+'a'*40}
   parts=field.split('.')
   for part in parts[:-1]:target=target[part]
   target[parts[-1]]=values[field];write_json(s.home/'.hangar/omp-plugin-sync.json',ledger)
  if kind.startswith('fail-'):
   action,failure=kind.removeprefix('fail-').split(':');s.native.failure=(action,failure)
  initial=snapshot(s.home);calls=[]
  def record(run,args,**kw):
   before=snapshot(s.home)
   try:
    result=run(args,**kw);output={'code':result.returncode,'stdout':result.stdout}
   except Exception as e:output={'error':type(e).__name__};result=None
   after=snapshot(s.home)
   calls.append({'args':[str(a).replace(str(s.home),'<HOME>') for a in args], 'cwd':str(kw['cwd']).replace(str(s.home),'<HOME>'), 'timeout':kw['timeout'], 'output':output,'set':{k:v for k,v in after.items() if before.get(k)!=v},'remove':[k for k in before if k not in after]})
   if result is None:raise subprocess.TimeoutExpired(args,kw['timeout'])
   return result
  mp.setattr(module,'_run',lambda args,**kw:record(real_run,args,**kw))
  sync=module.PluginSynchronizer(home=s.home,claude_dir=s.claude_dir,runner=lambda args,**kw:record(s.native,args,**kw))
  result=sync.reconcile(dry_run=kind in ['dry-run','dry-run-absent'])
  normalized=lambda v:normalize(v,s.home)
  return {'name':kind,'env':{k:v.replace(str(s.home),'<HOME>') for k,v in sync.env.items() if k in ['HOME','USERPROFILE','CLAUDE_CONFIG_DIR','PI_CODING_AGENT_DIR','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME']},'initial':initial,'calls':normalized(calls),'report':normalized(result),'final':snapshot(s.home)}
cases=[case(k) for k in ['fresh','unchanged','adopt','update','disable','remove','manual','corrupt-ledger','ambiguous','scope','inactive','inactive-manual','missing-sha','invalid-registry','manifest','dry-run','update-fails','equivalent','subdir','dry-run-absent','manual-enabled','manual-features','manual-removed'] + ['features-'+f for f in ['null','empty','extra']] + ['native-disabled','native-no-proof'] + ['collision-'+f for f in ['scheme','host','port','path','case']] + ['bad-native-'+f for f in ['package.json','omp-plugins.lock.json','installed_plugins.json']] + ['bad-ledger-'+f for f in ['source.id','source.name','source.origin','source.url','source.revision','source.enabled','status','native','native.proof','native.enabled','native.features','native.settings','native.digest','native.path','native.spec']] + ['fail-'+f for f in ['list:error','list:timeout','install:error','install:timeout','install:no-effect']]]
OUT.write_text(json.dumps({'origin':ORIGIN,'platform':sys.platform,'command':'cd backend && uv run python ../crates/hangar-server/src/omp_sync/gen_contract.py','cases':cases},ensure_ascii=False,indent=2)+'\n')
print(OUT,len(cases))

from tests.test_omp_context import context_home,ConfigCLI
from app import omp_context
from tests.test_omp_marketplaces import catalogs,CatalogCLI

def context_case(kind):
 with tempfile.TemporaryDirectory() as d, pytest.MonkeyPatch.context() as mp:
  home,agent,claude,cli=context_home.__wrapped__(Path(d),mp)
  template=Path(d)/'templates'/omp_context.RULE_TEMPLATE.name;template.parent.mkdir();template.write_bytes(omp_context.RULE_TEMPLATE.read_bytes());mp.setattr(omp_context,'RULE_TEMPLATE',template)
  rule=agent/'rules'/template.name
  if kind=='append-file':(agent/'APPEND_SYSTEM.md').write_text('Escolha pessoal')
  if kind=='append-link':
   (home/'other.md').write_text('Outro contexto');(agent/'APPEND_SYSTEM.md').symlink_to(home/'other.md')
  if kind in ['equivalent','canonical-custom','restricted-enabled','restricted-agents','restricted-condition','restricted-always','typed-bool','typed-key','duplicate-key','nested','mdc']:
   if kind=='equivalent':rule=agent/'rules/personal.md'
   if kind=='nested':rule=agent/'rules/nested/policy.md'
   if kind=='mdc':rule=agent/'rules/personal.mdc'
   rule.parent.mkdir(parents=True);content=template.read_text()
   metadata={'restricted-enabled':'enabled: false','restricted-agents':'agents: [reviewer]','restricted-condition':"condition: ['x']",'restricted-always':'alwaysApply: false','typed-bool':'enabled: !!bool [true]','typed-key':'? !!str [extra]\n: value','duplicate-key':'enabled: true\nenabled: true'}.get(kind)
   if metadata:content=content.replace('alwaysApply: true','alwaysApply: true\n'+metadata)
   if kind=='canonical-custom':content='Instruções pessoais diferentes'
   rule.write_text(content)
  if kind=='global-missing':(claude/'CLAUDE.md').unlink()
  if kind.startswith('block-'):
   config=read_json(cli.path);name=template.stem
   if kind=='block-id':config['disabledExtensions'].append('rule:'+name)
   if kind=='block-provider':config['disabledProviders']=['native']
   if kind=='block-ttsr':config['ttsr']={'disabledRules':[name]}
   write_json(cli.path,config)
  if kind in ['swap-append','swap-dir','rules-symlink']:
   (home/'external.md').write_text('Contexto externo');(home/'external-rules').mkdir()
   if kind=='rules-symlink':(agent/'rules').symlink_to(home/'external-rules',target_is_directory=True)
  initial=snapshot(home);calls=[]
  def native(args,**kw):
   before=snapshot(home);result=cli(args,**kw)
   if args[2]=='get':
    if kind=='swap-append' and not (agent/'APPEND_SYSTEM.md').is_symlink():(agent/'APPEND_SYSTEM.md').symlink_to(home/'external.md')
    if kind=='swap-dir' and not (agent/'rules').is_symlink():(agent/'rules').symlink_to(home/'external-rules',target_is_directory=True)
   after=snapshot(home);calls.append({'args':args,'cwd':str(kw['cwd']),'timeout':kw['timeout'],'output':{'code':result.returncode,'stdout':result.stdout},'set':{k:v for k,v in after.items() if before.get(k)!=v},'remove':[k for k in before if k not in after]});return result
  result=omp_context.configure_claude_context(home=home,claude_dir=claude,enabled=kind!='disabled',runner=native)
  normalized=lambda v:normalize(v,home,template)
  return normalized({'name':'context-'+kind,'operation':'context','enabled':kind!='disabled','template':template.read_text(),'env':{'HOME':str(home),'USERPROFILE':str(home),'CLAUDE_CONFIG_DIR':str(claude),'PI_CODING_AGENT_DIR':str(agent)},'initial':initial,'calls':calls,'report':result,'final':snapshot(home)})

def marketplace_case(kind):
 with tempfile.TemporaryDirectory() as d, pytest.MonkeyPatch.context() as mp:
  home,definitions=catalogs.__wrapped__(Path(d),mp)
  for key in ('OMP_PROFILE','PI_PROFILE','PI_CONFIG_DIR','XDG_DATA_HOME'):mp.delenv(key,raising=False)
  native=CatalogCLI(home,definitions,no_effect=kind=='no-effect')
  if kind=='conflict':write_json(native.registry,{'version':1,'marketplaces':[{'name':'catalog-alpha','sourceType':'git','sourceUri':'https://other.example.invalid/catalog.git'}]})
  if kind=='secret':definitions['catalog-alpha']['source']={'source':'git','url':'https://user:secret@example.invalid/catalog.git'};write_json(home/'.claude/plugins/known_marketplaces.json',definitions)
  if kind=='unchanged':module.PluginSynchronizer(home=home,claude_dir=home/'.claude',runner=native).import_marketplaces()
  initial=snapshot(home);calls=[]
  def run(args,**kw):
   before=snapshot(home);result=native(args,**kw);after=snapshot(home);calls.append({'args':args,'cwd':str(kw['cwd']),'timeout':kw['timeout'],'output':{'code':result.returncode,'stdout':result.stdout},'set':{k:v for k,v in after.items() if before.get(k)!=v},'remove':[k for k in before if k not in after]});return result
  sync=module.PluginSynchronizer(home=home,claude_dir=home/'.claude',runner=run);result=sync.import_marketplaces(dry_run=kind=='dry-run')
  normalized=lambda v:normalize(v,home)
  return normalized({'name':'marketplace-'+kind,'operation':'marketplaces','env':{k:v for k,v in sync.env.items() if k in ['HOME','USERPROFILE','CLAUDE_CONFIG_DIR','PI_CODING_AGENT_DIR']},'initial':initial,'calls':calls,'report':result,'final':snapshot(home)})

cases.extend(context_case(k) for k in ['normal','disabled','append-file','append-link','equivalent','canonical-custom','global-missing','restricted-enabled','restricted-agents','restricted-condition','restricted-always','typed-bool','typed-key','duplicate-key','nested','mdc','block-id','block-provider','block-ttsr','swap-append','swap-dir','rules-symlink'])
cases.extend(marketplace_case(k) for k in ['fresh','unchanged','conflict','dry-run','no-effect','secret'])
OUT.write_text(json.dumps({'origin':ORIGIN,'platform':sys.platform,'command':'cd backend && uv run python ../crates/hangar-server/src/omp_sync/gen_contract.py','cases':cases},ensure_ascii=False,indent=2)+'\n')
print('Total',len(cases))
