"""Caracteriza propriedade e falhas de leitura sem processos nem sessões reais."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch)
    from app import guest_users as store
    store._path_override = root / 'guests.json'
    guests = []
    for i, (name, sees, owner) in enumerate([('Ana', False, False), ('Bia', True, True)]):
        with patch('secrets.token_hex', lambda n: str(i) * (n * 2)), patch('secrets.token_urlsafe', lambda n: 'token-' + str(i)):
            guests.append(store.create(name, scratch, sees, owner)[0])
    initial = json.loads(store._path_override.read_text())
    matrix_disk = initial | {'sessions':{'s0':{'guest':guests[0].id,'life':None},
                                       's1':{'guest':guests[1].id,'life':None}}}
    store._path_override.write_text(json.dumps(matrix_disk))
    store._reset()
    with patch('app.guest_users.session_life', lambda _: None):
        matrix = [{'viewer':g.id if g else None,
                   'names':store.filter_visible(g, ['s0','s1','s2'], lambda n:n)} for g in [None,*guests]]
    store._path_override.write_text(json.dumps(initial))
    store._reset()
    rows = []
    def record(label, life):
        store._life_cache.clear()
        with patch('app.guest_users.session_life', lambda _: life):
            before = json.loads(store._path_override.read_text())
            owner = store.owner_of('session')
            row = {'name':label, 'life':life, 'owner':owner.id if owner else None,
                   'visible':[store.visible_to(g, 'session') for g in [None, *guests]],
                   'before':before, 'disk':json.loads(store._path_override.read_text())}
            rows.append(row)
    with patch('app.guest_users.session_life', lambda _: 'k:first'):
        store.claim('session', guests[0].id)
    record('same_life', 'k:first')
    record('unknown_life', None)
    record('recreated_name', 'k:second')
    store.set_life('session', 'k:second')
    record('mode_change', 'k:second')
    store.set_life('session', None)
    record('null_mode_change', 'k:second')
    with patch('app.guest_users.session_life', lambda _: None), patch('app.guest_users.time.sleep'):
        store.claim('session', guests[1].id)
    record('adopt_unknown_life', 't:123')
    store.delete(guests[1].id)
    record('deleted_owner', 't:123')
    adoption = initial | {'sessions':{'session':{'guest':guests[0].id,'life':None}}}
    store._path_override.write_text(json.dumps(adoption)); store._reset()
    with patch('app.guest_users._save', side_effect=OSError('fixture failure')):
        store._life_cache.clear()
        with patch('app.guest_users.session_life',lambda _: 'k:first'):
            first = store.owner_of('session')
        store._life_cache.clear()
        with patch('app.guest_users.session_life',lambda _: 'k:second'):
            second = store.owner_of('session')
    failed_adoption = {'before':adoption,'first':first.id if first else None,
                       'second':second.id if second else None,'disk':json.loads(store._path_override.read_text())}
    result = {'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
              'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_store.py',
              'failed_adoption':failed_adoption, 'initial':initial, 'matrix_disk':matrix_disk, 'list_matrix':matrix, 'rows':rows}
    raw = json.dumps(result, ensure_ascii=False, indent=2).replace(scratch, '⟦ROOT⟧')
    (HERE / 'store.json').write_text(raw + '\n', encoding='utf-8')
    print(f'{len(rows)} estados de propriedade Python capturados')
