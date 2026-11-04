"""Contém apenas os escritores filhos do Rust e comprova o encerramento deles."""
from __future__ import annotations

import os
import json
import logging
import signal
import subprocess
import sys
import time
import uuid
import threading
import tempfile
from pathlib import Path
from dataclasses import dataclass

import psutil
_record_guard = threading.RLock()
_FULL_SWEEP = 5.0
_tree_warned = False


class WindowsJob:
    def __init__(self, name=None, *, existing=False):
        import ctypes as c
        from ctypes import wintypes as w
        self.c = c
        self.api = c.WinDLL('kernel32', use_last_error=True)
        signatures = {
            'CreateJobObjectW': ([w.LPVOID, w.LPCWSTR], w.HANDLE),
            'OpenJobObjectW': ([w.DWORD, w.BOOL, w.LPCWSTR], w.HANDLE),
            'SetInformationJobObject': ([w.HANDLE, c.c_int, w.LPVOID, w.DWORD], w.BOOL),
            'AssignProcessToJobObject': ([w.HANDLE, w.HANDLE], w.BOOL),
            'TerminateJobObject': ([w.HANDLE, w.UINT], w.BOOL),
            'QueryInformationJobObject': ([w.HANDLE, c.c_int, w.LPVOID, w.DWORD, w.LPVOID], w.BOOL),
            'CloseHandle': ([w.HANDLE], w.BOOL),
        }
        for api_name, (args, result) in signatures.items():
            fn = getattr(self.api, api_name)
            fn.argtypes, fn.restype = args, result
        class Basic(c.Structure):
            _fields_ = [('user', c.c_int64), ('kernel', c.c_int64), ('flags', w.DWORD),
                ('min_ws', c.c_size_t), ('max_ws', c.c_size_t), ('limit', w.DWORD),
                ('affinity', c.c_size_t), ('priority', w.DWORD), ('scheduling', w.DWORD)]
        class Io(c.Structure):
            _fields_ = [(name, c.c_uint64) for name in ('read_ops','write_ops','other_ops','read_bytes','write_bytes','other_bytes')]
        class Extended(c.Structure):
            _fields_ = [('basic', Basic), ('io', Io), ('process_memory', c.c_size_t),
                ('job_memory', c.c_size_t), ('peak_process', c.c_size_t), ('peak_job', c.c_size_t)]
        class Accounting(c.Structure):
            _fields_ = [(name, c.c_int64) for name in ('user','kernel','period_user','period_kernel')] + [
                (name, w.DWORD) for name in ('faults','total','active','terminated')]
        self.Accounting = Accounting
        self.name = name or 'Global\\Hangar-runtime-' + uuid.uuid4().hex
        self.handle = (self.api.OpenJobObjectW(0x0004 | 0x0008, False, self.name) if existing
            else self.api.CreateJobObjectW(None, self.name))
        if not self.handle:
            if existing and c.get_last_error() == 2:
                return
            raise c.WinError(c.get_last_error())
        if existing:
            return
        try:
            limits = Extended()
            # O handle pertence só à vigia; fechar encerra o grupo. 0x800: o terminal de sessão sai do
            # Job ao nascer (CREATE_BREAKAWAY_FROM_JOB), senão o reinício do backend mata a sessão.
            limits.basic.flags = 0x2000 | 0x800
            self.check(self.api.SetInformationJobObject(self.handle, 9, c.byref(limits), c.sizeof(limits)))
        except BaseException:
            self.close()
            raise

    def check(self, result):
        if not result:
            raise self.c.WinError(self.c.get_last_error())

    def assign(self, proc):
        self.check(self.api.AssignProcessToJobObject(self.handle, int(proc._handle)))

    def terminate(self):
        self.check(self.api.TerminateJobObject(self.handle, 1))

    def members(self):
        """PIDs do Job neste instante (JobObjectBasicProcessIdList)."""
        from ctypes import wintypes as w
        class Ids(self.c.Structure):
            _fields_ = [('assigned', w.DWORD), ('listed', w.DWORD), ('ids', self.c.c_size_t * 1024)]
        data = Ids()
        self.check(self.api.QueryInformationJobObject(self.handle, 3, self.c.byref(data), self.c.sizeof(data), None))
        return [int(data.ids[n]) for n in range(data.listed)]

    def active(self):
        data = self.Accounting()
        self.check(self.api.QueryInformationJobObject(self.handle, 1, self.c.byref(data), self.c.sizeof(data), None))
        return data.active

    def close(self):
        if self.handle:
            self.check(self.api.CloseHandle(self.handle))
            self.handle = None


