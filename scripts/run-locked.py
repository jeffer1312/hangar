"""Roda um comando segurando uma trava exclusiva de arquivo: o `flock` de quem não tem `flock` (Git Bash).

Uso: run-locked.py <arquivo-da-trava> <comando> [args...]
     run-locked.py <arquivo-da-trava>   segura a trava, imprime `locked` e espera a entrada fechar
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


def die_together():
    """No Windows o filho não morre com o pai: morto este processo, o cargo seguiria sem a trava. Num Job
    com KILL_ON_JOB_CLOSE, o handle fecha junto com este processo e leva todos os descendentes."""
    import ctypes
    from ctypes import wintypes as w
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateJobObjectW.restype = w.HANDLE
    kernel.GetCurrentProcess.restype = w.HANDLE
    kernel.SetInformationJobObject.argtypes = [w.HANDLE, ctypes.c_int, w.LPVOID, w.DWORD]
    kernel.AssignProcessToJobObject.argtypes = [w.HANDLE, w.HANDLE]

    class Basic(ctypes.Structure):
        _fields_ = [("user", ctypes.c_int64), ("kernel", ctypes.c_int64), ("flags", w.DWORD),
                    ("min_ws", ctypes.c_size_t), ("max_ws", ctypes.c_size_t), ("limit", w.DWORD),
                    ("affinity", ctypes.c_size_t), ("priority", w.DWORD), ("scheduling", w.DWORD)]

    class Extended(ctypes.Structure):
        _fields_ = [("basic", Basic), ("io", ctypes.c_uint64 * 6), ("process_memory", ctypes.c_size_t),
                    ("job_memory", ctypes.c_size_t), ("peak_process", ctypes.c_size_t), ("peak_job", ctypes.c_size_t)]

    job = kernel.CreateJobObjectW(None, None)
    limits = Extended()
    limits.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    if not (job and kernel.SetInformationJobObject(job, 9, ctypes.byref(limits), ctypes.sizeof(limits))
            and kernel.AssignProcessToJobObject(job, kernel.GetCurrentProcess())):
        raise ctypes.WinError(ctypes.get_last_error())


def main():
    path, *command = sys.argv[1:]
    if os.name == "nt":
        die_together()
    lock = hold(path)
    try:
        if not command:
            print("locked", flush=True)
            sys.stdin.read()
            return 0
        return subprocess.call(command)
    finally:
        lock.close()


if __name__ == "__main__":
    sys.exit(main())
