"""Captura passos de subida executando a função original com dependências isoladas."""
import argparse,ast,contextlib,io,json,os,subprocess,sys,types
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve()
source=ref/'backend/app/main.py';tree=ast.parse(source.read_text());function=next(node for node in tree.body if isinstance(node,ast.FunctionDef) and node.name=='_passos_pendentes_da_versao');module=ast.Module(body=[function],type_ignores=[]);namespace={'os':os};exec(compile(module,str(source),'exec'),namespace)
rows=[]
for name,windows,package,pending,done,failed in [('package',False,True,[],[],False),('linux-applied',False,False,[],['one','two'],False),('linux-empty',False,False,[],[],False),('linux-failed',False,False,[],[],True),('windows-installer',True,False,[{'comando':'powershell install.ps1'}],[],False),('windows-mixed',True,False,[{'comando':'other.ps1'},{'comando':'install.ps1'}],[],False),('windows-applied',True,False,[{'comando':'other.ps1'}],['one'],False)]:
 calls=[]
 def manifest():calls.append(['manifest']);return {'version':'fixture'} if package else None
 def waiting(**kwargs):calls.append(['pending',kwargs]);return pending
 def apply(**kwargs):
  calls.append(['apply',kwargs])
  if failed:raise RuntimeError('fixture failure')
  return done
 app=types.ModuleType('app');app.diag=types.SimpleNamespace(install_manifest=manifest);app.atualizacoes=types.SimpleNamespace(pendentes=waiting,aplicar_pendentes=apply);sys.modules['app']=app
 output=io.StringIO()
 with contextlib.redirect_stdout(output):result=namespace['_passos_pendentes_da_versao'](windows)
 rows.append(dict(name=name,windows=windows,package=package,pending=pending,done=done,failed=failed,calls=calls,stdout=output.getvalue(),result=result))
out.write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip(),command='python generate-startup-steps.py --reference <checkout-Python> --output <fixture>',cases=rows),ensure_ascii=True,indent=2)+'\n')
print(f'{len(rows)} casos de passos na subida capturados')
