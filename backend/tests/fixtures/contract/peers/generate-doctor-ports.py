"""Captura portas do diagnóstico sem conectar ou executar programas."""
import argparse,ast,collections,contextlib,io,json,socket,subprocess,types
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
tree=ast.parse((a.reference/'backend/app/doctor.py').read_text())
nodes=[node for node in tree.body if isinstance(node,ast.FunctionDef) and node.name in {'diagnosticar','main'}]
ns={'Linha':collections.namedtuple('Linha','nivel titulo conserto'),'_WIN':False,'_MARCA':{'ok':'ok  ','aviso':'--  ','erro':'X   '},'_OUTROS_AGENTES':(('codex','Codex'),('pi','Pi'),('omp','omp'),('kimi','Kimi Code')),'_conserto_backend':lambda:'fixture-repair','_reinicios_automaticos':lambda:[],'_binario':lambda name:None,'_tailscale':lambda:('ausente',''),'_lan_responde':lambda s:False}
exec(compile(ast.Module(body=nodes,type_ignores=[]),'<original-doctor>','exec'),ns)
rows=[]
for port in [-1,0,65535,65536,10**30]:
 calls=[]
 ns['_porta_responde']=lambda value:calls.append(value) or False
 ns['settings']=types.SimpleNamespace(port=port,auth_token='fixture-token',public_url='')
 output=io.StringIO()
 with contextlib.redirect_stdout(output):rc=ns['main']([])
 try:resolved={'address':socket.getaddrinfo('127.0.0.1',port,0,socket.SOCK_STREAM)[0][4]}
 except Exception as error:resolved={'error':type(error).__name__,'message':str(error)}
 rows.append({'port':port,'probes':calls,'stdout':output.getvalue(),'rc':rc,'getaddrinfo':resolved})
a.output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(a.reference),'rev-parse','HEAD'],text=True).strip(),'command':'python generate-doctor-ports.py --reference <checkout-Python> --output <fixture>','scope':'Diagnóstico original com todas as sondas substituídas; getaddrinfo real somente para IP numérico local, sem conexão.','cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows))
