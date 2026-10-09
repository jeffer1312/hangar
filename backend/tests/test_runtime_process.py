import os
import subprocess
import sys
import time
from pathlib import Path

import psutil
import pytest


def _wait_pid(path, seconds=5):
    # O filho abre o arquivo antes de escrever: existir não basta, e no Windows lento lia ''.
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if path.exists() and (text := path.read_text().strip()):
            return int(text)
        time.sleep(.01)
    raise AssertionError(f"pid não gravado em {seconds}s")


def _ended(proc):
    # Zumbi no POSIX; no Windows o encerrado segue listado sem threads até o último handle fechar.
    try:
        return not proc.is_running() or proc.status() == psutil.STATUS_ZOMBIE or proc.num_threads() == 0
    except psutil.NoSuchProcess:
        return True


def test_exited_windows_process_held_by_a_handle_is_not_the_same_process(monkeypatch):
    from app import runtime_process
    class Exited:
        def __init__(self, pid): pass
        def create_time(self): return 7.0
        def status(self): return psutil.STATUS_RUNNING
        def num_threads(self): return 0
    monkeypatch.setattr(runtime_process.psutil, 'Process', Exited)
    monkeypatch.setattr(runtime_process.sys, 'platform', 'win32')
    assert runtime_process._same_process(1, 7.0) is False
    monkeypatch.setattr(runtime_process.sys, 'platform', 'linux')
    assert runtime_process._same_process(1, 7.0) is True


def test_job_cleanup_waits_for_every_member_to_leave_after_the_count_drops(monkeypatch):
    from app import runtime_process
    from app.runtime_process import Containment, cleanup
    events, alive = [], iter([True, True, False])
    class Job:
        def members(self): return [123]
        def terminate(self): events.append('terminate')
        def active(self): return 0
        def close(self): events.append('close')
    class Process:
        def __init__(self, pid): pass
        def create_time(self): return 5.0
    monkeypatch.setattr(runtime_process.psutil, 'Process', Process)
    monkeypatch.setattr(runtime_process, '_same_process', lambda pid, birth: next(alive))
    holder = type('Contained', (), {'runtime_containment': Containment(1, 1.0, Job()), 'poll': lambda self: None})()
    assert cleanup(holder, timeout=3) is True
    assert events == ['terminate', 'close'] and next(alive, 'gasto') == 'gasto'


def test_child_cleanup_after_abrupt_parent_death(tmp_path):
    from app.runtime_process import spawn_contained, cleanup
    pid_path = tmp_path / 'child.pid'
    script = "import subprocess,sys,time; p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); open(sys.argv[1],'w').write(str(p.pid)); time.sleep(60)"
    proc = spawn_contained([sys.executable, '-c', script, str(pid_path)], env=dict(os.environ))
    try:
        child = psutil.Process(_wait_pid(pid_path))
        assert child.is_running()
        proc.kill()
        proc.wait(5)
        assert cleanup(proc, timeout=3) is True
        assert _ended(child)
        assert proc.runtime_containment.cleaned
    finally:
        cleanup(proc, timeout=3)


@pytest.mark.skipif(sys.platform == 'win32', reason='POSIX: grupo auxiliar dentro da sessão contida')
def test_cleanup_contains_separate_auxiliary_group_in_same_rust_session(tmp_path):
    from app.runtime_process import spawn_contained, cleanup, refresh_members
    pid_path = tmp_path / 'auxiliary.pid'
    script = "import subprocess,sys,time; p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'],process_group=0);open(sys.argv[1],'w').write(str(p.pid));time.sleep(60)"
    proc = spawn_contained([sys.executable, '-c', script, str(pid_path)], env=dict(os.environ), record_path=tmp_path / 'containment.json')
    other = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(60)'])
    try:
        auxiliary = psutil.Process(_wait_pid(pid_path))
        assert os.getpgid(auxiliary.pid) != proc.pid and os.getsid(auxiliary.pid) == proc.pid
        refresh_members(proc)
        assert str(auxiliary.pid) in proc.runtime_containment.members
        proc.kill(); proc.wait(5)
        assert cleanup(proc, timeout=3) is True
        assert not auxiliary.is_running() or auxiliary.status() == psutil.STATUS_ZOMBIE
        assert other.poll() is None
    finally:
        cleanup(proc, timeout=3)
        other.kill(); other.wait(5)


