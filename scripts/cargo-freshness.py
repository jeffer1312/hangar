"""Invalida fingerprints locais quando outra árvore assume o target, sob a trava do wrapper."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile


def prepare(metadata: dict, target: Path) -> int:
    workspace = Path(metadata['workspace_root']).resolve()
    original_target = Path(metadata['target_directory']).resolve()
    configured_build = Path(metadata.get('build_directory', original_target)).resolve()
    build = target.resolve() if configured_build == original_target else configured_build
    names = {package['name'] for package in metadata['packages'] if package.get('source') is None}
    # Dependências locais excluídas do workspace também compartilham o cache do Cargo.
    names.update(dependency['name'] for package in metadata['packages']
                 for dependency in package.get('dependencies', []) if dependency.get('path'))
    state = {'workspace': str(workspace), 'packages': sorted(names)}
    build.mkdir(parents=True, exist_ok=True)
    stamp = build / '.hangar-cargo-workspace.json'
    try:
        previous = json.loads(stamp.read_text())
    except (FileNotFoundError, ValueError):
        previous = None
    if previous == state:
        return 0
    removed = 0
    for fingerprints in [*build.glob('*/.fingerprint'), *build.glob('*/*/.fingerprint')]:
        for entry in fingerprints.iterdir():
            match = re.fullmatch(r'(.+)-[0-9a-f]+', entry.name)
            if match and match[1] in names:
                if entry.is_symlink():
                    entry.unlink()
                elif entry.is_dir():
                    shutil.rmtree(entry)
                else:
                    continue
                removed += 1
    # Grave antes da compilação: uma falha pode deixar artefatos parciais desta árvore.
    with tempfile.NamedTemporaryFile('w', dir=build, delete=False, encoding='utf-8') as stream:
        temporary = Path(stream.name)
        json.dump(state, stream)
    try:
        os.replace(temporary, stamp)
    finally:
        temporary.unlink(missing_ok=True)
    return removed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--metadata', type=Path, required=True)
    parser.add_argument('--target-dir', type=Path, required=True)
    args = parser.parse_args()
    count = prepare(json.loads(args.metadata.read_text()), args.target_dir)
    if count:
        print(f'cargo-pinned: {count} fingerprints locais invalidados na troca de árvore', file=sys.stderr)


if __name__ == '__main__':
    main()
