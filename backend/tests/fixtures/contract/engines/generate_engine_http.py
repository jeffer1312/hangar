"""Captura a rota original com dependências isoladas e sem backend ou CLI reais."""
import ast
import asyncio
import json
import re
from pathlib import Path
import subprocess
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[5]
source = ROOT / 'backend/app/api.py'
node = next(n for n in ast.parse(source.read_text()).body if isinstance(n, ast.AsyncFunctionDef) and n.name == 'engine_model_set')
node.decorator_list = []
node.args.args[1].annotation = None
catalog_node = next(n for n in ast.parse((ROOT / 'backend/app/engines.py').read_text()).body if isinstance(n,ast.FunctionDef) and n.name == 'catalog_model')
catalog_namespace = {'re': re}
exec(compile(ast.Module(body=[catalog_node],type_ignores=[]),'original-catalog-model','exec'),catalog_namespace)

class HttpError(Exception):
    def __init__(self, status_code, detail):
        self.status_code, self.detail = status_code, detail

class PickerError(Exception):
    status, detail = 409, {'reason': 'picker'}

class NotTyped(PickerError):
    pass

async def scenario(name, *, found=True, engine='motor', fixed=False, model='k3', catalog=True, fast=False, supports=True, headless=False, effort=None, applied=True, failure=None):
    calls = []
    async def info(_):
        return SimpleNamespace(engine=engine, engine_account='fixed' if fixed else None) if found else None
    async def models(_, fresco):
        calls.append(['catalog', fresco])
        return [{'id': 'k3'}] if catalog else []
    async def set_model(_, model, effort):
        calls.append(['headless', model, effort])
        if failure: raise RuntimeError('fixture-error')
        return applied
    def terminal_model(_, model):
        calls.append(['terminal', model])
        if failure == 'not_typed': raise NotTyped()
        if failure == 'picker': raise PickerError()
        if failure == 'invalid': raise ValueError('fixture-invalid')
        return {'ok': True, 'result': 'Set model to k3'}
    def terminal_effort(*args):
        calls.append(['effort', *args[1:]])
        if failure == 'effort': raise ValueError('fixture-effort')
    ns = dict(asyncio=asyncio, HTTPException=HttpError, PickerError=PickerError,
        TerminalInput=SimpleNamespace(NaoDigitou=NotTyped),
        _recusa_se_painel_aberto=lambda _: calls.append(['panel']), _cached_info=info,
        registry_mod=SimpleNamespace(SessionOpenNeedsRust=RuntimeError),
        _engine_models=models, engines=SimpleNamespace(catalog_model=catalog_namespace['catalog_model']),
        _engine_fast_selection=lambda _: (None, 'priority' if fast else None),
        cliproxy=SimpleNamespace(supports_fast=lambda *_: supports),
        _headless=lambda _: headless, CLAUDE_HEADLESS='headless',
        get_adapter=lambda _: SimpleNamespace(set_model=set_model),
        _log=SimpleNamespace(exception=lambda *_args, **_kwargs: None),
        _session_config_dir=lambda _: '/fixture/account',
        default_model=SimpleNamespace(snapshot=lambda _: calls.append(['snapshot']) or 'before',
            restore_quando_aterrissar=lambda *_: calls.append(['restore'])),
        terminal=SimpleNamespace(set_engine_model=terminal_model, set_model_effort=terminal_effort),
        erro=lambda code, msg, **params: {'code': code, 'msg': msg, 'params': params})
    exec(compile(ast.Module(body=[node], type_ignores=[]), str(source), 'exec'), ns)
    try:
        result = {'status': 200, 'body': await ns['engine_model_set']('fixture', SimpleNamespace(model=model, effort=effort))}
    except HttpError as error:
        result = {'status': error.status_code, 'detail': error.detail}
    return {'name': name, 'result': result, 'calls': calls}

async def main():
    cases = []
    for name, options in [
        ('missing', {'found': False}), ('not_engine', {'engine': None}),
        ('outside_catalog', {'catalog': False}), ('fast_refused', {'fast': True, 'supports': False}),
        ('headless', {'headless': True}), ('headless_effort_deferred', {'headless': True, 'effort': 'high', 'applied': False}),
        ('headless_failed', {'headless': True, 'failure': 'native'}),
        ('terminal', {}), ('terminal_trim', {'model': ' k3 '}),
        ('terminal_not_typed', {'failure': 'not_typed'}), ('terminal_picker', {'failure': 'picker'}),
        ('terminal_invalid', {'failure': 'invalid'}), ('terminal_effort_failed', {'effort': 'high', 'failure': 'effort'}),
    ]:
        cases.append(await scenario(name, **options))
    payload = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
        'command': 'python backend/tests/fixtures/contract/engines/generate_engine_http.py', 'cases': cases}
    Path(__file__).with_name('engine-http.json').write_text(json.dumps(payload, ensure_ascii=False, indent=2) + '\n')

asyncio.run(main())
