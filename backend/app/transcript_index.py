from typing import Optional

_current: Optional[Index] = None


def current() -> Optional[Index]:
    return _current
