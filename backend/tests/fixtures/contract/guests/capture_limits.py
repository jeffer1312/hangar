"""Executa o middleware original extraído da AST, sem importar a aplicação inteira."""
import ast
import asyncio
import json
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[3] / 'app' / 'api.py'
nodes = ast.parse(SOURCE.read_text()).body
selected = [n for n in nodes if isinstance(n, ast.ClassDef) and n.name in
            {'_BodyTooLarge', '_BodySizeLimitMiddleware'}]
namespace = {}
exec(compile(ast.Module(body=selected, type_ignores=[]), str(SOURCE), 'exec'), namespace)

async def capture():
    rows = []
    for kind in ('content_length', 'chunked'):
        sent = []
        async def receive():
            return {'type':'http.request', 'body':b'x' * 9, 'more_body':False}
        async def send(message):
            sent.append(message)
        async def app(scope, receive, send):
            await receive()
            await send({'type':'http.response.start', 'status':200})
            await send({'type':'http.response.body', 'body':b'ok'})
        middleware = namespace['_BodySizeLimitMiddleware'](app, 8)
        scope = {'type':'http', 'headers':[(b'content-length', b'9')] if kind == 'content_length' else []}
        await middleware(scope, receive, send)
        rows.append({'name':kind, 'limit':8, 'input_bytes':9, 'status':sent[0]['status'],
                     'content_type':sent[0]['headers'][0][1].decode(), 'response':sent[1]['body'].decode()})
    result = {'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
              'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_limits.py',
              'rows':rows}
    (HERE/'limits.json').write_text(json.dumps(result,indent=2)+'\n')
    print(f'{len(rows)} respostas do limite Python capturadas')
asyncio.run(capture())
