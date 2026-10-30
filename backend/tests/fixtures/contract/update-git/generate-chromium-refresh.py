"""Captura o prazo e a retentativa do Chromium sem executar instalador."""
import argparse,ast,json,logging,os,subprocess,sys,tempfile,time,types
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d';tree=ast.parse(subprocess.check_output(['git','-C',str(ref),'show',f'{commit}:backend/app/atualizar.py'],text=True));node=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='_renovar_chromium');rows=[]
with tempfile.TemporaryDirectory() as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch);marker=root/'.hangar/native/chromium-ok';marker.parent.mkdir(parents=True)
 for name,text,age,code in [('absent',None,0,0),('failed-system','sistema: /usr/bin/google-chrome (baixar falhou: x)\n',2,1),('recent','baixado: /x/chrome-headless-shell (1.2.3.4)\n',2,0),('old','baixado: /x/chrome-headless-shell (1.2.3.4)\n',25,0),('failed-recent','download falhou: sem rede\n',2,0)]:
  marker.unlink(missing_ok=True);calls=[]
  if text is not None:marker.write_text(text)
  def run(args,**kwargs):calls.append({'args':[x.replace(str(root),'<ROOT>') for x in args],'cwd':'<ROOT>','timeout':kwargs['timeout']});return types.SimpleNamespace(returncode=code,stdout='',stderr='sem rede' if code else '')
  ns={'Path':Path,'sys':sys,'time':time,'REPO':root,'_rodar':run,'_log':logging.getLogger('fixture'),'_cauda':lambda p,n:p.stderr};exec(compile(ast.Module(body=[node],type_ignores=[]),'<original-refresh>','exec'),ns);ns['_renovar_chromium'](marker.stat().st_mtime+age*3600 if text is not None else time.time());rows.append({'name':name,'marker':text,'age_hours':age,'code':code,'calls':calls})
out.write_text(json.dumps({'source_commit':commit,'command':'python generate-chromium-refresh.py --reference <checkout> --output chromium-refresh.json','cases':rows},ensure_ascii=False,indent=2)+'\n')
