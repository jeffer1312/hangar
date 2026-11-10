"""Fixture SINTÉTICA da página da conversa: a da Task 22 com uma sessão cuja conversa traz um cartão de página
(`html_render`). Nenhuma sessão, pasta ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 22 sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.

Sessão daqui:
  sintetica-pagina  uma chamada de html_render com o resultado `hangar_page`; a página é servida em
                    GET /api/sessions/sintetica-pagina/pages/pg1?raw=1.
A página tem texto pequeno, uma faixa de linhas de 1 px (redução em razão quebrada aparece como faixas claras e escuras;
na metade exata fica cinza uniforme) e um contador que anda a cada segundo, para ver que os quadros continuam chegando.
"""
import pathlib
from urllib.parse import unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task22_perf_fixture.py").read_text(encoding="utf-8")
T22NS = {"__name__": "task22_base", "__file__": str(HERE / "task22_perf_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task22_perf_fixture.py", "exec"), T22NS)

BASE = T22NS["BASE"]
SESSIONS, bump, info, state, msg, call, result = (BASE[k] for k in ("SESSIONS", "bump", "info", "state", "msg", "call", "result"))

PAGE_NAME, PAGE_ID = "sintetica-pagina", "pg1"
PAGE = """<!doctype html>
<html lang="pt-BR"><head><meta charset="utf-8"><title>Página sintética</title>
<style>
  body { margin: 0; padding: 16px; font: 13px/1.45 system-ui, sans-serif; background: #fff; color: #111; }
  h1 { font-size: 16px; margin: 0 0 8px; }
  small { font-size: 10px; }
  .lines { height: 48px; margin: 10px 0; background: repeating-linear-gradient(90deg, #000 0 1px, #fff 1px 2px); }
  .grid { height: 48px; margin: 10px 0; background:
      repeating-linear-gradient(90deg, #000 0 1px, transparent 1px 2px),
      repeating-linear-gradient(0deg, #000 0 1px, #fff 1px 2px); }
  #tick { font: 600 20px ui-monospace, monospace; }
</style></head><body>
<h1>Página sintética da conversa</h1>
<p>Texto de 13 px: ação, função, informação e coração. O rápido cão marrom salta sobre a raposa preguiçosa 0123456789.</p>
<p><small>Texto de 10 px: ação, função, informação e coração. O rápido cão marrom salta sobre a raposa 0123456789.</small></p>
<div class="lines"></div>
<div class="grid"></div>
<p>Contador: <span id="tick">0</span></p>
<script>
  let n = 0;
  setInterval(() => { document.getElementById('tick').textContent = String(++n); }, 1000);
</script>
</body></html>
"""


def add_page():
    data = info(PAGE_NAME, "claude", state="idle", branch="main")
    data["cwd"] = "/sintetica/projetos/hangar-sintetico"
    card = '{"hangar_page":{"id":"%s","title":"Página sintética","height":330,"own_theme":true}}' % PAGE_ID
    events = [msg("user_msg", "p-u1", "Mostre a página."),
              call("p-c1", "tp1", "mcp__hangar__html_render", {"title": "Página sintética"}),
              result("p-r1", "tp1", card),
              msg("assistant_msg", "p-a1", "A página está acima.")]
    SESSIONS[PAGE_NAME] = {"info": data, "state": state("idle"), "events": events, "stats": None, "modes": []}
    bump()


BASE_FILL = T22NS["T14NS"]["fill"]


def fill():
    BASE_FILL()
    add_page()


T22NS["T14NS"]["fill"] = fill
add_page()


class Handler(T22NS["Handler"]):
    def do_GET(self):
        parts = [unquote(p) for p in urlparse(self.path).path.strip("/").split("/")]
        if parts[:2] == ["api", "sessions"] and parts[3:5] == ["pages", PAGE_ID] and len(parts) in (5, 6):
            if not self.authorized():
                return
            if len(parts) == 6:
                # Sem imagem guardada: o cartão desenha a página ao vivo.
                self.send_json({"detail": {"code": "erro_pagina_sem_imagem", "params": {}, "msg": "sem imagem"}}, 404)
                return
            data = PAGE.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        super().do_GET()


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
