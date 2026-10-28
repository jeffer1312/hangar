"""O Connect leva arquivos web ao Rust, conservando os cabeçalhos HTTP."""
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

from fastapi.testclient import TestClient
from app import account_bridge, runtime_coordinator
from app.api import app


def test_static_without_rust_has_an_explicit_error(monkeypatch):
    monkeypatch.setattr(runtime_coordinator, 'current', lambda: SimpleNamespace(mode='python'))
    response = TestClient(app).get('/')
    assert response.status_code == 503
    assert response.json()['detail']['code'] == 'web_ui_needs_rust_server'


def test_static_transport_preserves_path_query_range_and_headers(monkeypatch):
    seen = []
    class Rust(BaseHTTPRequestHandler):
        def do_GET(self):
            seen.append((self.path, self.headers.get('x-hangar-path'), self.headers.get('range')))
            self.send_response(206)
            for key, value in {'content-length':'3','content-type':'text/javascript; charset=utf-8',
                               'content-range':'bytes 2-4/10','cache-control':'public, max-age=31536000, immutable',
                               'etag':'"fixture"'}.items():
                self.send_header(key, value)
            self.end_headers()
            self.wfile.write(b'234')
        def log_message(self, *args):
            pass
    server = ThreadingHTTPServer(('127.0.0.1',0),Rust)
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    monkeypatch.setattr(runtime_coordinator,'current',lambda:SimpleNamespace(mode='rust'))
    monkeypatch.setattr(account_bridge,'_preparation_transport',(f'127.0.0.1:{server.server_port}','fixture-secret'))
    try:
        response=TestClient(app).get('/assets/app.js?x=1',headers={'range':'bytes=2-4'})
        assert response.status_code == 206
        assert response.content == b'234'
        assert response.headers['content-range'] == 'bytes 2-4/10'
        assert response.headers['etag'] == '"fixture"'
        assert seen == [('/__hangar_server/forwarded?x=1','/assets/app.js','bytes=2-4')]
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        assert not thread.is_alive()
