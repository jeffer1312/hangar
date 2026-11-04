"""Executa as asserções originais congeladas e registra cada passagem pública.

Execute de backend/ da base original, apontando este script da migração.
"""
import inspect,itertools,json,os,subprocess,sys,tempfile,threading
from pathlib import Path
sys.path.insert(0,os.getcwd())
import pytest
from app import omp_plugin_sync as original
from tests import test_omp_plugin_sync as tests
helpers=Path(__file__).with_name('gen_contract.py').read_text(encoding='utf-8');namespace={'__file__':str(Path(__file__).with_name('gen_contract.py'))}
exec(helpers[:helpers.index('def case(kind):')],namespace)
snapshot,normalize=namespace['snapshot'],namespace['normalize']
origin=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
excluded={'test_desfazer_que_falha_nao_esconde_a_causa_original','test_digest_so_rele_arquivo_quando_a_assinatura_muda','test_duas_threads_concorrentes_instalam_uma_unica_vez'}
functions=[v for k,v in vars(tests).items() if k.startswith('test_') and k not in excluded]
ids={name:i+1 for i,name in enumerate(k for k in vars(tests) if k.startswith('test_'))}
from tests import test_omp_plugin_sync_loop as loop_tests
for number,name in [(43,'test_parada_durante_precondicao_nao_inicia_mutacao'),(48,'test_mudanca_de_layout_nao_migra_ledger_existente')]:
 function=getattr(loop_tests,name);functions.append(function);ids[name]=number
records=[];base_reconcile=original.PluginSynchronizer.reconcile

def parameters(function):
 rows=[{}]
 for mark in getattr(function,'pytestmark',[]):
  if mark.name!='parametrize':continue
  names=[v.strip() for v in mark.args[0].split(',')];options=[]
  for value in mark.args[1]:
   value=getattr(value,'values',value)
   if len(names)==1:value=[value]
   options.append(dict(zip(names,value)))
  rows=[{**row,**option} for row in rows for option in options]
 return rows

for function in functions:
 for parameter in parameters(function):
  with tempfile.TemporaryDirectory() as temporary,pytest.MonkeyPatch.context() as patch:
   root=Path(temporary);scenario=tests.scenario.__wrapped__(root,patch);captures=[]
   def reconcile(self,**arguments):
    before=snapshot(self.home);calls=[];stop=arguments.get('stop_requested') or (lambda:False);initial_stop=bool(stop());old_run,old_runner=original._run,self.runner;gate=threading.Lock()
    def traced(delegate):
     def invoke(args,**kwargs):
      with gate:
       preceding=snapshot(self.home);output=None
       try:
        result=delegate(args,**kwargs);output={'code':result.returncode,'stdout':result.stdout};return result
       except Exception as error:
        output={'error':type(error).__name__};raise
       finally:
        following=snapshot(self.home);calls.append({'args':list(args),'cwd':str(kwargs['cwd']),'timeout':kwargs['timeout'],'output':output,'set':{k:v for k,v in following.items() if preceding.get(k)!=v},'remove':[k for k in preceding if k not in following],'stop_after':bool(stop())})
     return invoke
    original._run,self.runner=traced(old_run),traced(old_runner)
    try:result=base_reconcile(self,**arguments)
    finally:original._run,self.runner=old_run,old_runner
    captures.append(normalize({'initial':before,'calls':calls,'report':result,'final':snapshot(self.home),'stop':initial_stop,'dry_run':bool(arguments.get('dry_run',False)),'env':{k:v for k,v in self.env.items() if k in {'HOME','USERPROFILE','CLAUDE_CONFIG_DIR','PI_CODING_AGENT_DIR','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME'}}},self.home));return result
   patch.setattr(original.PluginSynchronizer,'reconcile',reconcile)
   supplied={'scenario':scenario,'tmp_path':root,'monkeypatch':patch,**parameter}
   function(**{k:supplied[k] for k in inspect.signature(function).parameters})
   name=function.__name__;records.append({'id':f'OP{ids[name]:03}','test':name,'parameters':parameter,'steps':captures,'assertions_passed':True});print(name,parameter,len(captures),flush=True)
output=Path(__file__).with_name('assertion-contract-windows.json' if os.name=='nt' else 'assertion-contract.json')
output.write_text(json.dumps({'origin':origin,'platform':sys.platform,'cases':records},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
print(output,'funções',len(functions),'casos',len(records),'passagens',sum(len(r['steps']) for r in records))