@dataclass
class Containment:
    pid: int
    birth: float
    job: WindowsJob | None = None
    cleaned: bool = False
    record: Path | None = None
    record_lease: object | None = None
    life: str | None = None
    members: dict | None = None
    saved: bool = False
    swept: float | None = None


def spawn_contained(args, *, env, record_path=None):
    from app.runtime_coordinator import WriterLease
    record = (record_path if isinstance(record_path, Path) else Path(record_path)) if record_path is not None else None
    lease = WriterLease(record.with_suffix('.lock')) if record is not None else None
    if record is not None:
        try:
            reconcile_startup(record, lease=lease)
        except BaseException:
            lease.close()
            raise
    job = None
    proc = None
    try:
        job = WindowsJob() if sys.platform == 'win32' else None
        kw = {'creationflags':0x08000000 | 0x00000004} if job else {'start_new_session':True}
        proc = subprocess.Popen(args, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, **kw)
        proc.runtime_containment = Containment(proc.pid, psutil.Process(proc.pid).create_time(), job,
            record=record, record_lease=lease, life=uuid.uuid4().hex)
        if job:
            job.assign(proc)
            psutil.Process(proc.pid).resume()
        elif os.getpgid(proc.pid) != proc.pid:
            raise RuntimeError('filho Rust sem grupo próprio')
        if record is not None:
            refresh_members(proc)
        return proc
    except BaseException:
        if proc is not None:
            proc.kill()
            proc.wait(timeout=5)
        if job:
            job.close()
        if lease:
            lease.close()
        raise


def _group_members(containment):
    members = []
    for proc in psutil.process_iter(['pid','status','create_time']):
        try:
            if os.getsid(proc.pid) == containment.pid and proc.status() != psutil.STATUS_ZOMBIE:
                if proc.pid == containment.pid and proc.create_time() != containment.birth:
                    raise RuntimeError('identidade do grupo Rust mudou')
                members.append(proc.pid)
        except (psutil.NoSuchProcess, ProcessLookupError):
            continue
    return members


_listchildpids = None


def _darwin_listchildpids():
    """`proc_listchildpids` da libproc; None fora do macOS ou sem a função."""
    global _listchildpids
    if _listchildpids is None:
        if sys.platform != 'darwin':
            _listchildpids = False
        else:
            import ctypes
            try:
                api = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True).proc_listchildpids
            except (OSError, AttributeError):
                _listchildpids = False
            else:
                api.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
                api.restype = ctypes.c_int

                def listchildren(pid):
                    size = 64
                    while True:
                        buffer = (ctypes.c_int * size)()
                        count = api(pid, buffer, ctypes.sizeof(buffer))
                        if count <= 0:
                            return []
                        if count < size:   # lista cheia pode ter cortado filhos: tenta com mais espaço
                            return list(buffer[:count])
                        size *= 4

                _listchildpids = listchildren
    return _listchildpids or None


def _tree_members(containment):
    """Desce do Rust pelos filhos que o kernel lista, sem olhar o resto da máquina.

    None = o kernel não expõe `children`; quem chama varre a máquina."""
    if not sys.platform.startswith('linux'):
        # macOS: a libproc lista só os filhos de cada membro, em vez de consultar a máquina inteira.
        listchildren = _darwin_listchildpids()
        if listchildren is None:
            return None
        members, pending = [], [containment.pid]
        while pending:
            pid = pending.pop()
            try:
                if os.getsid(pid) != containment.pid or psutil.Process(pid).status() == psutil.STATUS_ZOMBIE:
                    continue
            except (psutil.NoSuchProcess, ProcessLookupError):
                continue
            members.append(pid)
            pending.extend(listchildren(pid))
        return members
    if not Path(f'/proc/{containment.pid}/task/{containment.pid}/children').exists():
        return None
    members, pending = [], [containment.pid]
    while pending:
        pid = pending.pop()
        try:
            if os.getsid(pid) != containment.pid or psutil.Process(pid).status() == psutil.STATUS_ZOMBIE:
                continue
            tasks = list(Path(f'/proc/{pid}/task').iterdir())
        except (psutil.NoSuchProcess, ProcessLookupError, FileNotFoundError):
            continue
        members.append(pid)
        for task in tasks:
            try:
                pending.extend(int(child) for child in (task / 'children').read_text().split())
            except FileNotFoundError:
                continue  # a thread saiu entre a listagem e a leitura
    return members


