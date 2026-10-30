"""Captura os dois destinos e a impressão original, sem rede nem configurações reais."""
import argparse,ast,contextlib,io,json,subprocess,sys,types
from pathlib import Path
import qrcode
from pydantic import field_validator, ValidationError
from pydantic_settings import BaseSettings, SettingsConfigDict
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();reference=a.reference.resolve();output=a.output.resolve()
ns={'io':io,'sys':sys,'qrcode':qrcode,'detect_lan_ip':lambda:'192.0.2.10','_LOOPBACK':{'127.0.0.1','localhost','::1','0.0.0.0','auto'}}
for name,functions in [('config.py',{'porta_do_front','pairing_url','pairing_url_api'}),('main.py',{'print_pairing'}),('doctor.py',{'_qr'})]:
 tree=ast.parse((reference/'backend/app'/name).read_text());nodes=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in functions];exec(compile(ast.Module(body=nodes,type_ignores=[]),'<original-pairing>','exec'),ns)
class Output(io.StringIO):
 def isatty(self):return self.tty
cases=[('default',{}),('front',{'front_port':5173}),('public',{'public_url':'https://fixture.invalid///'}),('ipv6',{'lan_bind_ip':'::'}),('empty-bind',{'lan_bind_ip':''}),('empty-token',{'auth_token':''}),('negative-port',{'port':-1}),('large-port',{'port':10**30}),('unicode-token',{'auth_token':'fixture&token/é'})]
rows=[]
for name,changes in cases:
 values={'public_url':'','lan_bind_ip':'0.0.0.0','port':8765,'front_port':0,'auth_token':'fixture-token'};values.update(changes);settings=types.SimpleNamespace(**values)
 for tty in [False,True]:
  stream=Output();stream.tty=tty
  with contextlib.redirect_stdout(stream):ns['print_pairing'](settings)
  doctor=Output();doctor.tty=tty
  with contextlib.redirect_stdout(doctor):doctor_rc=ns['_qr'](settings)
  rows.append({'doctor_stdout':doctor.getvalue(),'doctor_rc':doctor_rc,'name':name,'tty':tty,'settings':values,'pwa':ns['pairing_url'](settings),'api':ns['pairing_url_api'](settings),'stdout':stream.getvalue(),'rc':0})
tree=ast.parse((reference/'backend/app/config.py').read_text())
original=next(n for n in tree.body if isinstance(n,ast.ClassDef) and n.name=='Settings')
fields={'port','front_port','model_config'}
selected=[n for n in original.body if isinstance(n,ast.AnnAssign) and isinstance(n.target,ast.Name) and n.target.id in fields or isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='model_config' for t in n.targets) or isinstance(n,ast.FunctionDef) and n.name=='_front_port_vazio_e_ausencia']
cls=ast.ClassDef(name='CapturedSettings',bases=[ast.Name(id='BaseSettings',ctx=ast.Load())],keywords=[],body=selected,decorator_list=[])
space={'BaseSettings':BaseSettings,'SettingsConfigDict':SettingsConfigDict,'field_validator':field_validator}
exec(compile(ast.fix_missing_locations(ast.Module(body=[cls],type_ignores=[])),'<original-settings>', 'exec'),space)
coercions=[]
for key in ['port','front_port']:
 for raw in ['', ' ', '1.0','+12','1_000','１２','-1',str(10**30),'1e3','nan']:
  try: result={'value':getattr(space['CapturedSettings'](**{key:raw},_env_file=None),key)}
  except ValidationError as error: result={'error':error.errors(include_url=False)[0]['type']}
  coercions.append({'key':key,'raw':raw,**result})
output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip(),'command':'python generate-pairing-print.py --reference <checkout-Python> --output <fixture>','scope':'Funções originais com configurações sintéticas já tipadas; LAN injetada; sem opção quiet no original.','cases':rows,'settings_coercions':coercions},ensure_ascii=False,indent=2)+'\n');print(len(rows))
