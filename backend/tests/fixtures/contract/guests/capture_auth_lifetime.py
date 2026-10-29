"""Confere a duração do contexto enviado aos filhos; a biblioteca FFI é isolada."""
import ctypes
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
class Function:
    def __init__(self,fn=lambda *args:0):self.fn=fn
    def __call__(self,*args):return self.fn(*args)
class Library:
    def __init__(self):
        def opened(data,length,pointer):
            ctypes.cast(pointer,ctypes.POINTER(ctypes.c_void_p))[0]=ctypes.c_void_p(1)
            return 0
        self.hangar_auth_abi_v1=Function(lambda:1)
        self.hangar_auth_open_v1=Function(opened)
        self.hangar_auth_evaluate_v1=Function()
        self.hangar_auth_verify_receipt_v1=Function()
        self.hangar_auth_close_v1=Function()
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch)
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from app import shared_auth
    with patch.object(shared_auth.rust_bins,'auth_pair',lambda _: (Path(scratch)/'fixture.so','fixture-hash')),patch.object(shared_auth.ctypes,'CDLL',lambda _:Library()):
        first=shared_auth.activate(Path(scratch)/'server','fixture-owner',0)
        child1=json.loads(first.child_env(Path(scratch)/'server')['HANGAR_AUTH_CONTEXT'])
        same=shared_auth.activate(Path(scratch)/'server','fixture-owner',8767)
        child2=json.loads(same.child_env(Path(scratch)/'server')['HANGAR_AUTH_CONTEXT'])
        second=shared_auth.Context(Path(scratch)/'server','fixture-owner',8767)
        child3=json.loads(second.child_env(Path(scratch)/'server')['HANGAR_AUTH_CONTEXT'])
        result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
                'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_auth_lifetime.py',
                'ffi':'isolada; executa Context/activate/child_env originais sem biblioteca nem servidor',
                'same_context':same is first,'upstream_updated':same.upstream_port==8767,
                'same_key_between_children':child1['receipt_key']==child2['receipt_key'],
                'same_epoch_between_children':child1['parent_epoch']==child2['parent_epoch'],
                'new_context_changes_key':child1['receipt_key']!=child3['receipt_key'],
                'new_context_changes_epoch':child1['parent_epoch']!=child3['parent_epoch']}
        first.close();second.close()
    (HERE/'auth-lifetime.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print('Duração do contexto capturada sem gravar chaves')
