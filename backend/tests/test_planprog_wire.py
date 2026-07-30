from app import planprog
from app.models import SessionInfo
from app.registry import _decorate_plan
from app.sse import _list_sig


def _plan(**kw):
    base = dict(name="p", path="/tmp/p.md", task_idx=2, task_total=3,
                done=9, total=17, complete=False,
                tasks=(planprog.TaskProgress("Task 1", 5, 5, ()),
                       planprog.TaskProgress("Task 2", 4, 8, ()),
                       planprog.TaskProgress("Task 3", 0, 4, ())))
    base.update(kw)
    return planprog.PlanProgress(**base)


def test_campos_no_modelo():
    i = SessionInfo(name="s")
    assert i.plan_name is None and i.plan_done is None and i.plan_tasks is None


def test_decorate_plan_preenche(monkeypatch):
    monkeypatch.setattr("app.registry.plan_progress", lambda cwd: _plan())
    i = SessionInfo(name="s", cwd="/repo")
    _decorate_plan(i)
    assert i.plan_name == "p"
    assert (i.plan_task, i.plan_task_total) == (2, 3)
    assert (i.plan_done, i.plan_total) == (9, 17)
    assert i.plan_complete is False
    assert i.plan_tasks == [(5, 5), (4, 8), (0, 4)]


def test_decorate_plan_sem_plano_deixa_none(monkeypatch):
    monkeypatch.setattr("app.registry.plan_progress", lambda cwd: None)
    i = SessionInfo(name="s", cwd="/repo")
    _decorate_plan(i)
    assert i.plan_name is None and i.plan_done is None


def test_decorate_plan_nao_levanta(monkeypatch):
    def _boom(cwd):
        raise RuntimeError("nao pode vazar pro tick da lista")
    monkeypatch.setattr("app.registry.plan_progress", _boom)
    i = SessionInfo(name="s", cwd="/repo")
    _decorate_plan(i)
    assert i.plan_name is None


def test_sig_muda_quando_o_progresso_anda():
    a = SessionInfo(name="s", plan_name="p", plan_done=9, plan_total=17)
    b = SessionInfo(name="s", plan_name="p", plan_done=10, plan_total=17)
    assert _list_sig([a]) != _list_sig([b])


def test_sig_muda_quando_o_plano_troca_com_o_mesmo_numero():
    """Bug ja documentado no comentario do `engine` (sse.py:166): sem o NOME na sig, trocar de plano
    com o mesmo done/total nao re-emite e o chip fica preso no plano errado."""
    a = SessionInfo(name="s", plan_name="alpha", plan_done=9, plan_total=17)
    b = SessionInfo(name="s", plan_name="beta", plan_done=9, plan_total=17)
    assert _list_sig([a]) != _list_sig([b])


def test_sig_estavel_sem_plano():
    assert _list_sig([SessionInfo(name="s")]) == _list_sig([SessionInfo(name="s")])


def test_sig_muda_quando_so_plan_tasks_muda():
    """Um step desmarcado na Task 1 e outro marcado na Task 2 no mesmo write pode deixar
    plan_name/plan_done/plan_total identicos e ainda mudar a distribuicao — sem plan_tasks na sig
    a barra segmentada e a Task atual ficam presas na distribuicao velha."""
    a = SessionInfo(name="s", plan_name="p", plan_done=9, plan_total=17,
                     plan_tasks=[(5, 5), (4, 8), (0, 4)])
    b = SessionInfo(name="s", plan_name="p", plan_done=9, plan_total=17,
                     plan_tasks=[(4, 5), (5, 8), (0, 4)])
    assert _list_sig([a]) != _list_sig([b])
