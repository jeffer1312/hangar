"""Captura o limite original com arquivos e TOML reais, sem a conta pessoal."""
import json
from pathlib import Path
import tempfile

loader = Path(__file__).with_name('generate_instructions.py')
scope = {'__file__': str(loader)}
exec(compile(loader.read_text().split('cases=[]', 1)[0], str(loader), 'exec'), scope)
original = scope['ns']
cases = []
for name, value in [('missing', None), ('small', '32768'), ('large', '2097152'),
                    ('wide', '9' * 100), ('zero', '0'), ('negative', '-1'),
                    ('boolean', 'true'), ('float', '1.0'), ('string', '"2097152"'),
                    ('source-size', None), ('missing-source', None)]:
    with tempfile.TemporaryDirectory() as raw:
        home = Path(raw)
        if value is not None:
            (home / 'config.toml').write_text('project_doc_max_bytes = ' + value + '\n')
        size = 0
        if name in ['source-size', 'missing-source']:
            records = home / '.hangar-instrucoes'
            records.mkdir()
            source = home / 'source.md'
            if name == 'source-size':
                size = 1048576
                source.write_bytes(b'x' * size)
            (records / 'source.json').write_text(json.dumps({'fonte': str(source)}))
        case = dict(name=name, config=value, source_size=size)
        try:
            case['result'] = str(original['limite_instrucoes'](home))
        except Exception as error:
            case['error'] = type(error).__name__
            case['message'] = str(error).replace(raw, '$ROOT')
        cases.append(case)
Path(__file__).with_name('instruction-limits.json').write_text(json.dumps(dict(
    source_commit=scope['COMMIT'],
    command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_instruction_limits.py',
    cases=cases), ensure_ascii=True, indent=2) + '\n')
print(len(cases))
