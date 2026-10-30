#!/usr/bin/env python3
"""Prova leitura literal de .env e recusa de UTF-8 inválido sem rede."""
import json,os,subprocess,sys,tempfile
from pathlib import Path
binary=Path(sys.argv[1]).resolve();fixture=json.loads((Path(__file__).resolve().parent.parent/'backend/tests/fixtures/contract/update-git/panel-env.json').read_text());failures=[]
for row in fixture['cases']:
 with tempfile.TemporaryDirectory() as tmp:
  root=Path(tmp);config=root/'.hangar/config';config.mkdir(parents=True);(root/'chosen').mkdir();(root/'other').mkdir()
  if row['input_hex'] is not None:(config/'.env').write_bytes(bytes.fromhex(row['input_hex']).replace(b'<ROOT>',tmp.encode()))
  (config/'peers.json').write_text('{"peer":{}}');dialog=root/'kdialog';dialog.write_text(f'#!{sys.executable}\nimport sys\nprint(sys.argv[2])\n');dialog.chmod(0o700)
  result=subprocess.run([str(binary),'panel-action',*row['args']],env={**os.environ,'HOME':tmp,'USERPROFILE':tmp,'PATH':tmp},cwd=tmp,capture_output=True,timeout=10)
  expected=row['result'];actual=result.stdout.decode().replace(tmp,'<ROOT>');issues=[]
  if result.returncode!=expected['rc']:issues.append('rc')
  if actual!=expected['stdout']:issues.append('stdout')
  if (config/'peers.json').read_text()!=expected['book']:issues.append('livro')
  if expected.get('exception') and result.stderr!='erro: configuração do painel contém Unicode inválido\n'.encode():issues.append('diagnóstico')
  if issues:failures.append((row['name'],issues,actual))
assert not failures,failures
print('14 contratos do leitor literal do painel conferidos sem rede')
