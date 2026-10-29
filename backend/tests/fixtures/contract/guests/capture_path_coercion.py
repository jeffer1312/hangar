"""Captura normalização lexical do pathlib; Windows puro não prova I/O na VM."""
import json
import subprocess
from pathlib import Path,PurePosixPath,PureWindowsPath
HERE=Path(__file__).resolve().parent
values=['','.','..','./vault.json','folder//vault.json','folder/../vault.json','~/.hangar/vault.json',
        '/','//','///','//server/share','///server/share','/a/./b//','a/.','a/..','a\\b',
        'C:','c:folder','C:/','C:/folder/./../vault.json','./C:folder','1:folder','é:folder',
        '\\folder','\\\\server','\\\\server\\share','\\\\server\\share\\folder\\.\\file',
        '\\\\?\\C:\\folder\\.\\file','\\\\?\\UNC\\server\\share','\\\\?\\UNC\\server\\share\\folder',
        '\\\\.\\device','a\x00b','a/ /b','a/.../b','././','//?/unc/server/share/./x']
rows=[{'input':v,'posix':str(PurePosixPath(v)),'windows':str(PureWindowsPath(v))} for v in values]
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_path_coercion.py',
        'source':'pathlib.PurePosixPath/PureWindowsPath; normalização lexical, sem I/O','rows':rows}
(HERE/'path-coercion.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
print(f'{len(rows)} entradas nas duas regras lexicais')
