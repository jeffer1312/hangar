"""Congela resultados executados do Python em árvores temporárias isoladas."""
import dataclasses
import json
from pathlib import Path
import subprocess
import tempfile
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[4]))

from app.config_sync_paths import Roots, canonicalize, resolve, marked_paths, fix_programs
from app.config_sync_describe import describe


def main():
    roots = [Roots('/opt/app', '/users/ana/.claude', '/users/ana/.codex', '/users/ana'),
             Roots('C:\\App', 'C:\\Users\\bia\\.claude', 'C:\\Users\\bia\\.codex', 'C:\\Users\\bia')]
    texts = ['', 'echo ${HOME} {HOME}', '⟦HOME⟧/a:⟦HOME⟧/b',
             '⟦HOME⟧\\x.py "⟦CLAUDE⟧/hooks/a b.sh"',
             "'⟦HOME⟧/a b.sh' ⟦CLAUDE⟧/x.py '⟦HOME⟧/a b.sh'", 'ação 𝄞 fim']
    for root in roots:
        for base in dataclasses.asdict(root).values():
            for end in ['', '/a', '\\a', '2/a', ' x', "'", ';', ':x', ')', '&']:
                texts.append(base + end)
        texts.append(root.home + '/.hangar/versions/v1/scripts/run')
    paths = []
    for root in roots:
        for value in texts:
            paths.append({'roots': dataclasses.asdict(root), 'input': value,
                          'canonical': canonicalize(value, root), 'resolved': resolve(value, root),
                          'marked': marked_paths(value)})
    commands = ['', 'rtk hook claude', 'if [ -f x ]; then FOO=1 sh x; fi',
                'FOO=1 node x.js', '"$PROJECT"/.venv/bin/python x.py',
                "'/missing/node' '/exists/s.js'", '/missing/python3 x.py',
                'C:\\missing\\node.exe a', '/missing/unknown x',
                'echo /missing/node', '/exists/node x', '/missing/Node.EXE x',
                'exec /missing/python x', '~/.bin/x a', '$CMD a',
                '/missing/python && /missing/node x']
    programs = []
    for available in [{}, {'node': '/usr/bin/node', 'python': 'C:/Program Files/Python/python.exe'}]:
        for command in commands:
            output, missing = fix_programs(command, which=available.get,
                                          exists=lambda p: p.startswith('/exists/'))
            programs.append({'input': command, 'available': available,
                             'output': output, 'missing': missing})
    descriptions = []
    samples = [('a.md', '# Título\ntexto'), ('a.md', '---\ndescription: >\n  Texto longo\n  em duas linhas\nname: a\n---\n# Outro'),
               ('a.py', '#!/bin/python\n# coding: utf-8\n# Primeira linha\n# Segunda linha\nprint(1)'),
               ('a.rs', '// Comentário\nfn main() {}'), ('a.py', '"""Descrição\n\nOutro parágrafo"""'),
               ('a.json', '{"x": 1}'), ('a.md', 'a' * 230), ('a.md', 'a' * 90 + '. ' + 'b' * 200),
               ('a.sh', 'set -e\n# Descrição\necho fim'), ('a.md', '\0texto')]
    with tempfile.TemporaryDirectory(prefix='config-sync-contract-') as folder:
        for name, content in samples:
            path = Path(folder) / name
            path.write_text(content, encoding='utf-8')
            descriptions.append({'name': name, 'content': content, 'output': describe(path)})
    result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'command': 'cd backend && uv run python tests/fixtures/contract/config_sync/generate.py',
              'paths': paths, 'programs': programs, 'descriptions': descriptions}
    Path(__file__).with_name('helpers.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
