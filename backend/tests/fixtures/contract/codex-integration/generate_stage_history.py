"""Captura a proveniência reconhecida pelo histórico nativo, sem adotar caminhos de projetos."""
import ast,asyncio,json,subprocess
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_integracao.py'],cwd=ROOT,text=True);cls=next(n for n in ast.parse(source).body if isinstance(n,ast.ClassDef) and n.name=='IntegracaoCodex');method=next(n for n in cls.body if isinstance(n,ast.AsyncFunctionDef) and n.name=='_historico')
class NativeError(RuntimeError):pass
ns={'Path':Path,'CodexNativoErro':NativeError,'msg':lambda code:{'codigo':code}}
exec(compile(ast.Module(body=[method],type_ignores=[]),'original-stage-history','exec'),ns)
cases=[]
inputs=[[],[{}],[{'successes':None}],[{'successes':True}],[{'successes':'text'}],[{'successes':{}}],[{'successes':[None]}],[{'successes':[{'itemType':[]}]}]]
for provider in [None,'claude-code','other',3]:
 for name in ['name','name.md','a/b','a\\b','.','..','',None,3]:
  for cwd in [None,'/project']:
   inputs.append([{'providerId':provider,'successes':[{'itemType':kind,'target':name,'cwd':cwd} for kind in ['MCP_SERVER_CONFIG','SUBAGENTS','COMMANDS','HOOKS']]}])
async def run():
 for index,entries in enumerate([*inputs,'native-error']):
  async def histories():
   if entries=='native-error':raise NativeError('fixture')
   return entries
  obj=SimpleNamespace(_estado={'avisos':[]});native=SimpleNamespace(historicos_importacao=histories)
  try:result=await ns['_historico'](obj,native);result={'ok':{key:sorted(value) for key,value in result.items()},'warnings':obj._estado['avisos']}
  except Exception as error:result={'error':type(error).__name__,'message':str(error),'warnings':obj._estado['avisos']}
  cases.append(dict(input=entries,result=result))
asyncio.run(run())
Path(__file__).with_name('stage-history.json').write_text(json.dumps(dict(source_commit=COMMIT,platform='posix',command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_stage_history.py',cases=cases),indent=2)+'\n')
print(len(cases))
