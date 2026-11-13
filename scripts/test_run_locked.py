"""run-locked.py: o segundo comando só começa depois que o primeiro solta a trava."""
from pathlib import Path
import subprocess
import sys

SCRIPT = Path(__file__).with_name("run-locked.py")


def test_second_command_waits_for_the_first(tmp_path):
    lock, log = tmp_path / "build.lock", tmp_path / "order.txt"
    write = ("import sys,time; open(sys.argv[1],'a').write(sys.argv[2]+'+'); print('dentro', flush=True); "
             "time.sleep(float(sys.argv[3])); open(sys.argv[1],'a').write(sys.argv[2]+'-')")
    first = subprocess.Popen([sys.executable, str(SCRIPT), str(lock), sys.executable, "-c", write, str(log), "a", "1.5"],
                             stdout=subprocess.PIPE, text=True)
    assert first.stdout.readline().strip() == "dentro", "o primeiro comando não pegou a trava"
    second = subprocess.run([sys.executable, str(SCRIPT), str(lock), sys.executable, "-c", write, str(log), "b", "0"], timeout=60)
    assert first.wait(timeout=60) == 0 and second.returncode == 0
    assert log.read_text() == "a+a-b+b-"


def test_exit_code_of_the_command_is_returned(tmp_path):
    done = subprocess.run([sys.executable, str(SCRIPT), str(tmp_path / "x.lock"), sys.executable, "-c", "raise SystemExit(7)"], timeout=60)
    assert done.returncode == 7
