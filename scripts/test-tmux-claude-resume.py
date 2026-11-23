"""Confere o `save` do tmux-claude-resume.sh sem tocar em tmux nem em sessões reais: as sessões do
Hangar (com `CP_SESSION_KEY` e spec, ou internas com `@cp_hidden`) saem da foto do resurrect; as
outras ficam como estavam."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile


script = Path(__file__).with_name("tmux-claude-resume.sh").resolve()
KEY = "ab" * 16
with tempfile.TemporaryDirectory(prefix="hangar-resume-test-") as directory:
    root = Path(directory)
    resurrect = root / "resurrect"
    resurrect.mkdir()
    env = dict(os.environ, HOME=directory, TMUX_RESURRECT_DIR=str(resurrect))
    bin_dir = root / "bin"
    bin_dir.mkdir()
    tmux = bin_dir / "tmux"
    tmux.write_text(f"#!{sys.executable}\n" + f'''import sys
args = sys.argv[1:]
if args[0] == 'list-sessions':
    print('hangar-a'); print('shell'); print('hangar b'); print('shortcut-hangar-abc123\\t1')
elif args[0] == 'show-environment':
    target = args[args.index('-t') + 1]
    if target == '=hangar-a':
        print('CP_SESSION_KEY={KEY}')
    elif target == '=hangar b':
        print('CP_SESSION_KEY={"cd" * 16}')
    else:
        sys.exit(1)
''')
    tmux.chmod(0o755)
    env["PATH"] = str(bin_dir) + os.pathsep + env["PATH"]
    # Só a sessão com spec gravado é do servidor (a adoção grava o do Claude cru); a `hangar b`, com a chave
    # e sem spec (a gravação falhou), fica com o resurrect em vez de sumir no reboot.
    (root / ".hangar/session-specs").mkdir(parents=True)
    (root / f".hangar/session-specs/{KEY}.json").write_text("{}")
    save = resurrect / "tmux_resurrect_20261016T000000.txt"
    lines = [
        "pane\thangar-a\t1\t:\t1\t:*\t0\t:/w\t1\tclaude\t:claude",
        "window\thangar-a\t1\t:claude\t1\t:*\tlayout\t:",
        "pane\tshell\t1\t:\t1\t:*\t0\t:/w\t1\tfish\t:",
        "window\tshell\t1\t:fish\t1\t:*\tlayout\t:",
        "pane\thangar b\t1\t:\t1\t:*\t0\t:/w\t1\tpi\t:pi",
        "state\thangar-a\tshell",
        # Terminal de atalho (`@cp_hidden`): sem spec, mas recriado cru viraria linha Claude sem Claude.
        "pane\tshortcut-hangar-abc123\t0\t:\t1\t:*\t0\t:/w\t1\tfish\t:",
        "window\tshortcut-hangar-abc123\t0\t:fish\t1\t:*\tlayout\t:",
    ]
    save.write_text("\n".join(lines) + "\n")
    (resurrect / "last").symlink_to(save.name)
    result = subprocess.run(["bash", str(script), "save"], env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    kept = save.read_text().splitlines()
    assert kept == [lines[2], lines[3], lines[4], lines[5]], kept
    assert (resurrect / "last").is_symlink(), "o `last` continua apontando a foto"
    assert "2 sessões do Hangar" in (root / ".hangar/logs/privado/claude-resume.log").read_text()

    # Sem sessão do Hangar a foto fica intacta.
    save.write_text("\n".join(lines[2:4]) + "\n")
    env["PATH"] = str(bin_dir) + os.pathsep + env["PATH"]
    tmux.write_text(f"#!{sys.executable}\nimport sys\nprint('shell') if sys.argv[1] == 'list-sessions' else sys.exit(1)\n")
    result = subprocess.run(["bash", str(script), "save"], env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert save.read_text().splitlines() == lines[2:4]
print("ok")