def cleanup(proc, timeout=5):
    containment = getattr(proc, 'runtime_containment', None)
    if containment is None:
        raise RuntimeError('fim dos descendentes Rust sem prova de contenção')
    if containment.cleaned:
        return True
    deadline = time.monotonic() + timeout
    if containment.job:
        # A contagem do Job zera antes de o processo terminar de sair: a prova espera cada membro.
        tracked = []
        try:
            pids = containment.job.members()
        except OSError:
            # Lista parcial não pode impedir o encerramento; a contagem do Job cobre o resto.
            logging.getLogger('hangar.runtime').warning('membros do Job não listados; prova só pela contagem', exc_info=True)
            pids = []
        for pid in pids:
            try:
                tracked.append((pid, psutil.Process(pid).create_time()))
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                continue
        containment.job.terminate()
        while containment.job.active() or any(_still_there(pid, birth) for pid, birth in tracked):
            if time.monotonic() >= deadline:
                raise RuntimeError('descendentes Rust ainda ativos no Job')
            time.sleep(.02)
        containment.job.close()
    else:
        while members := _group_members(containment):
            groups = set()
            for pid in members:
                try:
                    groups.add(os.getpgid(pid))
                except ProcessLookupError:
                    continue
            for group in groups:
                try:
                    os.killpg(group, signal.SIGKILL)
                except ProcessLookupError:
                    continue
            if time.monotonic() >= deadline:
                raise RuntimeError('descendentes Rust ainda ativos no grupo')
            time.sleep(.02)
    proc.poll()
    containment.cleaned = True
    if containment.record is not None:
        with _record_guard:
            if containment.record.exists():
                saved = json.loads(containment.record.read_bytes())
                if saved.get('life') != containment.life:
                    raise RuntimeError('registro de contenção mudou durante a limpeza')
                containment.record.unlink()
        containment.record_lease.close()
    return True


def record_path():
    from app import log_paths
    return log_paths.base().parent / 'runtime-process.json'


def boot_identity():
    if sys.platform.startswith('linux'):
        return Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    return None


def _same_process(pid, birth):
    try:
        proc = psutil.Process(pid)
        if proc.create_time() != birth or proc.status() == psutil.STATUS_ZOMBIE:
            return False
        # No Windows o processo encerrado segue listado enquanto alguém segura um handle dele, com o
        # mesmo nascimento; sem threads ele já saiu e não escreve mais.
        return sys.platform != 'win32' or proc.num_threads() > 0
    except psutil.NoSuchProcess:
        return False


def _still_there(pid, birth):
    # Sem permissão para olhar, o processo conta como presente: a prova só fecha com a saída vista.
    try:
        return _same_process(pid, birth)
    except psutil.AccessDenied:
        return True


def refresh_members(proc):
    containment = proc.runtime_containment
    if containment.record is None or containment.cleaned:
        return
    if not _same_process(containment.pid, containment.birth):
        return
    members = {}
    if containment.job is None:
        # A árvore perde o neto cujo pai morreu (ele segue na sessão); a varredura periódica o pega.
        now = time.monotonic()
        sweep = containment.swept is None or now - containment.swept >= _FULL_SWEEP
        found = None if sweep else _tree_members(containment)
        if found is None:
            global _tree_warned
            if not sweep and not _tree_warned:
                _tree_warned = True
                logging.getLogger('hangar.runtime').warning('kernel sem /proc/<pid>/task/*/children; Supervisor varre a máquina a cada volta')
            found = _group_members(containment)
            containment.swept = now
    else:
        # Fechado o último handle, o nome do Job some antes de os membros saírem: o reinício só espera os gravados.
        found = containment.job.members()
    for pid in found:
        try:
            members[str(pid)] = psutil.Process(pid).create_time()
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    from app import atomico
    with _record_guard:
        path = containment.record
        merged = {**(containment.members or {}), **members}
        # Registro apagado por fora volta na hora: sem ele o restart pós-queda não prova nada.
        if containment.saved and merged == containment.members and path.exists():
            return
        containment.members = merged
        containment.saved = False
        owner = psutil.Process(os.getpid())
        data = dict(version=1, life=containment.life, platform=sys.platform, owner_pid=owner.pid,
            owner_birth=owner.create_time(), pid=containment.pid, birth=containment.birth,
            pgid=containment.pid, members=containment.members, boot=boot_identity(),
            job_name=containment.job.name if containment.job else None)
        path.parent.mkdir(parents=True, exist_ok=True)
        stream = tempfile.NamedTemporaryFile(dir=path.parent, delete=False)
        temporary = Path(stream.name)
        # Disco cheio no meio da escrita não pode deixar o temporário: o vigia tenta de novo a cada volta.
        try:
            with stream:
                stream.write(json.dumps(data).encode())
                stream.flush()
                os.fsync(stream.fileno())
            atomico.substituir(temporary, path)
            containment.saved = True
        finally:
            temporary.unlink(missing_ok=True)


