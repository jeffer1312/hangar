"""Captura os casos já declarados de instalação nativa pelo atualizador, sem rede ou instalador."""
import io,json,os,sys,tempfile,subprocess,urllib.error,urllib.request
from pathlib import Path
ref=Path(sys.argv[1]).resolve();out=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='native-update-reference-') as td:
 base=Path(td);os.environ.update(HOME=td,USERPROFILE=td,HANGAR_NATIVE_UPDATE_URL='https://native.example/release');os.chdir(base)
 appdir=base/'app';appdir.mkdir();(appdir/'__init__.py').write_text('__path__.append('+repr(str(ref/'backend/app'))+')\n')
 for name in ['atualizar','atualizacoes','rust_release']:(appdir/f'{name}.py').write_bytes(subprocess.check_output(['git','-C',str(ref),'show',f'{commit}:backend/app/{name}.py']))
 sys.path.insert(0,str(base));from app import atualizar as app
 app.config.settings.auth_token='fixture-token';repo=base/'repo';repo.mkdir();app.REPO=repo;marker=base/'.hangar/native/release.json';marker.parent.mkdir(parents=True)
 specs=[dict(name='missing',marker=None),dict(name='source-build',marker='{"sem_build":true}'),dict(name='marker-array',marker='[]'),dict(name='marker-invalid',marker='{quebrado'),dict(name='beta',target='beta')]
 for windows in [False,True]:
  for required,api in [(2,2),(2,3),('missing',1)]:specs.append(dict(name=f'compatible-{windows}-{required}-{api}',windows=windows,required=required,api=api))
 for api in [1,None,'2',True,0]:specs.append(dict(name='api-'+repr(api),api=api))
 for source in ['api','manifest']:
  for failure in ['503','unreadable','not-object']:specs.append(dict(name=f'{source}-{failure}',source=source,failure=failure))
 for required in [None,'2',True,0]:specs.append(dict(name='required-'+repr(required),required=required))
 specs.append(dict(name='old-404',required='missing',source='api',failure='404'))
 original_get=app.rust_release._get
 rows=[]
 for spec in specs:
  if marker.exists():marker.unlink()
  if spec.get('marker','{}') is not None:marker.write_text(spec.get('marker','{}'))
  records=[];requests=[];commands=[];warnings=[];downloads=[];app._E_WINDOWS=spec.get('windows',False)
  def open_url(request,timeout):
   url=request.full_url if isinstance(request,urllib.request.Request) else request;source='api' if url.startswith('http://127.0.0.1:') else 'manifest';requests.append(dict(source=source,timeout=timeout))
   if source=='api':assert request.get_header('Authorization')=='Bearer fixture-token'
   if spec.get('source')==source:
    failure=spec['failure']
    if failure in ['503','404']:raise urllib.error.HTTPError(url,int(failure),'fixture',{},None)
    if failure=='unreadable':return io.BytesIO(b'{quebrado')
    if failure=='not-object':return io.BytesIO(b'[]')
   payload={'api':spec.get('api',2)} if source=='api' else ({} if spec.get('required')=='missing' else {'requires_api':spec.get('required',2)})
   return io.BytesIO(json.dumps(payload).encode())
  def download(url,deadline):
   downloads.append(dict(deadline=deadline));return original_get(url,deadline)
  app.rust_release._get=download
  urllib.request.urlopen=open_url;app.diag.registrar=lambda event,level='ok',**fields:records.append(dict(evento=event,nivel=level,**{k:v for k,v in fields.items() if v is not None and v!=''}));app._avisar=warnings.append
  def run(args,**kw):commands.append(dict(args=[str(v).replace(str(repo),'<ROOT>') for v in args],timeout=kw['timeout'],cwd='<ROOT>'));return subprocess.CompletedProcess(args,0,'instalado','')
  app._rodar=run;app._instalar_nativo(spec.get('target','main'),9876)
  rows.append(dict(input=spec,requests=requests,downloads=downloads,commands=commands,events=records,warnings=warnings))
 out.write_text(json.dumps(dict(source_commit=commit,command='python generate-native-update.py <reference> <output>',cases=rows),ensure_ascii=False,indent=2)+'\n');print(len(rows),'casos capturados')
