"""Captura CDP e JPEG reais com o cliente Python de referência e perfil descartável."""

import argparse
import asyncio
import base64
import contextlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
from urllib.parse import quote, urlsplit

import psutil
import websockets


SOURCE = "ba2db1021ab7fb801eca50a50f9509dcb825ac7e"
HTML = """<!doctype html><meta charset="utf-8"><style>
body{margin:0;background:#e8eef4;font:20px sans-serif}input,button{margin:12px;font:inherit}
p{animation:pulse .4s infinite alternate}@keyframes pulse{to{opacity:.9}}
</style><input id="text" value=""><button id="button" onclick="this.textContent='Recebido'">Enviar</button>
<p>Ação e navegador remoto</p>"""


async def capture(browser: Path, root: Path, source_root: Path) -> dict:
    sys.path.insert(0, str(source_root / "backend"))
    from app.navsock import _Cdp

    browser_env = {
        "PATH": os.defpath, "HOME": str(root / "home"), "USERPROFILE": str(root / "home"),
        "XDG_CONFIG_HOME": str(root / "config"), "XDG_CACHE_HOME": str(root / "cache"),
        "TMPDIR": str(root), "LANG": "C.UTF-8", "TZ": "UTC",
    }
    for key in ("SYSTEMROOT", "WINDIR"):
        if key in os.environ:
            browser_env[key] = os.environ[key]
    version = subprocess.run([str(browser), "--version"], check=True, capture_output=True,
                             text=True, env=browser_env).stdout.strip()
    proc = await asyncio.create_subprocess_exec(
        str(browser), "--headless", "--disable-gpu", "--disable-background-networking",
        "--disable-component-update", "--disable-sync", "--disable-extensions",
        "--no-first-run", "--no-default-browser-check", "--password-store=basic",
        "--host-resolver-rules=MAP * ~NOTFOUND", "--remote-debugging-address=127.0.0.1",
        "--remote-debugging-port=0", f"--user-data-dir={root / 'browser-profile'}", "about:blank",
        env=browser_env, stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.DEVNULL,
        stderr=asyncio.subprocess.PIPE,
    )
    transcript = []
    descendants = []
    try:
        async with asyncio.timeout(20):
            while True:
                line = await proc.stderr.readline()
                if not line:
                    raise RuntimeError("o Chromium encerrou antes de publicar seu endereço CDP")
                found = re.search(rb"DevTools listening on (ws://[^\s]+)", line)
                if found:
                    endpoint = found.group(1).decode()
                    break
        assert urlsplit(endpoint).hostname in ("127.0.0.1", "localhost", "::1")
        async with websockets.connect(endpoint, max_size=32 * 1024 * 1024) as browser_ws:
            await browser_ws.send(json.dumps({"id": 1, "method": "Target.getTargets"}))
            async with asyncio.timeout(10):
                while True:
                    answer = json.loads(await browser_ws.recv())
                    if answer.get("id") == 1:
                        break
            page = next(item for item in answer["result"]["targetInfos"] if item["type"] == "page")
            origin = urlsplit(endpoint)
            page_endpoint = f"ws://{origin.netloc}/devtools/page/{page['targetId']}"
        async with _Cdp(page_endpoint) as cdp:
            async def command(method, params=None):
                result = await cdp.cmd(method, params)
                transcript.append({"method": method, "params": params or {}, "response": result})
                if "error" in result:
                    raise RuntimeError(f"comando CDP recusado: {method}: {result['error']}")
                return result

            await command("Page.enable")
            await command("Page.navigate", {"url": "data:text/html;charset=utf-8," + quote(HTML)})
            async with asyncio.timeout(10):
                while (await cdp.eventos.get()).get("method") != "Page.loadEventFired":
                    pass
            await command("Page.startScreencast", {
                "format": "jpeg", "quality": 45, "maxWidth": 390, "maxHeight": 390,
                "everyNthFrame": 5,
            })
            async with asyncio.timeout(10):
                while True:
                    event = await cdp.eventos.get()
                    if event.get("method") == "Page.screencastFrame":
                        frame = event["params"]
                        break
            assert base64.b64decode(frame["data"]).startswith(b"\xff\xd8\xff")
            await command("Page.screencastFrameAck", {"sessionId": frame["sessionId"]})
            screenshot = (await command("Page.captureScreenshot", {"format": "jpeg", "quality": 55}))["result"]
            assert base64.b64decode(screenshot["data"]).startswith(b"\xff\xd8\xff")
            await command("Runtime.evaluate", {"expression": "document.querySelector('#text').focus()"})
            await command("Input.insertText", {"text": "ação 🎯"})
            await command("Input.dispatchKeyEvent", {
                "type": "keyDown", "key": "Backspace", "code": "Backspace",
                "windowsVirtualKeyCode": 8, "modifiers": 0,
            })
            await command("Input.dispatchKeyEvent", {
                "type": "keyUp", "key": "Backspace", "code": "Backspace",
                "windowsVirtualKeyCode": 8, "modifiers": 0,
            })
            value = (await command("Runtime.evaluate", {
                "expression": "document.querySelector('#text').value", "returnByValue": True,
            }))["result"]["result"]["value"]
            assert value == "ação ", "Backspace precisa remover o emoji inteiro no navegador real"
            await command("Page.stopScreencast")
        return {
            "source_commit": SOURCE,
            "generator": "backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/capture_chromium.py --browser <chrome-headless-shell>",
            "python_version": sys.version.split()[0], "platform": sys.platform,
            "browser_version": version, "page_html": HTML, "frame": frame,
            "screenshot": screenshot, "input_value_after_backspace": value,
            "transcript": transcript,
            "limits": [
                "O CDP é real, mas usa uma página e um perfil temporários; não é o app desktop nativo.",
                "Metadados e IDs desta captura são medidos e congelados, não normalizados para regeneração idêntica.",
                "O protocolo nativo fechado não permite Runtime.evaluate; essa leitura é só da prova externa.",
            ],
        }
    finally:
        if proc.returncode is None:
            try:
                descendants = psutil.Process(proc.pid).children(recursive=True)
            except psutil.NoSuchProcess:
                descendants = []
            with contextlib.suppress(ProcessLookupError):
                proc.terminate()
            try:
                await asyncio.wait_for(proc.wait(), 10)
            except TimeoutError:
                with contextlib.suppress(ProcessLookupError):
                    proc.kill()
                await asyncio.wait_for(proc.wait(), 10)
        for child in descendants:
            try:
                if child.is_running():
                    child.terminate()
            except psutil.NoSuchProcess:
                pass
        _, alive = await asyncio.to_thread(psutil.wait_procs, descendants, timeout=5)
        for child in alive:
            with contextlib.suppress(psutil.NoSuchProcess):
                child.kill()
        _, alive = await asyncio.to_thread(psutil.wait_procs, alive, timeout=5)
        assert not alive, "processos do Chromium da caracterização permaneceram vivos"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path(__file__).with_name("chromium.json"))
    args = parser.parse_args()
    browser, output = args.browser.resolve(strict=True), args.output.resolve()
    repo = Path(__file__).resolve().parents[5]
    archive = subprocess.run(["git", "archive", SOURCE, "backend/app"], cwd=repo,
                             check=True, capture_output=True).stdout
    with tempfile.TemporaryDirectory(prefix="hangar-browser-cdp-") as directory:
        root = Path(directory)
        with tarfile.open(fileobj=io.BytesIO(archive)) as source:
            source.extractall(root / "reference", filter="data")
        (root / "home").mkdir()
        previous_env = os.environ.copy()
        previous_cwd = Path.cwd()
        try:
            os.environ.clear()
            os.environ.update({
                "PATH": os.defpath, "HOME": str(root / "home"), "USERPROFILE": str(root / "home"),
                "XDG_CONFIG_HOME": str(root / "config"), "XDG_CACHE_HOME": str(root / "cache"),
                "CP_AUTH_TOKEN": "browser-contract-token", "CP_LAN_BIND_IP": "127.0.0.1",
                "LANG": "C.UTF-8", "TZ": "UTC", "GIT_CONFIG_GLOBAL": os.devnull,
            })
            for name in ("SYSTEMROOT", "WINDIR"):
                if name in previous_env:
                    os.environ[name] = previous_env[name]
            os.chdir(root)
            payload = asyncio.run(capture(browser, root, root / "reference"))
        finally:
            os.chdir(previous_cwd)
            os.environ.clear()
            os.environ.update(previous_env)
    output.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"CDP real capturado: {payload['browser_version']}; {len(payload['transcript'])} comandos; JPEG íntegro; processos encerrados")


if __name__ == "__main__":
    main()
