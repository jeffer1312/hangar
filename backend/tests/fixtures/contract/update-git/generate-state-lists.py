"""Caracteriza log e avisos com arquivo original isolado e relógio fixo."""
import argparse,ast,datetime,json,logging,os,pathlib,subprocess,tempfile,types
parser=argparse.ArgumentParser();parser.add_argument('--reference',type=pathlib.Path,required=True);parser.add_argument('--output',type=pathlib.Path,required=True);args=parser.parse_args();root=args.reference.resolve();tree=ast.parse((root/'backend/app/atualizar.py').read_text());nodes=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in {'estado','_escrever','_log_do_estado','_avisar'}]
class Frozen:
 @staticmethod
 def now():return datetime.datetime(2026,1,2,3,4,5,tzinfo=datetime.timezone.utc)
rows=[]
with tempfile.TemporaryDirectory() as temporary:
 file=pathlib.Path(temporary)/'estado.json'
 ns={'json':json,'os':os,'datetime':Frozen,'_caminho_estado':lambda:file,'atomico':types.SimpleNamespace(substituir=os.replace,explicar=str),'_log':logging.getLogger('fixture'),'_SOU_O_MOTOR':True,'_TETO_LOG':400}
 exec(compile(ast.Module(body=nodes,type_ignores=[]),'<original-state-lists>','exec'),ns)
 for op,key in [('_log_do_estado','log'),('_avisar','avisos')]:
  for initial in [None,[],['anterior'],'ab',{'x':1},0,True]:
   file.write_text(json.dumps({key:initial}));row={'operation':op,'initial':initial,'text':'nova'}
   try:ns[op]('nova');row['exception']=None
   except Exception as e:row['exception']=type(e).__name__;row['message']=str(e)
   row['state']=json.loads(file.read_text());row['state'].pop('ts',None);rows.append(row)
 for text in ['', 'a\nb\n','a\rb\r\n','a\u0085b\u2028c\u001cd']:
  file.write_text('{}');ns['_log_do_estado'](text);state=json.loads(file.read_text());state.pop('ts');rows.append({'operation':'_log_do_estado','initial':[],'text':text,'state':state,'exception':None})
args.output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),'command':'python backend/tests/fixtures/contract/update-git/generate-state-lists.py --reference <checkout-Python> --output <fixture>','cases':rows},ensure_ascii=False,indent=2));print(len(rows),'casos')
