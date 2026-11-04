"""Mede o dry-run público na base Python, usando o mesmo estado da referência Rust."""
import base64,json,os,sys,tempfile,time
from pathlib import Path
sys.path.insert(0,os.getcwd())
reference=Path(__file__).with_name('contract-windows.json' if os.name=='nt' else 'contract.json')
case=next(c for c in json.loads(reference.read_text(encoding='utf-8'))['cases'] if c['name']=='unchanged')
with tempfile.TemporaryDirectory() as temporary:
 home=Path(temporary)/'home';home.mkdir()
 def expand(v):
  if isinstance(v,str):return v.replace('<HOME_JSON>',json.dumps(str(home))[1:-1]).replace('<HOME>',str(home))
  if isinstance(v,dict):return {k:expand(x) for k,x in v.items()}
  if isinstance(v,list):return [expand(x) for x in v]
  return v
 case=expand(case)
 for relative,entry in case['initial'].items():
  path=home/relative
  if entry.get('dir'):path.mkdir(parents=True,exist_ok=True);continue
  path.parent.mkdir(parents=True,exist_ok=True)
  if 'text' in entry:path.write_bytes(entry['text'].encode())
  elif 'base64' in entry:path.write_bytes(base64.b64decode(entry['base64']))
  else:path.symlink_to(entry['link'],target_is_directory=Path(entry['link']).is_dir())
 for key in ('OMP_PROFILE','PI_PROFILE','PI_CONFIG_DIR'):os.environ.pop(key,None)
 os.environ.update(case['env'])
 from app.omp_plugin_sync import PluginSynchronizer
 def forbidden(*args,**kwargs):raise AssertionError('O dry-run iniciou processo')
 sync=PluginSynchronizer(home=home,claude_dir=home/'.claude',runner=forbidden)
 for _ in range(5):assert sync.reconcile(dry_run=True)['errors']==[]
 before=time.process_time();samples=[]
 for _ in range(50):
  start=time.perf_counter();report=sync.reconcile(dry_run=True);samples.append((time.perf_counter()-start)*1000)
  assert report['errors']==[] and report['items'][0]['action']=='unchanged'
 cpu=(time.process_time()-before)*1000
 if os.name=='nt':
  import psutil
  rss=psutil.Process().memory_info().peak_wset//1024
 else:
  import resource
  rss=int(next(line.split()[1] for line in Path('/proc/self/status').read_text().splitlines() if line.startswith('VmHWM:'))) if sys.platform=='linux' else resource.getrusage(resource.RUSAGE_SELF).ru_maxrss//1024
 samples.sort()
 print(json.dumps({'language':'python','platform':sys.platform,'scenario':'unchanged-dry-reconcile','samples':50,'warmups':5,'p50_ms':samples[25],'p95_ms':samples[47],'cpu_ms':cpu,'peak_rss_kib':rss},ensure_ascii=False))
