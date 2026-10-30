"""Executa as quatro recusas originais e registra suas respostas sem serviço real."""
import argparse,importlib.util,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
import pytest
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d'
with tempfile.TemporaryDirectory() as scratch:
 base=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CP_UPDATE_BRANCH='');os.chdir(base)
 with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','-C',str(ref),'archive',commit,'backend/app','backend/tests/test_atualizar.py']))) as tar:tar.extractall(base,filter='data')
 sys.path.insert(0,str(base/'backend'));spec=importlib.util.spec_from_file_location('original',base/'backend/tests/test_atualizar.py');tests=importlib.util.module_from_spec(spec);spec.loader.exec_module(tests);app=tests.atualizar;rows=[]
 for name in ['test_preparar_baixa_dist_e_binario_juntos_e_guarda_os_dois_avisos','test_preparar_com_dist_que_explode_guarda_o_aviso_do_binario','test_binario_baixado_junto_so_e_gravado_com_a_tela_no_lugar']:
  for fails in ([False,True] if name.startswith('test_binario') else [None]):
   folder=base/(name+str(fails));folder.mkdir();seen=[]
   with pytest.MonkeyPatch.context() as patch:
    repo=tests.repo.__wrapped__(folder,patch);original=app._preparar
    def prepare(*args,**kwargs):
     try:result=original(*args,**kwargs);seen.append({'ok':True,'warnings':app.estado().get('avisos',[])});return result
     except Exception as error:seen.append({'ok':False,'error':str(error),'warnings':app.estado().get('avisos',[])});raise
    patch.setattr(app,'_preparar',prepare);test=getattr(tests,name);args={'repo':repo,'monkeypatch':patch,'tela_explode':fails};test(**{k:v for k,v in args.items() if k in __import__('inspect').signature(test).parameters});rows.append({'name':name,'fails':fails,'calls':seen})
 out.write_text(json.dumps({'source_commit':commit,'command':'python generate-parallel-assets.py --reference <checkout> --output parallel-assets.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows),'cenários originais passaram')
