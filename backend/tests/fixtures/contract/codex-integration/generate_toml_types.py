"""Registra caminhos tipados do tomllib, distinguindo datas de tabelas particulares."""
import datetime,json,tomllib
from pathlib import Path
cases=[]
for raw in ['value=1979-05-27','value=07:32:00','value=1979-05-27T07:32:00','value=1979-05-27T07:32:00Z','value=1979-05-27T07:32:00-07:00','value=[1979-05-27,07:32:00,1979-05-27T07:32:00]','[nested]\nvalue=1979-05-27','value={"$__toml_private_datetime"="1979-05-27"}','wide='+str(10**90)+'\nvalue=1979-05-27','value="1979-05-27"']:
 value=tomllib.loads(raw);dates=[]
 def visit(value,path):
  if isinstance(value,(datetime.datetime,datetime.date,datetime.time)):dates.append(dict(path=path,kind=type(value).__name__))
  elif isinstance(value,dict):
   for key,item in value.items():visit(item,path+[key])
  elif isinstance(value,list):
   for index,item in enumerate(value):visit(item,path+[str(index)])
 visit(value,[]);cases.append(dict(toml=raw,dates=dates))
Path(__file__).with_name('toml-types.json').write_text(json.dumps(dict(source='stdlib tomllib, Python 3.14; consumidor original 8a3ce81c1',command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_toml_types.py',cases=cases),indent=2)+'\n')
print(len(cases))
