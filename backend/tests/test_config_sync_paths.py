from app.config_sync_paths import (Roots, canonicalize, fix_programs, map_strings, mark,
                                   marked_paths, resolve)

SRC = Roots(hangar="/home/ana/hangar", claude="/home/ana/.claude", codex="/home/ana/.codex",
            home="/home/ana")
WIN = Roots(hangar="C:\\hangar", claude="C:\\Users\\bia\\.claude", codex="C:\\Users\\bia\\.codex",
            home="C:\\Users\\bia")
DST = Roots(hangar="/opt/hangar", claude="/home/bia/.claude", codex="/home/bia/.codex",
            home="/home/bia")
H, C, G = mark("HOME"), mark("CLAUDE"), mark("HANGAR")


def test_canonicalize_uses_most_specific_root():
    cmd = ("python3 /home/ana/.claude/hooks/x.py && node /home/ana/hangar/scripts/s.js "
           "/home/ana/.orca/h.sh")
    assert canonicalize(cmd, SRC) == (f"python3 {C}/hooks/x.py && node {G}/scripts/s.js "
                                      f"{H}/.orca/h.sh")


def test_canonicalize_respects_path_boundary():
    assert canonicalize("/home/ana2/x /home/ana", SRC) == f"/home/ana2/x {H}"


def test_canonicalize_windows_source_both_separators():
    assert canonicalize("C:\\Users\\bia\\x.py C:/Users/bia/y.py", WIN) == f"{H}\\x.py {H}/y.py"


def test_resolve_to_linux_target_turns_backslashes_into_slashes():
    assert resolve(f"{H}\\x.py '{C}/hooks/a b.sh'", DST) == \
        "/home/bia/x.py '/home/bia/.claude/hooks/a b.sh'"


def test_resolve_to_windows_target_uses_forward_slashes():
    assert resolve(f"{H}/.orca/h.sh", WIN) == "C:/Users/bia/.orca/h.sh"


def test_resolve_leaves_shell_variables_alone():
    assert resolve("echo ${HOME} {HOME}", DST) == "echo ${HOME} {HOME}"


def test_marked_paths_quoted_and_bare():
    cmd = (f"if [ -f '{H}/.orca/a b.sh' ]; then /bin/sh '{H}/.orca/a b.sh'; fi; "
           f"python3 {C}/hooks/x.py")
    assert marked_paths(cmd) == [f"{H}/.orca/a b.sh", f"{C}/hooks/x.py"]


def test_map_strings_walks_nested_values_and_keys():
    data = {"a": ["/x", {"b": "/y"}], "n": 1}
    assert map_strings(data, str.upper) == {"A": ["/X", {"B": "/Y"}], "N": 1}


def test_fix_programs_swaps_missing_interpreter_keeping_quotes():
    cmd = "'/home/bia/.local/share/fnm/v24/bin/node' '/opt/hangar/scripts/s.js'"
    new, missing = fix_programs(cmd, which={"node": "/usr/bin/node"}.get,
                                exists=lambda p: p == "/opt/hangar/scripts/s.js")
    assert new == "'/usr/bin/node' '/opt/hangar/scripts/s.js'"
    assert missing == []


def test_fix_programs_python3_falls_back_to_python():
    new, missing = fix_programs("/usr/bin/python3 x.py",
                                which=lambda n: "C:/Py/python.exe" if n == "python" else None,
                                exists=lambda p: False)
    assert new == "C:/Py/python.exe x.py" and missing == []


def test_fix_programs_reports_missing_bare_program():
    new, missing = fix_programs("rtk hook claude", which=lambda n: None, exists=lambda p: False)
    assert new == "rtk hook claude" and missing == ["rtk"]


def test_fix_programs_ignores_shell_words_and_assignments():
    _, missing = fix_programs("if [ -f x ]; then FOO=1 sh x; fi", which=lambda n: None,
                              exists=lambda p: False)
    assert missing == []
    _, missing = fix_programs("FOO=1 node x.js", which=lambda n: None, exists=lambda p: False)
    assert missing == []
