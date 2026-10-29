"""Captura datas TOML e erros de decodificação nas checagens originais."""
import json
from pathlib import Path
import subprocess
source=Path(__file__).with_name('generate_checks.py')
ns={'__file__':str(source)}
exec(compile(source.read_text().split('for cli, path, content in [',1)[0],str(source),'exec'),ns)
run=ns['run']
for cli,folder,field in [('codex','.codex','model'),('kimi','.kimi-code','default_model')]:
    for value in ['07:32','07:32:60','0000-01-01','2025-02-29','2024-02-30','1979-05-27','00:00:01','00:00:00.000001','07:32:00','07:32:00.123','1979-05-27T07:32:00','1979-05-27T07:32:00.123456789Z','1979-05-27T07:32:00-07:30','1979-05-27T07:32:00+02:15']:
        for wrap in [lambda x:x,lambda x:'['+x+']',lambda x:'{date='+x+'}']:
            text=wrap(value)
            run(cli+'-model-'+text,'_modelo_padrao',cli,files={folder+'/config.toml':field+'='+text})
        for key in ['mcp_servers']+(['mcp'] if cli=='kimi' else []):
            run(cli+'-'+key+'-'+value,'_mcp',cli,files={folder+'/config.toml':key+'='+value})
# O escritor do harness escreve bytes reais para esta classe; o restante é original.
original=Path.write_text
for cli,folder,field in [('codex','.codex','model'),('kimi','.kimi-code','default_model'),('claude','.claude','model'),('pi','.pi/agent','defaultModel'),('omp','.omp/agent','defaultModel')]:
    for kind in ['_mcp','_modelo_padrao']:
        path=folder+('/config.toml' if cli in ('codex','kimi') else '/mcp.json' if kind=='_mcp' else '/settings.json')
        if cli=='claude' and kind=='_mcp':path='.claude.json'
        for data in [b'\xff',b'\xc3',b'\xef\xbb\xbf{}']:
            from unittest.mock import patch
            def write(self,text,*args,**kwargs):
                return self.write_bytes(data) if text=='FIXTURE_BYTES' else original(self,text,*args,**kwargs)
            with patch.object(Path,'write_text',write):run(cli+'-'+kind+'-'+data.hex(),kind,cli,files={path:'FIXTURE_BYTES'})
            ns['cases'][-1]['files_hex']={path:data.hex()}
Path(__file__).with_name('read-types.json').write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ns['ROOT'],text=True).strip(),
 command='cd backend && uv run python tests/fixtures/contract/harness-health/generate_read_types.py',cases=ns['cases']),ensure_ascii=True,indent=2)+'\n')
print(len(ns['cases']))
