def test_sessioninfo_serializa_campos_diff():
    from app.models import SessionInfo
    s = SessionInfo(name="x", git_added=128, git_removed=24)
    d = s.model_dump()
    assert d["git_added"] == 128 and d["git_removed"] == 24


def test_sessioninfo_diff_default_none():
    from app.models import SessionInfo
    d = SessionInfo(name="x").model_dump()
    assert d["git_added"] is None and d["git_removed"] is None
