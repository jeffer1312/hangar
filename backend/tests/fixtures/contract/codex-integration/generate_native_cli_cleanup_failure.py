"""Captura a falha de limpeza, sua precedência e a nova tentativa do cliente original."""
import ast
import asyncio
import logging
import json
from pathlib import Path

source = Path(__file__).with_name('generate_native_cli.py')
tree = ast.parse(source.read_text())
tree.body = [node for node in tree.body if not (
    isinstance(node, ast.Expr) and isinstance(node.value, ast.Call)
    and isinstance(node.value.func, ast.Attribute) and node.value.func.attr == 'run')]
namespace = {'__file__': str(source)}
exec(compile(tree, str(source), 'exec'), namespace)
original = namespace['namespace']['CodexNativo']
error_type = namespace['namespace']['CodexNativoErro']


class DeferredCleanup(original):
    attempts = 0
    retained_on_close = []

    async def close(self):
        self.retained_on_close.append(bool(self._operations))
        await super().close()

    def _remove_work_dir(self, path):
        self.attempts += 1
        if self.attempts == 1:
            raise error_type('Não foi possível remover a pasta temporária do Codex.')
        return original._remove_work_dir(path)


class Records(logging.Handler):
    def __init__(self):
        super().__init__()
        self.messages = []

    def emit(self, record):
        message = record.getMessage()
        if message.startswith('limpeza da pasta temporária'):
            self.messages.append(message)


async def main():
    namespace['namespace']['CodexNativo'] = DeferredCleanup
    logger = namespace['namespace']['_log']
    cases = []
    for scenario in ('exit', 'ok'):
        handler = Records()
        logger.addHandler(handler)
        try:
            row = await namespace['capture'](scenario, 'cli', scenario)
        finally:
            logger.removeHandler(handler)
        cases.append({
            'scenario': scenario, 'result': row['result'],
            'operations_empty': row['operations_empty'],
            'operations_retained': DeferredCleanup.retained_on_close[-1],
            'directories_removed': row['directories_removed'],
            'warnings': handler.messages,
        })
    Path(__file__).with_name('native-cli-cleanup-failure.json').write_text(json.dumps({
        'source_commit': namespace['COMMIT'],
        'command': 'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_native_cli_cleanup_failure.py',
        'boundary': 'CLI e nova tentativa originais; primeira remoção injeta CodexNativoErro, a segunda remove a pasta real.',
        'cases': cases,
    }, ensure_ascii=True, indent=2) + '\n')
    print(len(cases))


asyncio.run(main())
