"""Confere o scripts/check-rules.py num repositório descartável.

Uso: python3 -I scripts/test_check_rules.py
"""
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

CHECK = Path(__file__).resolve().parent / "check-rules.py"
ENV = {
    **{k: v for k, v in os.environ.items() if not k.startswith("GIT_")},
    "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t",
}


class CheckRules(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.repo = Path(folder.name)
        self.git("init", "-q", "-b", "main")
        self.put("backend/app/x.py", "a = 1\nb = 2\n")
        self.put("crates/hangar-server/Cargo.toml", '[package]\nname = "hangar-server"\n')
        self.put("crates/hangar-core/Cargo.toml", '[package]\nname = "hangar-core"\n[dependencies]\nserde = "1"\n')
        self.put("desktop-native/Cargo.toml", '[package]\nname = "hangar-native"\n')
        self.commit("base")
        self.base = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.repo, env=ENV, check=True, capture_output=True, text=True).stdout

    def put(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "--no-verify", "-m", message)

    def check(self):
        done = subprocess.run([sys.executable, "-I", str(CHECK), self.base, "HEAD"], cwd=self.repo, env=ENV, capture_output=True, text=True)
        return done.returncode, done.stdout + done.stderr

    def test_a_clean_change_passes(self):
        self.put("docs/x.md", "oi\n")
        self.commit("docs")
        self.assertEqual(self.check()[0], 0)

    def test_a_new_python_line_in_the_backend_is_refused(self):
        self.put("backend/app/x.py", "a = 1\nb = 2\nc = 3\n")
        self.commit("add")
        code, output = self.check()
        self.assertEqual(code, 1, output)
        self.assertIn("backend/app/x.py", output)

    def test_a_python_file_moved_into_the_backend_is_refused(self):
        self.put("tools/y.py", "a = 1\nb = 2\n")
        self.commit("outside")
        self.base = self.git("rev-parse", "HEAD").strip()
        self.git("mv", "tools/y.py", "backend/app/y.py")
        self.commit("move")
        self.assertEqual(self.check()[0], 1)

    def test_deleting_python_passes(self):
        self.put("backend/app/x.py", "a = 1\n")
        self.commit("remove")
        self.assertEqual(self.check()[0], 0)

    def test_comments_and_blank_lines_do_not_count(self):
        self.put("backend/app/x.py", "a = 1\n\n# por que\nb = 2\n")
        self.commit("comment")
        self.assertEqual(self.check()[0], 0)

    def test_the_mark_with_a_reason_authorizes_python(self):
        self.put("backend/app/x.py", "a = 1\nb = 2\nc = 3\n")
        self.commit("add\n\nHANGAR_PYTHON_AUTORIZADO: pedido do dono")
        self.assertEqual(self.check()[0], 0)

    def test_the_mark_without_a_reason_does_not_authorize(self):
        self.put("backend/app/x.py", "a = 1\nb = 2\nc = 3\n")
        self.commit("add\n\nHANGAR_PYTHON_AUTORIZADO:")
        self.assertEqual(self.check()[0], 1)

    def test_the_base_moving_on_does_not_count_as_the_branch(self):
        self.git("checkout", "-q", "-b", "branch")
        self.put("docs/x.md", "oi\n")
        self.commit("docs")
        self.git("checkout", "-q", "main")
        self.put("backend/app/x.py", "a = 1\n")
        self.commit("main removes")
        self.base = self.git("rev-parse", "HEAD").strip()
        self.git("checkout", "-q", "branch")
        self.assertEqual(self.check()[0], 0)

    def test_a_crate_depending_on_the_server_is_refused_in_every_form(self):
        forms = [
            '[dependencies]\nhangar-server = { path = "../hangar-server" }\n',
            '[dependencies]\nhangar-server.workspace = true\n',
            '[dev-dependencies]\nsrv = { package = "hangar-server", path = "../hangar-server" }\n',
            '[target."cfg(unix)".build-dependencies]\nhangar-server = "1"\n',
        ]
        for form in forms:
            with self.subTest(form):
                self.put("crates/hangar-core/Cargo.toml", '[package]\nname = "hangar-core"\n' + form)
                self.commit("dep " + form)
                code, output = self.check()
                self.assertEqual(code, 1, output)
                self.assertIn("crates/hangar-core/Cargo.toml", output)

    def test_the_desktop_depending_on_the_server_is_refused(self):
        self.put("desktop-native/Cargo.toml", '[dependencies]\nhangar-server = { path = "../crates/hangar-server" }\n')
        self.commit("dep")
        self.assertEqual(self.check()[0], 1)

    def test_an_unreadable_base_is_an_error_and_not_a_pass(self):
        self.base = "0" * 40
        self.assertEqual(self.check()[0], 2)


if __name__ == "__main__":
    unittest.main()