def reconcile_startup(path=None, *, allow_current=False, lease=None):
    from app.runtime_coordinator import WriterLease
    path = path or record_path()
    path = path if isinstance(path, Path) else Path(path)
    with _record_guard:
        saved = json.loads(path.read_bytes()) if path.exists() else None
        if saved:
            if (saved.get('version') != 1 or saved.get('platform') != sys.platform
                    or type(saved.get('pid')) is not int or saved['pid'] <= 0
                    or saved.get('pgid') != saved['pid'] or type(saved.get('owner_pid')) is not int
                    or not isinstance(saved.get('members'), dict) or not isinstance(saved.get('life'), str)
                    or not isinstance(saved.get('birth'), (int,float)) or not isinstance(saved.get('owner_birth'), (int,float))):
                raise RuntimeError('registro de contenção inválido; escrita suspensa')
            owner_alive = _same_process(saved['owner_pid'], saved['owner_birth'])
            rust_alive = _same_process(saved['pid'], saved['birth'])
            if owner_alive:
                if allow_current and saved['owner_pid'] == os.getpid() and rust_alive:
                    return True
                raise RuntimeError('outro ciclo do backend ainda possui escritores Rust')
        acquired = WriterLease(path.with_suffix('.lock')) if lease is None else lease
        try:
            if saved is None:
                return True
            if sys.platform.startswith('linux') and saved.get('boot') != boot_identity():
                path.unlink()
                return True
            if sys.platform == 'win32':
                import re
                if not isinstance(saved.get('job_name'), str) or not re.fullmatch(r'Global\\Hangar-runtime-[a-f0-9]{32}', saved['job_name']):
                    raise RuntimeError('Job anterior sem identidade; escrita suspensa')
                job = WindowsJob(saved['job_name'], existing=True)
                if job.handle is not None:
                    orphan = Containment(saved['pid'], saved['birth'], job)
                    holder = type('ContainedProcess', (), {'runtime_containment':orphan, 'poll':lambda self:None})()
                    cleanup(holder)
                else:
                    # Job sumido = o último handle fechou e o Windows ainda encerra os membros: espera cada um sair.
                    gravados = [(saved['pid'], saved['birth']), *((int(pid), birth) for pid, birth in saved['members'].items())]
                    deadline = time.monotonic() + 5
                    while any(_still_there(pid, birth) for pid, birth in gravados):
                        if time.monotonic() >= deadline:
                            raise RuntimeError('descendentes Rust ainda ativos sem o Job')
                        time.sleep(.02)
            else:
                containment = Containment(saved['pid'], saved['birth'])
                members = _group_members(containment)
                if members and not any(_same_process(pid, saved['members'].get(str(pid))) for pid in members):
                    raise RuntimeError('grupo órfão sem prova de nascimento; escrita suspensa')
                holder = type('ContainedProcess', (), {'runtime_containment':containment, 'poll':lambda self:None})()
                cleanup(holder)
            if path.exists() and json.loads(path.read_bytes()).get('life') != saved['life']:
                raise RuntimeError('contenção mudou durante a reconciliação')
            path.unlink(missing_ok=True)
            return True
        finally:
            if lease is None:
                acquired.close()
