"""Compara HTTP de servidores estáticos isolados, sem iniciar o backend do Hangar."""
import ast
import json
import os
import queue
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[5]


def python_server():
    import uvicorn
    from fastapi import FastAPI
    from fastapi.staticfiles import StaticFiles
    from starlette.responses import Response
    source = subprocess.check_output(['git', 'show', 'ba2db1021:backend/app/api.py'], cwd=ROOT, text=True)
    node = next(n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == '_UIStatic')
    namespace = dict(StaticFiles=StaticFiles, Path=Path, Response=Response)
    exec(compile(ast.Module(body=[node], type_ignores=[]), '<referência>', 'exec'), namespace)
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root/'assets').mkdir()
        (root/'index.html').write_text('<p>tela</p>')
        (root/'assets/app.js').write_text('const value = 1;\n' * 4096)
        app = FastAPI()
        app.mount('/', namespace['_UIStatic'](directory=root, html=True))
        sock = socket.socket()
        sock.bind(('127.0.0.1', 0))
        sock.listen()
        print(f'STATIC_PROBE http://127.0.0.1:{sock.getsockname()[1]}', flush=True)
        uvicorn.Server(uvicorn.Config(app, log_level='error')).run(sockets=[sock])
        sock.close()


def measure(command):
    import httpx2 as httpx
    import psutil
    with tempfile.TemporaryDirectory() as home:
        process = subprocess.Popen(command, cwd=ROOT, env={**os.environ, 'HOME':home, 'USERPROFILE':home},
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        lines = queue.Queue()
        def read():
            for line in process.stdout:
                lines.put(line)
            lines.put(None)
        reader = threading.Thread(target=read)
        reader.start()
        try:
            while True:
                line = lines.get(timeout=90)
                if line is None:
                    raise RuntimeError('Sonda encerrou sem publicar endereço')
                if line.startswith('STATIC_PROBE '):
                    base = line.split()[1]
                    break
            # O socket publicado já está em listen; a chamada espera a primeira resposta.
            timings = {}
            proc = psutil.Process(process.pid)
            with httpx.Client(base_url=base, headers={'accept-encoding':'identity'}, timeout=10) as client:
                client.get('/').raise_for_status()
                before = proc.cpu_times()
                for path in ('/', '/assets/app.js'):
                    samples = []
                    for _ in range(100):
                        start = time.perf_counter_ns()
                        response = client.get(path)
                        response.raise_for_status()
                        samples.append((time.perf_counter_ns()-start)/1_000_000)
                    samples.sort()
                    timings[path] = {'p50_ms':samples[49], 'p95_ms':samples[94], 'bytes':len(response.content)}
                after = proc.cpu_times()
            return {'routes':timings, 'cpu_seconds':after.user+after.system-before.user-before.system,
                    'rss_bytes':proc.memory_info().rss}
        finally:
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise RuntimeError('Sonda não encerrou com SIGINT')
            reader.join(timeout=5)
            assert not reader.is_alive()
            process.stdout.close()
            allowed = (0, -signal.SIGINT) if command[-1] == "python" else (0,)
            if process.returncode not in allowed:
                raise RuntimeError(f"Sonda encerrou com código {process.returncode}")


if __name__ == '__main__':
    if sys.argv[1:] == ['python']:
        python_server()
    else:
        print(json.dumps({'source_commit':'ba2db1021','samples_per_route':100,'encoding':'identity',
                          'python':measure([sys.executable, str(Path(__file__).resolve()), 'python']),
                          'rust':measure([str(Path(sys.argv[1]).resolve()), 'web_ui::static_http_probe', '--exact', '--ignored', '--nocapture'])}, indent=2))
