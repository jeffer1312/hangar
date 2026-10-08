"""Captura respostas integrais dos handlers de anexos sem tocar sessões reais."""
import base64
from contextlib import ExitStack, closing
from dataclasses import dataclass, asdict
import ipaddress
import json
import os
import re
from pathlib import Path
import socket
import sys
from types import SimpleNamespace
from unittest.mock import patch
from urllib.parse import quote

from fastapi.testclient import TestClient

from app import api, uploads, video
from app.models import SessionInfo

EPOCH = 1700000000
FIXTURES = Path(__file__).parent / "fixtures" / "uploads_contract"
FIXTURE_NAMES = (
    "binary-and-gallery", "errors-and-isolation", "retention-and-audio",
    "active-content-range", "video-without-ffmpeg",
)


@dataclass
class UploadCapture:
    normalized_response: list[dict]
    normalized_tree: list[dict]


def assert_native_upload(observation: dict) -> None:
    """A reivindicação futura deve atender sem enviar bytes ao handler Python."""
    assert observation["status"] == 200 and observation["upstream_body_bytes"] == 0, (
        f"autoria recusada: status {observation['status']}, "
        f"{observation['upstream_body_bytes']} bytes no upstream"
    )


class UploadReference:
    """I/O real; só identidade sintética, relógio, aleatoriedade e raiz são controlados."""

    def __init__(self, root: Path):
        self.root = Path(root)
        self.generation = 0
        self.original_connect = socket.socket.connect
        self.original_connect_ex = socket.socket.connect_ex
        self.original_getaddrinfo = socket.getaddrinfo

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False

    def _check_address(self, address):
        if isinstance(address, tuple):
            host = address[0]
            try:
                allowed = ipaddress.ip_address(host).is_loopback
            except ValueError:
                allowed = host == "localhost"
            if not allowed:
                raise OSError("rede externa bloqueada na referência")

    def _connect(self, sock, address):
        self._check_address(address)
        return self.original_connect(sock, address)

    def _connect_ex(self, sock, address):
        self._check_address(address)
        return self.original_connect_ex(sock, address)

    def _getaddrinfo(self, host, *args, **kwargs):
        if host is not None:
            self._check_address((host, 0))
        return self.original_getaddrinfo(host, *args, **kwargs)

    def environment(self, root, *, deterministic=True):
        stack = ExitStack()
        root = Path(root)
        self.current_root = root
        self.cwd = root / "project"
        self.other_cwd = root / "other-project"
        self.cwd.mkdir(parents=True, exist_ok=True)
        self.other_cwd.mkdir(parents=True, exist_ok=True)
        self.vault = root / "vault"
        self.sessions = [
            SessionInfo(name="fixture", cwd=str(self.cwd), jsonl=str(root / "transcript.jsonl")),
            SessionInfo(name="neighbor", cwd=str(self.cwd), jsonl=str(root / "neighbor.jsonl")),
            SessionInfo(name="other", cwd=str(self.other_cwd), jsonl=str(root / "other.jsonl")),
            SessionInfo(name="no-cwd"),
        ]
        self.tokens = 0
        self.records = []
        self.uploaded_paths = []
        self.project_names = {}
        def token_hex(n):
            self.tokens += 1
            return f"{self.tokens:0{2*n}x}"
        old_overrides = api.app.dependency_overrides.copy()
        stack.callback(lambda: (api.app.dependency_overrides.clear(),
                                api.app.dependency_overrides.update(old_overrides)))
        api.app.dependency_overrides[api.require_auth] = lambda: None
        api.app.dependency_overrides[api._transfer_check] = lambda: None
        stack.enter_context(patch.dict(os.environ, {
            "HOME": str(root / "home"), "USERPROFILE": str(root / "home"),
        }))
        stack.enter_context(patch.object(socket.socket, "connect",
                                        lambda sock, addr: self._connect(sock, addr)))
        stack.enter_context(patch.object(socket.socket, "connect_ex",
                                        lambda sock, addr: self._connect_ex(sock, addr)))
        stack.enter_context(patch.object(socket, "getaddrinfo", self._getaddrinfo))
        stack.enter_context(patch.object(uploads, "_raiz", return_value=self.vault))
        stack.enter_context(patch.object(api.registry, "list", side_effect=lambda *args, **kwargs: self.sessions))
        stack.enter_context(patch.object(api, "_list_snap", {"snap": None}))
        self.retention = 0
        original_config = api.runtime_config.get
        stack.enter_context(patch.object(api.runtime_config, "get",
            side_effect=lambda key, *a: self.retention if key == "upload_retention_days"
            else original_config(key, *a)))
        if deterministic:
            stack.enter_context(patch.object(uploads, "time", SimpleNamespace(time=lambda: EPOCH)))
            stack.enter_context(patch.object(uploads, "secrets", SimpleNamespace(token_hex=token_hex)))
        for cwd, label in ((self.cwd, "<project>"), (self.other_cwd, "<other-project>")):
            self.project_names[uploads._projeto(str(cwd))] = label
        return stack

    def normalize(self, value):
        if isinstance(value, dict):
            return {k: self.normalize(v) for k, v in value.items()}
        if isinstance(value, list):
            return [self.normalize(v) for v in value]
        if isinstance(value, str):
            prefix = str(self.current_root.resolve())
            if value.startswith(prefix):
                value = "<root>" + value[len(prefix):].replace(os.sep, "/")
            for name, label in self.project_names.items():
                value = value.replace(name, label)
        return value

    def _normalize_json_body(self, content: bytes) -> bytes:
        text = content.decode("utf-8")
        def replace_token(match):
            token = match.group()
            if text[match.end():].lstrip().startswith(":"):
                return token
            return self._normalize_json_string(token)
        return re.sub(r'"(?:[^"\\]|\\.)*"', replace_token, text).encode("utf-8")

    def _normalize_json_string(self, token: str) -> str:
        value = json.loads(token)
        if self.normalize(value) == value:
            return token
        offsets = []
        index = 1
        while index < len(token) - 1:
            offsets.append(index)
            if token[index] == "\\":
                if token[index + 1] == "u":
                    width = 6
                    code = int(token[index + 2:index + 6], 16)
                    if (0xD800 <= code <= 0xDBFF and token[index + 6:index + 8] == "\\u"
                            and 0xDC00 <= int(token[index + 8:index + 12], 16) <= 0xDFFF):
                        width = 12
                    index += width
                else:
                    index += 2
            else:
                index += 1
        offsets.append(index)
        assert len(offsets) == len(value) + 1
        edits = []
        prefix = str(self.current_root.resolve())
        remaining_start = 0
        if value.startswith(prefix):
            edits.append((0, len(prefix), "<root>"))
            remaining_start = len(prefix)
            if os.sep != "/":
                edits.extend((i, i + 1, "/") for i in range(remaining_start, len(value))
                             if value[i] == os.sep)
        for name, label in self.project_names.items():
            start = value.find(name, remaining_start)
            while start >= 0:
                edits.append((start, start + len(name), label))
                start = value.find(name, start + len(name))
        result = []
        cursor = 0
        for start, end, replacement in sorted(edits):
            # Só os intervalos voláteis mudam; escapes do restante do token são literais.
            result.append(token[cursor:offsets[start]])
            result.append(json.dumps(replacement, ensure_ascii=False)[1:-1])
            cursor = offsets[end]
        result.append(token[cursor:])
        return "".join(result)

    def request(self, client, method, url, **kwargs):
        response = client.request(method, url, **kwargs)
        # O mtime é uma entrada explícita da fixture, anterior à galeria/Range.
        if method == "POST" and "/upload" in url and response.status_code == 200:
            path = Path(response.json()["path"])
            os.utime(path, (EPOCH, EPOCH))
            self.uploaded_paths.append(path)
        content = response.content
        parsed = None
        if response.headers.get("content-type", "").startswith("application/json"):
            parsed = self.normalize(response.json())
            content = self._normalize_json_body(content)
        headers = dict(response.headers)
        if parsed is not None and "content-length" in headers:
            headers["content-length"] = str(int(headers["content-length"]) - len(response.content) + len(content))
        record = {
            "kind": "http", "method": method, "url": self.normalize(url),
            "request": {
                "headers": dict(kwargs.get("headers", {})),
                "body_base64": base64.b64encode(kwargs.get("content", b"")).decode("ascii"),
            },
            "status": response.status_code, "headers": headers,
            "body_base64": base64.b64encode(content).decode("ascii"), "json": parsed,
        }
        self.records.append(record)
        return response

    def snapshot_tree(self):
        result = []
        if not self.vault.exists():
            return result
        for path in sorted(self.vault.rglob("*")):
            st = path.stat()
            item = {
                "path": self.normalize(str(path.resolve())),
                "kind": "directory" if path.is_dir() else "file",
                "mtime": "<directory-mtime>" if path.is_dir() else st.st_mtime,
                "mode": st.st_mode,
            }
            if path.is_file():
                item["size"] = st.st_size
                item["content_base64"] = base64.b64encode(path.read_bytes()).decode("ascii")
            result.append(item)
        return result

    def _upload(self, client, content, filename, session="fixture", query=""):
        response = self.request(client, "POST", f"/api/sessions/{session}/upload{query}",
                                content=content, headers={"x-filename": quote(filename, safe="")})
        if response.status_code != 200:
            raise AssertionError(response.text)
        return Path(response.json()["path"])

    def _resolve_audio(self, label, ref, *, owner=True, cwd=None, session="transcript"):
        try:
            path = uploads.resolve_session_audio(str(cwd or self.cwd), session, ref,
                                                 allow_absolute=owner)
            result = {"status": 200, "path": self.normalize(path)}
        except uploads.UploadError as exc:
            result = {"status": exc.status, "detail": exc.detail}
        self.records.append({"kind": "audio-resolver", "case": label, "result": result})

    def run_fixture(self, name):
        if name not in FIXTURE_NAMES:
            raise ValueError(name)
        self.generation += 1
        root = self.root / f"run-{self.generation}"
        with self.environment(root), closing(TestClient(api.app, headers={"accept-encoding": "identity"})) as client:
            if name == "binary-and-gallery":
                self.request(client, "GET", "/api/sessions/fixture/uploads")
                first = self._upload(client, "referência-binária".encode() + b"\x00\xff", "relatorio.bin")
                second = self._upload(client, "texto com acentos: ação".encode(), "texto.txt")
                self.request(client, "GET", "/api/sessions/fixture/uploads")
                self.request(client, "GET", f"/api/sessions/fixture/uploads/{first.name}")
                self.request(client, "GET", f"/api/sessions/fixture/uploads/{second.name}?download=true")
                self.sessions[0].name = "renamed"
                api._list_snap["snap"] = None
                self.request(client, "GET", "/api/sessions/renamed/uploads")
            elif name == "errors-and-isolation":
                for session in ("missing", "no-cwd", "fixture"):
                    self.request(client, "POST", f"/api/sessions/{session}/upload", content=b"")
                self.request(client, "POST", "/api/sessions/fixture/upload", content=b"x",
                             headers={"content-length": str(100 * 1024 * 1024 + 1)})
                own = self._upload(client, b"own", "../../ameaça%2Etxt")
                self._upload(client, b"neighbor", "neighbor.bin", "neighbor")
                self._upload(client, b"other", "other.bin", "other")
                for session in ("neighbor", "other", "missing"):
                    self.request(client, "GET", f"/api/sessions/{session}/uploads/{own.name}")
                self.request(client, "GET", "/api/sessions/fixture/uploads/no-file.bin")
                self.request(client, "GET", "/api/sessions/fixture/uploads/a%5Cb")
                for hostile in ("..", "../..", "a/b", "..\\x"):
                    try:
                        path = uploads.save_upload(str(self.cwd), hostile, b"x", "x.png")
                        os.utime(path, (EPOCH, EPOCH))
                        result = {"status": 200, "path": self.normalize(path)}
                    except uploads.UploadError as exc:
                        result = {"status": exc.status, "detail": exc.detail}
                    self.records.append({"kind": "platform-save", "session": hostile, "result": result})
            elif name == "retention-and-audio":
                own = self._upload(client, b"audio-fixture", "audio.webm", query="?audio_only=true")
                neighbor = self._upload(client, b"old", "old.webm", "neighbor", "?audio_only=true")
                other = self._upload(client, b"other", "other.webm", "other", "?audio_only=true")
                self._resolve_audio("own-name", own.name)
                self._resolve_audio("previous-transcript", str(neighbor))
                self._resolve_audio("guest-absolute", str(own), owner=False)
                self._resolve_audio("other-project", str(other))
                self._resolve_audio("network", "//outside.invalid/share/audio.webm")
                self._resolve_audio("nul", "bad\x00.webm")
                self._resolve_audio("traversal", "../audio.webm")
                self._resolve_audio("missing", "missing.webm")
                self.retention = 7
                os.utime(neighbor, (EPOCH - 8 * 86400, EPOCH - 8 * 86400))
                os.utime(other, (EPOCH - 8 * 86400, EPOCH - 8 * 86400))
                self.request(client, "GET", "/api/sessions/neighbor/uploads")
                self._upload(client, b"trigger", "trigger.bin")
                self.request(client, "GET", "/api/sessions/neighbor/uploads")
                self.request(client, "GET", "/api/sessions/other/uploads")
                self.retention = 0
                self.request(client, "GET", "/api/sessions/fixture/uploads")
            elif name == "active-content-range":
                samples = (
                    ("document.html", "<h1>Ação</h1><script>document.title='teste'</script>".encode()),
                    ("document.svg", b"<svg xmlns='http://www.w3.org/2000/svg'><script/></svg>"),
                    ("document.xml", b"<?xml version='1.0'?><documento/>"),
                    ("document.bin", bytes(range(32))),
                )
                for filename, payload in samples:
                    path = self._upload(client, payload, filename)
                    url = f"/api/sessions/fixture/uploads/{path.name}"
                    self.request(client, "GET", url)
                    self.request(client, "GET", url + "?download=true")
                    self.request(client, "GET", url, headers={"range": "bytes=2-5"})
                    self.request(client, "GET", url, headers={"range": "bytes=9999-"})
            else:
                with patch.object(video, "_ffmpeg", return_value=None):
                    self._upload(client, b"synthetic-video-no-decoder", "clip.mp4")
                self.request(client, "GET", "/api/sessions/fixture/uploads")
            return UploadCapture(self.records, self.snapshot_tree())

    def write_golden(self, name, *, approve=False):
        if not approve:
            raise ValueError("a gravação do golden exige aprovação explícita")
        capture = self.run_fixture(name)
        path = FIXTURES / sys.platform / f"{name}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(asdict(capture), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        return path
