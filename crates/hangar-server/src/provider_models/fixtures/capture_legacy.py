import functools,json,os,sys,inspect,hashlib
from pathlib import Path
from copy import deepcopy
root=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();selection=json.loads(output.read_text())['pytest_functions'];os.chdir(root/'backend');sys.path.insert(0,str(root/'backend'))
import pytest
from app import pi_models as pm,pi_catalog as pc,kimi_models as km
records=[];active='';passed=0
def install(module,name,operation=None,reader=None):
 original=getattr(module,name)
 @functools.wraps(original)
 def wrapped(*args,**kwargs):
  value=reader(args,kwargs) if reader else list(args)
  try:
   answer=original(*args,**kwargs);expected={'ok':deepcopy(answer)}
  except (pm.PiModelError,km.KimiModelError) as error:
   records.append({'pytest':active,'op':operation or name,'input':deepcopy(value),'expected':{'error':{'status':error.status,'detail':error.detail}}});raise
  records.append({'pytest':active,'op':operation or name,'input':deepcopy(value),'expected':expected});return answer
 setattr(module,name,wrapped)
def content(args,kwargs):
 path=args[0] if args else kwargs.get('path')
 try:return [Path(path or km.CONFIG).read_text(encoding='utf-8')]
 except (OSError,UnicodeError):return [None]
def sidecar(args,kwargs):
 path=pm.sidecar_path(*args,**kwargs)
 try:return [path.read_text(encoding='utf-8')]
 except (OSError,UnicodeError):return [None]
for module,names in [(pc,['parse','parse_omp']),(pm,['model_command','think_command','check_known','confirms']),(km,['clean_alias','check_known','check_effort','parse_switched','parse_thinking_set','parse_thinking_row','confirms','confirms_effort'])]:
 for name in names:install(module,name,('pi_' if module is pm else 'kimi_' if module is km else '')+name)
install(km,'read_catalog','kimi_read_catalog',content);install(pm,'read_catalog','pi_read_catalog',sidecar)
class Capture:
 def pytest_runtest_setup(self,item):
  global active;active=item.nodeid
 def pytest_runtest_logreport(self,report):
  global passed
  if report.when=='call' and report.passed:passed+=1
 def pytest_sessionfinish(self,session,exitstatus):
  if exitstatus!=0: return
  output.write_text(json.dumps({'pytest_functions':selection,'passed_assertion_cases':passed,'source_commit':'d2a42410f9fe6df4ea491b296c7fea7b98d8b844','generator':'Execução dos103 testes da matriz com captura das chamadas à referência Python, sem alterar asserções.','cases':records},ensure_ascii=False,indent=2)+'\n')
paths=['tests/'+name for name in selection]
raise SystemExit(pytest.main(['-q','--tb=short',*paths],plugins=[Capture()]))
