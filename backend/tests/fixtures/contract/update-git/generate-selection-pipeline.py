"""Executa cenários originais de seleção e congela os repositórios antes do motor."""
import argparse,hashlib,base64,importlib.util,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
import pytest
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit='8552d4f3d'
names=['test_topo_sem_binario_para_no_commit_do_binario_publicado','test_commit_que_muda_o_rust_espera_o_binario_dele','test_commit_so_de_python_depois_do_binario_chega','test_main_recebe_branch_que_mesclou_o_build_publicado','test_build_deste_sistema_atrasado_no_manifesto_por_sistema','test_manifesto_por_sistema_sem_este_sistema_nao_confere','test_topo_com_binario_avanca_ate_o_topo','test_sem_binario_deste_sistema_vai_ao_topo_como_antes','test_sem_como_conferir_vai_ao_topo_e_diz','test_sem_binario_conferido_nao_troca_o_contrato_do_checkout','test_sem_binario_conferido_e_nada_no_contrato_do_checkout_fica_onde_esta','test_ja_no_topo_com_binario_de_outro_contrato_nao_recua_e_avisa','test_commit_local_adiante_do_topo_nao_e_atras_do_topo','test_branch_de_teste_nova_nasce_no_commit_escolhido_com_upstream','test_merge_que_muda_o_contrato_conta_pelo_primeiro_pai']
with tempfile.TemporaryDirectory(prefix='selection-reference-') as scratch:
 base=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CP_UPDATE_BRANCH='',GIT_CONFIG_GLOBAL=os.devnull,GIT_CONFIG_NOSYSTEM='1');os.chdir(base)
 archive=subprocess.check_output(['git','-C',str(ref),'archive',commit,'backend/app','backend/tests/test_atualizar.py'])
 with tarfile.open(fileobj=io.BytesIO(archive)) as tar:tar.extractall(base,filter='data')
 sys.path.insert(0,str(base/'backend'));spec=importlib.util.spec_from_file_location('original_tests',base/'backend/tests/test_atualizar.py');tests=importlib.util.module_from_spec(spec);spec.loader.exec_module(tests);app=tests.atualizar;real_restart=app._reiniciar;rows=[]
 for name in names:
  test=getattr(tests,name);marks=[m for m in getattr(test,'pytestmark',[]) if m.name=='parametrize'];variants=[{}]
  for mark in marks:variants=[dict(v,**{mark.args[0]:value}) for v in variants for value in mark.args[1]]
  for index,params in enumerate(variants):
   folder=base/f'{name}-{index}';folder.mkdir()
   with pytest.MonkeyPatch.context() as patch:
    repo=tests.repo.__wrapped__(folder,patch);rust=tests.rust.__wrapped__(repo,folder,patch)
    for key,value in rust['manifesto']['files'].items():value['sha256']=hashlib.sha256(('fixture-'+key.rsplit('/',1)[-1]).encode()).hexdigest()
    patch.setattr(app,'_topologia',lambda:'manual');patch.setattr(app,'_reiniciar',real_restart)
    original=app.executar;record={};requests=[]
    def execute(*args,**kwargs):
     branch=tests._branch(repo);head=tests._rev(repo,'HEAD');bundles={}
     for key,path in [('repo',repo),('origin',folder/'origem.git')]:
      bundle=folder/(key+'.bundle');result=tests._git(path,'bundle','create',str(bundle),'--all');assert result.returncode==0,result.stderr;bundles[key]=base64.b64encode(bundle.read_bytes()).decode()
     read=app.rust_release.read_manifest
     def fetch(url):
      try:value=read(url);requests.append({'url':url,'manifest':value});return value
      except Exception as error:requests.append({'url':url,'error':type(error).__name__,'status':getattr(error,'code',None),'message':str(error)});raise
     patch.setattr(app.rust_release,'read_manifest',fetch)
     record.update(branch=branch,head_before=head,bundles=bundles,target=app.config.settings.update_branch,enabled=app.config.settings.rust_server)
     state=original(*args,**kwargs)
     record.update(state={key:state.get(key) for key in ['fase','ok','avisos','waiting_for_server_build','reiniciar_manual','erro']},head_after=tests._rev(repo,'HEAD'),branch_after=tests._branch(repo),upstream=tests._git(repo,'rev-parse','--abbrev-ref','@{upstream}').stdout.strip(),behind=app._behind_top(),requests=list(requests),prepare_reused_manifest=rust['preparo'].get('release',(None,None,None))[2] is rust['manifesto'] if rust['preparo'].get('release') else False)
     return state
    patch.setattr(app,'executar',execute)
    args={'repo':repo,'rust':rust,'monkeypatch':patch,**params};test(**{k:v for k,v in args.items() if k in __import__('inspect').signature(test).parameters})
    record.update(name=name,parameters=params);rows.append(record)
 out.write_text(json.dumps({'source_commit':commit,'command':'python generate-selection-pipeline.py --reference <checkout> --output selection-pipeline.json','scope':'Testes e fixtures originais executados, bundles antes de executar, fontes externas originais injetadas pelos próprios testes.','cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows),'casos originais com asserções passaram')