@pytest.mark.skipif(sys.platform != 'win32', reason='Windows Job real no CI')
def test_windows_job_contains_grandchild_before_resume(tmp_path):
    test_child_cleanup_after_abrupt_parent_death(tmp_path)


def test_restart_after_fake_backend_and_rust_death_cleans_old_writer(tmp_path):
    import json
    from app import runtime_process
    record=tmp_path/'containment.json'
    child_file=tmp_path/'child.pid'
    ready=tmp_path/'ready.json'
    fake_backend=tmp_path/'backend.py'
    # O sinal de pronto é atômico: o teste só espera o arquivo existir e lê em seguida.
    fake_backend.write_text('''import json,os,sys,time
from pathlib import Path
from app.runtime_process import spawn_contained, refresh_members
script="import subprocess,sys,time; p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); open(sys.argv[1],'w').write(str(p.pid)); time.sleep(60)"
p=spawn_contained([sys.executable,'-c',script,sys.argv[2]],env=dict(os.environ),record_path=Path(sys.argv[1]))
while not Path(sys.argv[2]).exists() or not Path(sys.argv[2]).read_text():time.sleep(.01)
refresh_members(p)
ready=Path(sys.argv[3])
ready.with_suffix('.tmp').write_text(json.dumps({'rust':p.pid,'child':int(Path(sys.argv[2]).read_text())}))
os.replace(ready.with_suffix('.tmp'),ready)
time.sleep(60)
''')
    backend=subprocess.Popen([sys.executable,str(fake_backend),str(record),str(child_file),str(ready)],
        env={**os.environ,'PYTHONPATH':str(Path(__file__).resolve().parents[1])})
    info=None
    try:
        deadline=time.monotonic()+6
        while not ready.exists() and backend.poll() is None and time.monotonic()<deadline:time.sleep(.01)
        assert ready.exists()
        info=json.loads(ready.read_text())
        # No Windows o python do venv é um lançador: o dono gravado é o processo filho dele.
        owner=json.loads(record.read_text())['owner_pid']
        backend.kill()
        backend.wait(5)
        try:psutil.Process(owner).kill();psutil.Process(owner).wait(5)
        except psutil.NoSuchProcess:pass
        try:psutil.Process(info['rust']).kill()
        except psutil.NoSuchProcess:pass
        assert runtime_process.reconcile_startup(record) is True
        # O filho pode sumir entre as consultas; sumir conta como limpo.
        try:alive=psutil.Process(info['child']).status()!=psutil.STATUS_ZOMBIE
        except psutil.NoSuchProcess:alive=False
        assert not alive
        assert not record.exists()
    finally:
        if backend.poll() is None:
            backend.kill()
            backend.wait(5)
        if record.exists():runtime_process.reconcile_startup(record)


def test_recycled_group_leader_birth_blocks_cleanup_without_killing(tmp_path):
    import json
    from app import runtime_process
    proc=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'],start_new_session=sys.platform!='win32')
    record=tmp_path/'containment.json'
    try:
        birth=psutil.Process(proc.pid).create_time()
        record.write_text(json.dumps({'version':1,'life':'test','platform':sys.platform,'owner_pid':999999999,
            'owner_birth':1,'pid':proc.pid,'birth':birth-1,'pgid':proc.pid,'members':{str(proc.pid):birth-1},
            'boot':runtime_process.boot_identity(),'job_name':None}))
        with pytest.raises(RuntimeError):runtime_process.reconcile_startup(record)
        assert proc.poll() is None and record.exists()
    finally:
        proc.kill()
        proc.wait(5)


