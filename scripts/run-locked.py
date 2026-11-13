"""Roda um comando segurando uma trava exclusiva de arquivo: o `flock` de quem não tem `flock` (Git Bash).

Uso: run-locked.py <arquivo-da-trava> <comando> [args...]
O sistema solta a trava quando o processo morre, então ela nunca fica presa por uma build interrompida.
"""
import os
import subprocess
import sys
import time


def hold(path):
    lock = open(path, "a+b")
    if os.name != "nt":
        import fcntl
        fcntl.flock(lock, fcntl.LOCK_EX)
        return lock
    import msvcrt
    waiting = False
    while True:
        try:
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
            return lock
        except OSError:
            if not waiting:
                print(f"run-locked: esperando a vez em {path}", file=sys.stderr, flush=True)
                waiting = True
            time.sleep(1)


def main():
    path, *command = sys.argv[1:]
    lock = hold(path)
    try:
        return subprocess.call(command)
    finally:
        lock.close()


if __name__ == "__main__":
    sys.exit(main())
