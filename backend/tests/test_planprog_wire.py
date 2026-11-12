from app.models import SessionInfo


def test_campos_no_modelo():
    i = SessionInfo(name="s")
    assert i.plan_name is None and i.plan_done is None and i.plan_tasks is None
