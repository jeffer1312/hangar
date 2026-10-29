"""Captura o erro de leitura da thread sem processos nem ambiente real."""
import argparse,json,os,subprocess,sys,tempfile,threading
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-read-error-') as scratch:
 os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(scratch);sys.path.insert(0,str(ref/'backend'));from app import atualizar
 class Output:
  def __iter__(self):
   yield 'partial\n'
   raise OSError('fixture-read-failed')
 class Process:
  stdout=Output();returncode=0
  def wait(self,timeout):return 0
 atualizar.subprocess.Popen=lambda *a,**k:Process()
 failures=[];threading.excepthook=lambda args:failures.append({'type':args.exc_type.__name__,'message':str(args.exc_value)})
 result=atualizar._rodar(['fixture'],timeout=1,log=lambda _:None)
 out.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/peers/generate-read-error.py --reference <checkout-Python> --output backend/tests/fixtures/contract/peers/process-read-error.json','returncode':result.returncode,'stdout':result.stdout,'thread_errors':failures},indent=2)+'\n')
 print(json.dumps({'returncode':result.returncode,'stdout':result.stdout,'thread_errors':failures}))
