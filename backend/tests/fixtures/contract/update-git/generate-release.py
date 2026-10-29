"""Caracteriza o download e a retenção de binários com transporte determinístico e disco isolado."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.error

parser=argparse.ArgumentParser()
parser.add_argument('--reference',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args();reference=args.reference.resolve();output=args.output.resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-release-reference-') as scratch:
    root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(root);sys.path.insert(0,str(reference/'backend'))
    from app import rust_release as release
    release.diag.registrar=lambda *a,**kw:None
    release.RELEASES_URL='https://release.invalid'
    release.platform_key=lambda:'linux-x86_64'
    rows=[]
    for name in ['new','same','replace','hold','hash-mismatch','missing-entry','download-failure','manifest-list','branch-404-empty','branch-404-keeps','branch-no-platform','unsupported']:
        dest=root/name;dest.mkdir()
        if name in ['same','replace','hold','branch-404-keeps']:
            for binary in release.NAMES:(dest/binary).write_bytes(b'new-'+binary.encode() if name=='same' else b'old-'+binary.encode())
        manifest={'commit':'a'*40,'files':{f'linux-x86_64/{binary}':{'name':binary,'sha256':hashlib.sha256(b'new-'+binary.encode()).hexdigest()} for binary in release.NAMES}}
        if name=='missing-entry':del manifest['files']['linux-x86_64/hangar-cano']
        urls=[]
        def get(url,deadline):
            urls.append(url)
            if url.endswith('server-latest.json'):
                if 'server-beta/' in url and name.startswith('branch-404'):raise urllib.error.HTTPError(url,404,'missing',{},None)
                if 'server-beta/' in url and name=='branch-no-platform':return b'{"files":{}}'
                return b'[]' if name=='manifest-list' else json.dumps(manifest).encode()
            if name=='download-failure':raise OSError('fixture-download-failed')
            return b'wrong' if name=='hash-mismatch' else b'new-'+url.rsplit('/',1)[-1].encode()
        release._get=get
        release.release_tag=lambda:'server-beta' if name.startswith('branch-') else 'server-latest'
        release.platform_key=lambda:None if name=='unsupported' else 'linux-x86_64'
        hold=[] if name=='hold' else None
        result=release.fetch(dest=dest,hold=hold)
        disk=lambda:{p.name:p.read_bytes().decode() for p in dest.iterdir() if p.is_file()}
        before=disk()
        installed=release.install_held(hold) if hold is not None else None
        after=disk()
        restored=release.restore_previous(dest) if hold is not None else None
        rows.append({'name':name,'result':result,'urls':urls,'before_publish':before,'install_warnings':installed,'after_publish':after,'restore_warnings':restored,'after_restore':disk()})
    output.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-release.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/release-cases.json','cases':rows},ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} cenários de release capturados de {commit}')
