"""Sonda a leitura entre contas no Codex puro, sem login ou turno de modelo."""

import asyncio
import json
import os
from pathlib import Path
import shutil
import tempfile


async def read_config(home, account, cwd):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("CODEX_", "OPENAI_", "CHATGPT_"))}
    env.update(HOME=str(home), USERPROFILE=str(home), CODEX_HOME=str(account),
               XDG_CONFIG_HOME=str(home / ".config"), XDG_DATA_HOME=str(home / ".local/share"),
               XDG_CACHE_HOME=str(home / ".cache"), XDG_STATE_HOME=str(home / ".local/state"))
    process = await asyncio.create_subprocess_exec(
        shutil.which("codex"), "app-server", "--stdio", cwd=cwd, env=env,
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.DEVNULL,
    )

    async def request(identifier, method, params):
        process.stdin.write((json.dumps({"id": identifier, "method": method, "params": params}) + "\n").encode())
        await process.stdin.drain()
        while line := await asyncio.wait_for(process.stdout.readline(), 15):
            message = json.loads(line)
            if message.get("id") == identifier:
                if "error" in message:
                    raise RuntimeError(message["error"])
                return message["result"]
        raise RuntimeError("O app-server encerrou antes de responder")

    try:
        await request(1, "initialize", {"clientInfo": {"name": "config-isolation-probe", "version": "1"}})
        result = await request(2, "config/read", {"cwd": str(cwd), "includeLayers": True})
        return {"model": result["config"]["model"],
                "origin": result["origins"]["model"]["name"]["type"],
                "sandbox": result["config"]["sandbox_mode"],
                "approval": result["config"]["approval_policy"]}
    finally:
        if process.returncode is None:
            process.terminate()
            try:
                await asyncio.wait_for(process.wait(), 3)
            except TimeoutError:
                process.kill()
                await process.wait()


async def main():
    with tempfile.TemporaryDirectory(prefix="codex-home-isolation-") as directory:
        home = Path(directory)
        primary, secondary, repo = home / ".codex", home / ".codex-other", home / "repo"
        for folder in (primary, secondary, repo, repo / ".git"):
            folder.mkdir()
        options = ('sandbox_mode="danger-full-access"\napproval_policy="never"\n'
                   + f'[projects.{json.dumps(str(home))}]\ntrust_level="trusted"\n'
                   + f'[projects.{json.dumps(str(repo))}]\ntrust_level="trusted"\n')
        (primary / "config.toml").write_text('model="primary-marker"\n' + options)
        (secondary / "config.toml").write_text('model="secondary-marker"\n' + options)
        cases = []
        for label, account, cwd in (("primary_home", primary, home),
                                     ("secondary_home", secondary, home),
                                     ("secondary_repo", secondary, repo)):
            cases.append((label, await read_config(home, account, cwd)))
        (secondary / "config.toml").write_text('project_root_markers=[]\nmodel="secondary-marker"\n' + options)
        cases.append(("secondary_no_parent_search", await read_config(home, secondary, home)))
        primary.rename(home / "primary-outside-discovery")
        cases.append(("secondary_without_default_folder", await read_config(home, secondary, home)))
        for label, result in cases:
            print(label, json.dumps(result))
        assert all(row["sandbox"] == "danger-full-access" and row["approval"] == "never"
                   for _, row in cases)
        assert cases[0][1]["model"] == "primary-marker"
        assert cases[2][1]["model"] == cases[4][1]["model"] == "secondary-marker"
        print("BUG REPRODUZIDO" if cases[1][1]["model"] == "primary-marker" else "CONTA ISOLADA")


asyncio.run(main())
