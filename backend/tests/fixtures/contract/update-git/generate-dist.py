"""Captura publicação do dist com arquivos regulares e links, em disco temporário."""
import argparse,io,json,os,subprocess,sys,tarfile,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-dist-reference-') as scratch:
 root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(root);sys.path.insert(0,str(ref/'backend'));from app import atualizar as app
 rows=[]
 for name in ['regular','incomplete','internal-link','external-link']:
  app.REPO=root/name;dist=app.REPO/'frontend/dist';dist.mkdir(parents=True);(dist/'index.html').write_text('old');archive=io.BytesIO()
  with tarfile.open(fileobj=archive,mode='w:gz') as tar:
   content=b'new';entry=tarfile.TarInfo('other.html' if name=='incomplete' else 'index.html');entry.size=len(content);tar.addfile(entry,io.BytesIO(content))
   if name.endswith('-link'):
    link=tarfile.TarInfo('linked.html');link.type=tarfile.SYMTYPE;link.linkname='index.html' if name=='internal-link' else '../outside';tar.addfile(link)
  archive.seek(0)
  try:result=app._trocar_pelo_tar(archive,'r:gz');error=None
  except Exception as e:result=None;error=type(e).__name__
  rows.append({'name':name,'result':result,'error':error,'index':(dist/'index.html').read_text(),'previous':(app.REPO/'frontend/.dist-velho/index.html').read_text() if (app.REPO/'frontend/.dist-velho/index.html').exists() else None})
 out.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-dist.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/dist-cases.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} cenários capturados')
