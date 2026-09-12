"""Confere save/restore sem tocar em tmux ou sessões reais."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile


script = Path(__file__).with_name("tmux-claude-resume.sh").resolve()
with tempfile.TemporaryDirectory(prefix="hangar-resume-test-") as directory:
    root = Path(directory)
    env = dict(os.environ, HOME=directory, TMUX_RESURRECT_DIR=str(root / "map"))
    fake_agent = root / "hangar-codex-tui"
    fake_agent.write_text("import time\ntime.sleep(30)\n")
    agent = subprocess.Popen([sys.executable, str(fake_agent)], env=env)
    try:
        bin_dir = root / "bin"
        bin_dir.mkdir()
        tmux = bin_dir / "tmux"
        tmux.write_text(f"#!{sys.executable}\n" + '''import json, os, sys
from pathlib import Path
command = sys.argv[1]
if command == 'list-sessions':
    print('missing %1 ' + os.environ['FAKE_PID'])
    print('valid %2 ' + os.environ['FAKE_PID'])
elif command == 'display':
    print('bash')
elif command == 'send-keys':
    with Path(os.environ['FAKE_OUTPUT']).open('a') as stream:
        stream.write(json.dumps(sys.argv[2:]) + '\\n')
''')
        tmux.chmod(0o755)
        output = root / "commands.jsonl"
        env.update(PATH=str(bin_dir) + os.pathsep + env["PATH"], FAKE_PID=str(agent.pid), FAKE_OUTPUT=str(output))
        sidecars = root / ".hangar/codex-sessions"
        sidecars.mkdir(parents=True)
        expected = {"thread_id": "12345678-1234-1234-1234-123456789012",
                    "cwd": '/home/exemplo/Área de trabalho/projeto "novo"',
                    "codex_home": '/home/exemplo/conta-ação\\local', "codex_account": "secundaria"}
        (sidecars / "valid.json").write_text(json.dumps(expected))

        def run(mode):
            result = subprocess.run(["bash", str(script), mode], env=env, capture_output=True, text=True, timeout=10)
            assert result.returncode == 0, (mode, result.stderr)

        run("save")
        saved = root / "map/claude-sessions.tsv"
        assert saved.read_text().startswith("valid\tcodex\t"), saved.read_text()
        (sidecars / "missing.json").write_text("{invalid")
        run("save")
        assert saved.read_text().startswith("valid\tcodex\t"), saved.read_text()
        for broken in (None, "{invalid"):
            if broken is None:
                (sidecars / "missing.json").unlink(missing_ok=True)
            else:
                (sidecars / "missing.json").write_text(broken)
            saved.write_text("missing\tcodex\told-thread\t\t\nvalid\tcodex\t" + expected["thread_id"] + "\t\t\n")
            output.unlink(missing_ok=True)
            run("restore")
            commands = [json.loads(line) for line in output.read_text().splitlines()]
            assert len(commands) == 1 and commands[0][1] == "=valid:.", commands
            args = shlex.split(commands[0][2])
            for flag, key in (("--cwd", "cwd"), ("--codex-home", "codex_home"), ("--codex-account", "codex_account")):
                assert args[args.index(flag) + 1] == expected[key], (flag, args)
        assert "missing" in (root / ".hangar/logs/privado/claude-resume.log").read_text()
        print("OK: sidecar ausente/inválido não interrompe outras sessões; caminhos JSON preservados")
    finally:
        agent.terminate()
        agent.wait(timeout=5)
