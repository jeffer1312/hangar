def test_sessioninfo_serializa_campos_git():
    from app.models import SessionInfo
    s = SessionInfo(name="x", git_dirty=3, git_ahead=2, git_behind=0)
    d = s.model_dump()
    assert d["git_dirty"] == 3 and d["git_ahead"] == 2 and d["git_behind"] == 0


def test_sessioninfo_git_default_none():
    from app.models import SessionInfo
    d = SessionInfo(name="x").model_dump()
    assert d["git_dirty"] is None and d["git_ahead"] is None and d["git_behind"] is None