def test_windows_jobs_keep_unique_and_explicit_names_with_fake_api(monkeypatch):
    import ctypes
    from app.runtime_process import WindowsJob
    calls=[]
    class Function:
        def __init__(self,name):self.name=name
        def __call__(self,*args):
            calls.append((self.name,args))
            return 100 if self.name in {'CreateJobObjectW','OpenJobObjectW'} else 1
    class Api:
        def __init__(self):self.functions={}
        def __getattr__(self,name):return self.functions.setdefault(name,Function(name))
    monkeypatch.setattr(ctypes,'WinDLL',lambda *args,**kwargs:Api(),raising=False)
    monkeypatch.setattr(ctypes,'get_last_error',lambda:0,raising=False)
    first,second=WindowsJob(),WindowsJob()
    supplied='Global\\Hangar-runtime-'+('a'*32)
    existing=WindowsJob(supplied,existing=True)
    try:
        assert first.name.startswith('Global\\Hangar-runtime-') and first.name!=second.name
        assert existing.name==supplied
        assert [args[2] for name,args in calls if name=='OpenJobObjectW']==[supplied]
    finally:
        first.close();second.close();existing.close()


def test_job_cleanup_terminates_even_when_members_cannot_be_listed(monkeypatch):
    from app.runtime_process import Containment, cleanup
    events = []
    class Job:
        def members(self): raise OSError('ERROR_MORE_DATA')
        def terminate(self): events.append('terminate')
        def active(self): return 0
        def close(self): events.append('close')
    holder = type('Contained', (), {'runtime_containment': Containment(1, 1.0, Job()), 'poll': lambda self: None})()
    assert cleanup(holder, timeout=3) is True
    assert events == ['terminate', 'close']


@pytest.mark.skipif(sys.platform == 'win32', reason='grupo de processo POSIX')
def test_full_disk_while_refreshing_the_record_leaves_no_temporary(tmp_path, monkeypatch):
    # O vigia repete a gravação a cada volta: cada ENOSPC que deixasse um temporário enchia mais o disco.
    from app import runtime_process
    from app.runtime_process import spawn_contained, cleanup, refresh_members
    record = tmp_path / 'containment.json'
    proc = spawn_contained([sys.executable, '-c', 'import time;time.sleep(60)'], env=dict(os.environ), record_path=record)
    try:
        def full(fd):
            raise OSError(28, 'No space left on device')
        monkeypatch.setattr(runtime_process.os, 'fsync', full)
        proc.runtime_containment.members = {}  # um pid novo obriga a gravar
        for _ in range(3):
            with pytest.raises(OSError):
                refresh_members(proc)
        assert sorted(p.name for p in tmp_path.iterdir()) == ['containment.json', 'containment.lock']
    finally:
        monkeypatch.undo()
        proc.kill(); proc.wait(5)
        cleanup(proc, timeout=3)


def _count_writes(monkeypatch):
    from app import atomico
    writes = []
    real = atomico.substituir
    monkeypatch.setattr(atomico, 'substituir', lambda src, dst: (writes.append(dst), real(src, dst)))
    return writes


def _fake_job_holder(tmp_path, monkeypatch):
    from app import runtime_process
    from app.runtime_process import Containment
    class Job:
        name = 'Global\\Hangar-runtime-' + 'b' * 32
    monkeypatch.setattr(runtime_process, '_same_process', lambda pid, birth: True)
    containment = Containment(os.getpid(), 1.0, Job(), record=tmp_path / 'containment.json', life='t')
    return type('Contained', (), {'runtime_containment': containment})()


_SPAWN_ON_GO = ("import os,subprocess,sys,time\n"
    "while not os.path.exists(sys.argv[1]): time.sleep(.01)\n"
    "p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'])\n"
    "open(sys.argv[2],'w').write(str(p.pid)); time.sleep(60)")


