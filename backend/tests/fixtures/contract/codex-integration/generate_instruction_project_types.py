"""Captura os tipos aceitos e recusados na seleção original de projetos e registros."""
import json
from pathlib import Path
import tempfile

loader = Path(__file__).with_name('generate_instructions.py')
scope = {'__file__': str(loader)}
exec(compile(loader.read_text().split('cases=[]', 1)[0], str(loader), 'exec'), scope)
original = scope['ns']
cases = []
variants = [('table', None), ('array', None), ('empty-array', []), ('integer', 1),
            ('boolean', True), ('float', 1.5), ('string', 'relative'), ('empty-string', ''),
            ('array-integer', [1])]
variants += [('record-' + name, value) for name, value in [
    ('integer', 1), ('zero', 0), ('boolean', True), ('null', None),
    ('empty-list', []), ('list', ['relative']), ('empty-object', {}), ('object', {'x': 1})]]
for name, value in variants:
    with tempfile.TemporaryDirectory() as raw:
        home = Path(raw)
        codex = home / '.codex'
        project = home / 'project'
        for directory in [codex, project, home / '.claude']:
            directory.mkdir()
        (home / '.claude/CLAUDE.md').write_text('GLOBAL\n', encoding='utf-8')
        (project / 'CLAUDE.md').write_text('PROJETO\n', encoding='utf-8')
        record = None
        if name.startswith('record-'):
            store = codex / '.hangar-instrucoes'
            store.mkdir()
            record = {'alvo': value}
            (store / 'seed.json').write_text(json.dumps(record), encoding='utf-8')
            config = None
        elif name == 'table':
            config = '[projects.' + json.dumps(str(project)) + ']\n'
        elif name == 'array':
            config = 'projects = ' + json.dumps([str(project)]) + '\n'
        else:
            config = 'projects = ' + json.dumps(value) + '\n'
        if config is not None:
            (codex / 'config.toml').write_text(config, encoding='utf-8')
        try:
            original['preparar_instrucoes'](home, codex, force=True)
            result = {'ok': True}
        except Exception as error:
            result = {'ok': False, 'message': str(error)}
        case = dict(name=name, config=config, record=record, result=result,
                    global_alias=(codex / 'AGENTS.override.md').is_symlink(),
                    project_alias=(project / 'AGENTS.override.md').is_symlink(),
                    source=(project / 'CLAUDE.md').read_text(encoding='utf-8'))
        cases.append(json.loads(json.dumps(case).replace(raw, '$ROOT')))
Path(__file__).with_name('instruction-project-types.json').write_text(json.dumps(dict(
    source_commit=scope['COMMIT'], command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_instruction_project_types.py',
    comparison='Resultado, mensagem da exceção e efeitos; classe da exceção Python não faz parte do protocolo público.',
    cases=cases), ensure_ascii=True, indent=2) + '\n')
print(len(cases))
