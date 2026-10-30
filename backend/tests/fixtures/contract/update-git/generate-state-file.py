"""Captura leitura e escrita do estado original, com relógio e diretório isolados."""
import argparse,ast,datetime,json,logging,os,pathlib,subprocess,tempfile,types
parser=argparse.ArgumentParser();parser.add_argument('--reference',type=pathlib.Path,required=True);parser.add_argument('--output',type=pathlib.Path,required=True);args=parser.parse_args();root=args.reference.resolve();tree=ast.parse((root/'backend/app/atualizar.py').read_text());nodes=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in {'estado','_escrever'}]
class Frozen:
 @staticmethod
 def now():return datetime.datetime(2026,1,2,3,4,5,tzinfo=datetime.timezone.utc)
rows=[]
with tempfile.TemporaryDirectory() as temporary:
 folder=pathlib.Path(temporary);file=folder/'estado.json'
 ns={'Path':pathlib.Path,'json':json,'os':os,'datetime':Frozen,'_caminho_estado':lambda:file,'atomico':types.SimpleNamespace(substituir=os.replace,explicar=str),'_log':logging.getLogger('fixture')}
 exec(compile(ast.Module(body=nodes,type_ignores=[]),'<original-state>','exec'),ns)
 for name,raw in [('missing',None),('empty',b''),('list',b'[]'),('string',b'"x"'),('null',b'null'),('nan',b'{"keep":NaN}'),('infinity',b'{"keep":Infinity}'),('surrogate',b'{"keep":"\\ud800"}'),('unicode','{"keep":"é\U0010f800"}'.encode()),('deep',b'{"keep":'+b'['*1500+b'0'+b']'*1500+b'}'),('deep-10000',b'{"keep":'+b'['*10000+b'0'+b']'*10000+b'}')]:
  for p in folder.iterdir():p.unlink()
  if raw is not None:file.write_bytes(raw)
  row={'name':name,'input_hex':raw.hex() if raw is not None else None}
  for op,call in [('read',lambda:ns['estado']()),('write',lambda:ns['_escrever'](fase='pronto'))]:
   try:
    value=call();row[op]={'value':json.dumps(value,ensure_ascii=True)}
   except Exception as error:row[op]={'exception':type(error).__name__,'message':str(error)}
  row['file_hex']=file.read_bytes().hex() if file.exists() else None;row['temporary_files']=[p.name.replace(str(os.getpid()),'<PID>') for p in folder.iterdir() if p!=file];rows.append(row)
args.output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),'command':'python backend/tests/fixtures/contract/update-git/generate-state-file.py --reference <checkout-Python> --output <fixture>', 'cases':rows},ensure_ascii=False,indent=2))
print([(r['name'],r['read'].get('exception'),r['write'].get('exception')) for r in rows])
