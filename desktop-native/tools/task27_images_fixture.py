"""Fixture SINTÉTICA da Task 27 (GIF, imagem do Read e `![..](..)`): a da Task 15d com uma sessão a mais. Nenhuma
sessão, pasta ou arquivo daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 15d sem o bloco que sobe o servidor (o arquivo dela não muda) e acrescenta:

- `GET /api/sessions/<nome>/file?path=` (com o token sintético) para animacao.gif (8 quadros, um quadrado que anda),
  lida.png (a imagem que o Read leu), grafico.png (citada em `![..](..)`) e quebrada.png (bytes que não são imagem);
- `GET /remote/<arquivo>` (sem token, no nome `localhost`): logo.png existe, sumiu.png dá 404 e infinito.png
  manda bytes sem `Content-Length` até o cliente desistir. É o "servidor de terceiros" das imagens remotas.

Cada pedido de imagem vai para `fixture-<porta>.log` na pasta corrente, com `authorization=presente|ausente` (nunca o
valor). `GET /control/t27log` devolve as mesmas linhas.
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
SESSIONS, bump, info, state, msg, call, result = (BASE[k] for k in ("SESSIONS", "bump", "info", "state", "msg", "call", "result"))

NAME = "sintetica-gif"
DIR = "/sintetica/projetos/hangar-sintetico/relatorio"
LOCK = threading.Lock()
LOG = []


def png(width, height, color):
    r, g, b = color
    rows = []
    for y in range(height):
        shade = y * 60 // height
        line = bytearray(bytes((min(255, r + shade), min(255, g + shade), min(255, b + shade))) * width)
        for x in range(0, width, 40):
            line[x * 3:x * 3 + 3] = b"\xff\xff\xff"
        rows.append(b"\x00" + bytes(line))
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"".join(rows), 1)) + chunk(b"IEND", b""))


def gif(width, height, frames, delay_cs):
    """GIF animado sem compressão real: códigos de 9 bits com `clear` a cada 200 símbolos (o dicionário não cresce)."""
    palette = bytes((30, 40, 70, 240, 150, 40, 235, 235, 235)) + bytes(3 * 253)
    out = bytearray(b"GIF89a" + struct.pack("<HHBBB", width, height, 0xF7, 0, 0) + palette)
    out += b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"
    side = height // 2
    for n in range(frames):
        x0 = n * (width - side) // max(1, frames - 1)
        pixels = [1 if x0 <= x < x0 + side and height // 4 <= y < height // 4 + side else (2 if x % 30 == 0 else 0)
                  for y in range(height) for x in range(width)]
        codes = []
        for i, p in enumerate(pixels):
            if i % 200 == 0:
                codes.append(256)
            codes.append(p)
        codes.append(257)
        bits = acc = 0
        data = bytearray()
        for code in codes:
            acc |= code << bits
            bits += 9
            while bits >= 8:
                data.append(acc & 0xFF)
                acc >>= 8
                bits -= 8
        if bits:
            data.append(acc & 0xFF)
        out += b"\x21\xf9\x04\x00" + struct.pack("<H", delay_cs) + b"\x00\x00"
        out += b"\x2c" + struct.pack("<HHHHB", 0, 0, width, height, 0) + b"\x08"
        for i in range(0, len(data), 255):
            block = data[i:i + 255]
            out += bytes((len(block),)) + block
        out += b"\x00"
    return bytes(out + b"\x3b")


FILES = {
    "animacao.gif": ("image/gif", gif(240, 160, 8, 25)),
    "lida.png": ("image/png", png(900, 560, (40, 120, 90))),
    "grafico.png": ("image/png", png(720, 420, (150, 70, 120))),
    "quebrada.png": ("image/png", b"isto nao e uma imagem (sintetico)\n" * 20),
}
REMOTE = {"logo.png": png(360, 240, (180, 120, 30))}


class Handler(T15NS["Handler"]):
    def note(self, where, name):
        auth = "presente" if self.headers.get("Authorization") else "ausente"
        line = f"{time.strftime('%H:%M:%S')} {where} {name} authorization={auth}"
        with LOCK:
            LOG.append(line)
            with open(f"fixture-{self.server.server_port}.log", "a", encoding="utf-8") as log:
                log.write(line + "\n")

    def send_bytes(self, kind, data):
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def endless(self):
        """Resposta sem `Content-Length` que não acaba: só para quando o cliente fecha a conexão."""
        self.send_response(200)
        self.send_header("Content-Type", "image/png")
        self.end_headers()
        chunk, sent = b"\x89PNG" + bytes(64 * 1024 - 4), 0
        try:
            while True:
                self.wfile.write(chunk)
                sent += len(chunk)
        except OSError:
            self.note("remoto", f"infinito.png encerrado pelo cliente depois de {sent // (1024 * 1024)} MiB")
        self.close_connection = True

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t27log":
            with LOCK:
                self.send_json({"log": list(LOG)})
            return
        if url.path.startswith("/remote/"):
            name = url.path.rsplit("/", 1)[-1]
            self.note("remoto", name)
            if name == "infinito.png":
                self.endless()
                return
            if name not in REMOTE:
                self.send_json({"detail": "not found"}, 404)
                return
            self.send_bytes("image/png", REMOTE[name])
            return
        parts = url.path.split("/")
        if len(parts) == 5 and parts[:3] == ["", "api", "sessions"] and parts[3] == NAME and parts[4] == "file":
            path = parse_qs(url.query).get("path", [""])[0]
            name = path.rsplit("/", 1)[-1]
            self.note("file", name)
            if not self.authorized():
                return
            if not path.startswith(DIR + "/") or name not in FILES:
                self.send_json({"detail": "not found"}, 404)
                return
            self.send_bytes(*FILES[name])
            return
        super().do_GET()


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
REMOTE_BASE = f"http://localhost:{server.server_port}/remote"
NOW = time.time()
FILLER = "\n\n".join(f"Parágrafo {n + 1} do relatório sintético: texto de preenchimento para a conversa rolar e a animação "
                     "sair da tela." for n in range(24))
EVENTS = [
    msg("user_msg", "m0", "Grava a animação do carregamento, lê a captura e monta o relatório.", ts=NOW - 400),
    msg("assistant_msg", "m1", f"Gravei a animação (sintético): {DIR}/animacao.gif", ts=NOW - 380),
    call("m2", "t1", "Read", {"file_path": f"{DIR}/lida.png"}),
    result("m3", "t1", "Read image (sintético)"),
    msg("assistant_msg", "m4", FILLER, ts=NOW - 340),
    msg("assistant_msg", "m5", "Relatório pronto (sintético).\n\n"
        f"Gráfico local: ![gráfico de barras]({DIR}/grafico.png)\n\n"
        f"Logo do parceiro: ![logo do parceiro]({REMOTE_BASE}/logo.png)\n\n"
        f"Falhas esperadas: ![]({DIR}/quebrada.png) e ![foto antiga]({REMOTE_BASE}/sumiu.png)\n\n"
        f"Sem fim: ![transmissão]({REMOTE_BASE}/infinito.png)", ts=NOW - 320),
]
data = info(NAME, "claude", state="idle", branch="main")
data["cwd"] = "/sintetica/projetos/hangar-sintetico"
SESSIONS[NAME] = {"info": data, "state": state("idle"), "events": [dict(e) for e in EVENTS], "stats": None, "modes": []}
bump()

print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