@pytest.mark.skipif(sys.platform == 'win32', reason='grupo de processo POSIX')
def test_refresh_members_writes_only_when_members_change(tmp_path, monkeypatch):
    from app.runtime_process import spawn_contained, cleanup, refresh_members
    go, pid_path = tmp_path / 'go', tmp_path / 'child.pid'
    proc = spawn_contained([sys.executable, '-c', _SPAWN_ON_GO, str(go), str(pid_path)],
        env=dict(os.environ), record_path=tmp_path / 'containment.json')
    try:
        writes = _count_writes(monkeypatch)
        for _ in range(3):
            refresh_members(proc)
        assert writes == []
        go.touch()
        child = _wait_pid(pid_path)
        refresh_members(proc); refresh_members(proc)
        assert len(writes) == 1
        assert str(child) in proc.runtime_containment.members
    finally:
        monkeypatch.undo()
        proc.kill(); proc.wait(5)
        cleanup(proc, timeout=3)


@pytest.mark.skipif(not sys.platform.startswith('linux'), reason='descida por /proc só no Linux')
def test_refresh_members_follows_tree_without_full_scan(tmp_path, monkeypatch):
    from app import runtime_process
    from app.runtime_process import spawn_contained, cleanup, refresh_members
    go, pid_path = tmp_path / 'go', tmp_path / 'child.pid'
    proc = spawn_contained([sys.executable, '-c', _SPAWN_ON_GO, str(go), str(pid_path)],
        env=dict(os.environ), record_path=tmp_path / 'containment.json')
    try:
        go.touch()
        child = _wait_pid(pid_path)
        def no_scan(*a, **k):
            raise AssertionError('varreu a máquina inteira')
        monkeypatch.setattr(runtime_process.psutil, 'process_iter', no_scan)
        proc.runtime_containment.swept = runtime_process.time.monotonic()
        refresh_members(proc)
        assert str(child) in proc.runtime_containment.members
    finally:
        monkeypatch.undo()
        proc.kill(); proc.wait(5)
        cleanup(proc, timeout=3)


@pytest.mark.skipif(not sys.platform.startswith('linux'), reason='descida por /proc só no Linux')
def test_full_sweep_catches_orphan_in_session(tmp_path, monkeypatch):
    # O pai do neto morre: o neto sai da árvore do Rust, mas segue na sessão e escreve.
    from app import runtime_process
    from app.runtime_process import spawn_contained, cleanup, refresh_members
    go, pid_path = tmp_path / 'go', tmp_path / 'orphan.pid'
    script = ("import os,subprocess,sys,time\n"
        "while not os.path.exists(sys.argv[1]): time.sleep(.01)\n"
        "inner=\"import subprocess,sys;p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']);open(sys.argv[1],'w').write(str(p.pid))\"\n"
        "subprocess.run([sys.executable,'-c',inner,sys.argv[2]]); time.sleep(60)")
    proc = spawn_contained([sys.executable, '-c', script, str(go), str(pid_path)],
        env=dict(os.environ), record_path=tmp_path / 'containment.json')
    clock = [time.monotonic() + 1000]
    monkeypatch.setattr(runtime_process, 'time', type('Clock', (), {'monotonic': staticmethod(lambda: clock[0])}))
    try:
        refresh_members(proc)
        go.touch()
        orphan = _wait_pid(pid_path)
        deadline = time.time() + 5
        while psutil.Process(orphan).ppid() == proc.pid or any(
                c.pid != orphan for c in psutil.Process(proc.pid).children()):
            assert time.time() < deadline
            time.sleep(.01)
        clock[0] += 1
        refresh_members(proc)
        assert str(orphan) not in proc.runtime_containment.members
        clock[0] += 5
        refresh_members(proc)
        assert str(orphan) in proc.runtime_containment.members
    finally:
        monkeypatch.undo()
        proc.kill(); proc.wait(5)
        cleanup(proc, timeout=3)


