"""Captura impedimentos da atualização automática, sem rede nem processos."""
import argparse,ast,datetime as dates,io,json,subprocess,types
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve()
source=ref/'backend/app/api.py';tree=ast.parse(source.read_text());function=next(node for node in tree.body if isinstance(node,ast.FunctionDef) and node.name=='_auto_update_motivo');code=compile(ast.Module(body=[function],type_ignores=[]),str(source),'exec')
now=dates.datetime(2026,1,2,12,tzinfo=dates.timezone.utc)
class Clock:
 @staticmethod
 def now():return now
 fromisoformat=dates.datetime.fromisoformat
scenarios=[('ready',{}),('channel',{'target':'beta'}),('dependencies',{'pre':{'pode':False}}),('work-branch',{'pre':{'branch_de_trabalho':True,'branch':'work/task'}}),('switch',{'switch':True,'pre':{'branch':'beta'}}),('diverged',{'pre':{'divergiu':True,'ahead':1}}),('ahead',{'pre':{'ahead':1}}),('uncertain',{'pre':{'ahead_incerto':True}}),('current',{'pre':{'behind':0}}),('dirty',{'pre':{'sujo':1}}),('running',{'state':{'fase':'rodando'}}),('failed-recent',{'state':{'ok':False,'ts':(now-dates.timedelta(hours=1)).isoformat()}}),('failed-expired',{'state':{'ok':False,'ts':(now-dates.timedelta(hours=24)).isoformat()}}),('failed-future',{'state':{'ok':False,'ts':(now+dates.timedelta(hours=1)).isoformat()}}),('failed-invalid',{'state':{'ok':False,'ts':'invalid'}}),('pin-missing',{'pin':None}),('pin-older',{'pin':'older'}),('dist-error',{'dist_error':True}),('dist-utf8',{'dist_hex':'ff'}),('git-error',{'git_code':1}),('git-empty',{'git_out':''}),('dist-mismatch',{'dist_hex':b'other'.hex()})]
scenarios += [(name,{'state':{'ok':False,'ts':stamp}}) for name,stamp in [('failed-naive','2026-01-02T11:00:00'),('failed-date','2026-01-02'),('failed-separator','2026-01-02x11:00:00+00:00'),('failed-basic','20260102T110000+0000'),('failed-week','2026-W01-5T11:00:00+00:00'),('failed-null',None)]]
constants={node.targets[0].id:ast.literal_eval(node.value) for node in tree.body if isinstance(node,ast.Assign) and isinstance(node.targets[0],ast.Name) and node.targets[0].id in ['_DIST_SHA_URL','_AUTO_UPDATE_FALHA_JANELA_S']}
rows=[]
for name,changes in scenarios:
 case={'target':'main','pre':{'pode':True,'branch':'main','alvo':'main','branch_de_trabalho':False,'divergiu':False,'ahead':0,'ahead_incerto':False,'behind':1,'sujo':0},'state':{},'switch':False,'pin':'origin/main','dist_error':False,'dist_hex':b'fixture-head\n'.hex(),'git_code':0,'git_out':'fixture-head\n'}
 for key,value in changes.items():
  if key=='pre':case[key].update(value)
  else:case[key]=value
 calls=[];warnings=[]
 def record(name,value,*args,**kwargs):calls.append([name,list(args),kwargs]);return value
 def download(url,timeout):
  calls.append(['download',[url],{'timeout':timeout}])
  if case['dist_error']:raise OSError('fixture failure')
  return io.BytesIO(bytes.fromhex(case['dist_hex']))
 updater=types.SimpleNamespace(alvo=lambda:record('target',case['target']),checar=lambda:record('preflight',case['pre']),_troca_de_branch=lambda pre:record('switch',case['switch']),estado_para_tela=lambda:record('state',case['state']),pinned_target=lambda branch:record('pin',(case['pin'],None),branch),_git=lambda *args,**kwargs:record('git',subprocess.CompletedProcess(args,case['git_code'],case['git_out'],''),*args,**kwargs))
 ns={'Optional':__import__('typing').Optional,'atualizar':updater,'datetime':Clock,'urllib':types.SimpleNamespace(request=types.SimpleNamespace(urlopen=download)),'_AUTO_UPDATE_FALHA_JANELA_S':constants['_AUTO_UPDATE_FALHA_JANELA_S'],'_DIST_SHA_URL':constants['_DIST_SHA_URL'],'_log':types.SimpleNamespace(warning=lambda *args:warnings.append(list(args)))}
 exec(code,ns);reason=ns['_auto_update_motivo']();rows.append(dict(name=name,input=case,reason=reason,calls=calls,warnings=warnings,rendered_warnings=[row[0] % tuple(row[1:]) for row in warnings]))
out.write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip(),command='python generate-auto-update.py --reference <checkout-Python> --output <fixture>',now=now.isoformat(),cases=rows),ensure_ascii=True,indent=2)+'\n');print(f'{len(rows)} impedimentos automáticos capturados')
