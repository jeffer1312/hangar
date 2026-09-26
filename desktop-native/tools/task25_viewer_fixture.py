"""Fixture SINTÉTICA da Task 25 (visor de imagem): a da Task 15d com uma sessão a mais, cuja resposta cita quatro
imagens. Nenhuma sessão, pasta ou arquivo daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 15d sem o bloco que sobe o servidor (o arquivo dela não muda), e acrescenta a rota
`GET /api/sessions/<nome>/file?path=` só para as quatro imagens:

- paisagem.png 2400×1500 (maior que o palco: abre reduzida e o zoom tem para onde ir);
- pequena.png 640×400 (menor que o palco: abre no tamanho dela, sem ampliar);
- retrato.png 1000×1600;
- quebrada.png: bytes que não são imagem (a falha de leitura).

Linhas verticais finas a cada 40 px e horizontais a cada 200 px: borram na redução e ficam nítidas com zoom.
`GET /control/t25?delay=<s>` atrasa as imagens (estado "carregando"); `GET /control/t25log` lista os pedidos.
"""
import pathlib
import struct
import threading
import time
import zlib
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task15_cards_fixture.py").read_text(encoding="utf-8")
T15NS = {"__name__": "task15_base", "__file__": str(HERE / "task15_cards_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task15_cards_fixture.py", "exec"), T15NS)

BASE = T15NS["BASE"]
SESSIONS, bump, info, state, msg = (BASE[k] for k in ("SESSIONS", "bump", "info", "state", "msg"))

NAME = "sintetica-imagens"
DIR = "/sintetica/projetos/hangar-sintetico/capturas"
IMAGES = {"paisagem.png": (2400, 1500, (40, 90, 160)), "pequena.png": (640, 400, (170, 80, 60)),
          "retrato.png": (1000, 1600, (60, 140, 90)), "quebrada.png": None}
LOCK = threading.Lock()
T25 = {"delay": 0.0}
LOG = []
CACHE = {}


def png(width, height, color):
    r, g, b = color
    rows = []
    for y in range(height):
        shade = y * 60 // height
        line = bytearray(bytes((min(255, r + shade), min(255, g + shade), min(255, b + shade))) * width)
        for x in range(0, width, 40):
            line[x * 3:x * 3 + 3] = b"\xff\xff\xff"
        if y % 200 == 0:
            line[:] = b"\xe0\xe0\xe0" * width
        rows.append(b"\x00" + bytes(line))
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"".join(rows), 1)) + chunk(b"IEND", b""))


def image(name):
    with LOCK:
        if name not in CACHE:
            spec = IMAGES[name]
            CACHE[name] = png(*spec) if spec else b"isto nao e uma imagem (sintetico)\n" * 20
        return CACHE[name]


NOW = time.time()
ANSWER = "Gerei as quatro capturas da tela (sintético):\n\n" + "\n".join(f"- {DIR}/{n}" for n in IMAGES) + "\n\nFim da resposta."
EVENTS = [
    msg("user_msg", "m0", "Tira as capturas da tela de configuração e me mostra.", ts=NOW - 300),
    msg("assistant_msg", "m1", ANSWER, ts=NOW - 280),
]


class Handler(T15NS["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t25":
            with LOCK:
                for value in parse_qs(url.query).get("delay", []):
                    T25["delay"] = float(value)
                current = dict(T25)
            self.send_json(current)
            return
        if url.path == "/control/t25log":
            with LOCK:
                self.send_json({"log": list(LOG)})
            return
        parts = url.path.split("/")
        if len(parts) == 5 and parts[:3] == ["", "api", "sessions"] and parts[3] == NAME and parts[4] == "file":
            if not self.authorized():
                return
            path = parse_qs(url.query).get("path", [""])[0]
            name = path.rsplit("/", 1)[-1]
            with LOCK:
                LOG.append(f"{time.strftime('%H:%M:%S')} {name}")
                delay = T25["delay"]
            time.sleep(delay)
            if not path.startswith(DIR + "/") or name not in IMAGES:
                self.send_json({"detail": "not found"}, 404)
                return
            data = image(name)
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        super().do_GET()


data = info(NAME, "claude", state="idle", branch="main")
data["cwd"] = "/sintetica/projetos/hangar-sintetico"
SESSIONS[NAME] = {"info": data, "state": state("idle"), "events": [dict(e) for e in EVENTS], "stats": None, "modes": []}
bump()

server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