def test_windows_job_refresh_writes_once(tmp_path, monkeypatch):
    from app.runtime_process import refresh_members
    holder = _fake_job_holder(tmp_path, monkeypatch)
    writes = _count_writes(monkeypatch)
    for _ in range(4):
        refresh_members(holder)
    assert len(writes) == 1


def test_refresh_members_retries_after_failed_write(tmp_path, monkeypatch):
    from app import runtime_process
    from app.runtime_process import refresh_members
    holder = _fake_job_holder(tmp_path, monkeypatch)
    writes = _count_writes(monkeypatch)
    real_fsync = os.fsync
    failures = iter([True, True])
    def fsync(fd):
        if next(failures, False):
            raise OSError(28, 'No space left on device')
        real_fsync(fd)
    monkeypatch.setattr(runtime_process.os, 'fsync', fsync)
    for _ in range(2):
        with pytest.raises(OSError):
            refresh_members(holder)
    refresh_members(holder); refresh_members(holder)
    assert len(writes) == 1 and (tmp_path / 'containment.json').exists()


@pytest.mark.skipif(sys.platform == 'win32', reason='grupo de processo POSIX')
def test_reconcile_still_finds_orphan_grandchild(tmp_path):
    # O registro só tem o Rust: o neto órfão nunca entrou nele e mesmo assim a reconciliação o acha.
    import json
    from app import runtime_process
    pid_path = tmp_path / 'orphan.pid'
    inner = "import subprocess,sys;p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']);open(sys.argv[1],'w').write(str(p.pid))"
    rust = subprocess.Popen([sys.executable, '-c', f"import subprocess,sys,time;subprocess.run([sys.executable,'-c',{inner!r},sys.argv[1]]);time.sleep(60)", str(pid_path)],
        start_new_session=True)
    record = tmp_path / 'containment.json'
    try:
        orphan = psutil.Process(_wait_pid(pid_path))
        birth = psutil.Process(rust.pid).create_time()
        record.write_text(json.dumps({'version': 1, 'life': 'test', 'platform': sys.platform, 'owner_pid': 999999999,
            'owner_birth': 1, 'pid': rust.pid, 'birth': birth, 'pgid': rust.pid, 'members': {str(rust.pid): birth},
            'boot': runtime_process.boot_identity(), 'job_name': None}))
        assert runtime_process.reconcile_startup(record) is True
        assert _ended(orphan)
    finally:
        rust.kill(); rust.wait(5)


def test_record_deleted_from_outside_is_rewritten(tmp_path, monkeypatch):
    from app.runtime_process import refresh_members
    holder = _fake_job_holder(tmp_path, monkeypatch)
    refresh_members(holder)
    (tmp_path / 'containment.json').unlink()
    refresh_members(holder)
    assert (tmp_path / 'containment.json').exists()


@pytest.mark.skipif(not sys.platform.startswith('linux'), reason='descida por /proc só no Linux')
def test_kernel_without_children_falls_back_to_full_sweep(tmp_path, monkeypatch):
    from app import runtime_process
    from app.runtime_process import spawn_contained, cleanup, refresh_members
    go, pid_path = tmp_path / 'go', tmp_path / 'child.pid'
    proc = spawn_contained([sys.executable, '-c', _SPAWN_ON_GO, str(go), str(pid_path)],
        env=dict(os.environ), record_path=tmp_path / 'containment.json')
    try:
        go.touch()
        child = _wait_pid(pid_path)
        real_exists, real_read = Path.exists, Path.read_text
        def read(self, *a, **k):
            if self.name == 'children':
                raise FileNotFoundError(self)
            return real_read(self, *a, **k)
        monkeypatch.setattr(Path, 'exists', lambda self: False if self.name == 'children' else real_exists(self))
        monkeypatch.setattr(Path, 'read_text', read)
        proc.runtime_containment.swept = runtime_process.time.monotonic()
        refresh_members(proc)
        assert str(child) in proc.runtime_containment.members
    finally:
        monkeypatch.undo()
        proc.kill(); proc.wait(5)
        cleanup(proc, timeout=3)
