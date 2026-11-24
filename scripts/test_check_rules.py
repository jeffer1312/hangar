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

    def refused(self, name, text, mark):
        """A linha nova é recusada, a que já existia não, e a marca com motivo libera."""
        self.put(name, text)
        self.commit("add")
        code, output = self.check()
        self.assertEqual(code, 1, output)
        self.assertIn(f"HANGAR_{mark}_AUTORIZADO", output)
        self.assertIn(name, output)
        self.put("docs/x.md", "oi\n")
        self.commit(f"more\n\nHANGAR_{mark}_AUTORIZADO: pedido do dono")
        self.assertEqual(self.check()[0], 0)
        self.base = self.git("rev-parse", "HEAD").strip()
        self.put("docs/x.md", "oi de novo\n")
        self.put(name, text + "\n")
        self.commit("touch")
        self.assertEqual(self.check()[0], 0)

    def passes(self, name, text):
        self.put(name, text)
        self.commit("add")
        code, output = self.check()
        self.assertEqual(code, 0, output)

    def test_a_new_web_screen_is_refused_and_an_edited_one_passes(self):
        self.refused("frontend/src/components/New.svelte", "<p>oi</p>\n" * 5, "WEB")
        self.passes("frontend/src/components/New.svelte", "<p>tchau</p>\n")

    def test_a_moved_web_screen_passes(self):
        self.put("frontend/src/Old.svelte", "".join(f"<p>{n}</p>\n" for n in range(20)))
        self.commit("old")
        self.base = self.git("rev-parse", "HEAD").strip()
        (self.repo / "frontend/src/screens").mkdir()
        self.git("mv", "frontend/src/Old.svelte", "frontend/src/screens/Old.svelte")
        self.commit("move")
        self.assertEqual(self.check()[0], 0)

    def test_the_old_name_in_code_is_refused(self):
        self.refused("crates/hangar-core/src/lib.rs", 'const DIR: &str = ".claude-pocket";\n', "NOME_ANTIGO")

    def test_the_old_name_in_docs_comments_and_tests_passes(self):
        self.passes("docs/x.md", "era claude-pocket\n")
        self.passes("crates/hangar-core/src/lib.rs", "// vem do claude-pocket\n")
        self.passes("crates/hangar-core/tests/old.rs", 'let x = ".claude_pocket";\n')

    def test_the_react_native_keyboard_is_refused_in_one_or_many_lines(self):
        self.refused("mobile/app/a.tsx", "import { View, KeyboardAvoidingView } from 'react-native';\n", "TECLADO")
        self.git("rm", "-q", "mobile/app/a.tsx")
        self.commit("remove")
        self.base = self.git("rev-parse", "HEAD").strip()
        self.refused("mobile/app/b.tsx", "import {\n  View,\n  KeyboardAvoidingView,\n} from 'react-native';\n", "TECLADO")

    def test_the_keyboard_controller_passes(self):
        self.passes("mobile/app/a.tsx", "import { View } from 'react-native';\nimport { KeyboardAvoidingView } from 'react-native-keyboard-controller';\n")

    def test_vibration_outside_the_haptics_module_is_refused(self):
        self.refused("mobile/src/a.tsx", "import * as Haptics from 'expo-haptics';\n", "VIBRACAO")

    def test_a_new_vibration_call_in_a_file_that_already_imports_it_is_refused(self):
        self.refused("mobile/src/a.tsx", "void Haptics.selectionAsync();\n", "VIBRACAO")

    def test_vibration_in_the_haptics_module_and_in_tests_passes(self):
        self.passes("mobile/src/ui/haptics.ts", "import * as Haptics from 'expo-haptics';\nHaptics.selectionAsync();\n")
        self.passes("mobile/src/a.test.tsx", "vi.mock('expo-haptics', () => ({}));\n")
        self.passes("mobile/src/b.tsx", "haptics.selection();\n")

    def test_a_call_with_a_raw_base_url_is_refused_in_every_form(self):
        forms = [
            "fetch(`${s.baseUrl}/api/x`);\n",
            "fetch(s.baseUrl + '/api/x');\n",
            "new URL('/api/x', server.baseUrl);\n",
        ]
        for number, form in enumerate(forms):
            with self.subTest(form):
                self.refused(f"mobile/src/net/a{number}.ts", form, "ENDERECO")

    def test_a_base_url_that_is_identity_or_label_passes(self):
        self.passes("mobile/src/a.tsx", "key={`${s.id}:${s.baseUrl}:${s.token}`}\nconst u = `${getBaseUrl()}/api/x`;\n")

    def test_a_session_opened_outside_the_launcher_is_refused(self):
        self.refused("crates/hangar-server/src/other.rs", 'args = vec!["new-session", "-d"];\n', "LANCADOR")

    def test_the_launcher_and_its_tests_pass(self):
        self.passes("crates/hangar-server/src/session_open/terminal.rs", 'args = vec!["new-session", "-d"];\n')
        self.passes("crates/hangar-server/src/accounts/tests.rs", 'assert_eq!(args[0], "new-session");\n')

    def test_the_mark_copied_from_the_help_does_not_authorize(self):
        self.put("frontend/src/New.svelte", "<p>oi</p>\n")
        self.commit("add\n\nHANGAR_WEB_AUTORIZADO: <motivo>")
        self.assertEqual(self.check()[0], 1)

    def rebase_on(self, name, text):
        self.put(name, text)
        self.commit("before")
        self.base = self.git("rev-parse", "HEAD").strip()

    def test_a_reformatted_old_line_is_not_new(self):
        self.rebase_on("mobile/src/a.tsx", "import * as Haptics from 'expo-haptics';\n")
        self.passes("mobile/src/a.tsx", 'import * as Haptics from "expo-haptics";\n')

    def test_a_file_that_already_imported_the_keyboard_passes_an_edit(self):
        self.rebase_on("mobile/app/a.tsx", "import { KeyboardAvoidingView } from 'react-native';\n<View />\n")
        self.passes("mobile/app/a.tsx", "import { KeyboardAvoidingView } from 'react-native';\n<KeyboardAvoidingView />\n")

    def test_a_content_line_that_looks_like_a_header_keeps_the_file_name(self):
        self.refused("crates/hangar-core/src/a.rs", "++ x\nconst D: &str = \".claude-pocket\";\n", "NOME_ANTIGO")

    def test_a_local_base_already_resolved_passes(self):
        self.passes("packages/core/src/a.ts", "const baseUrl = baseOf(s);\nfetch(`${baseUrl}/api/x`);\n")

    def test_a_raw_base_url_joined_to_a_path_is_refused(self):
        for number, form in enumerate(["fetch(`${s.baseUrl}${path}`);\n", "fetch(s.baseUrl + path);\n", "new URL(path, server.baseUrl);\n"]):
            with self.subTest(form):
                self.refused(f"packages/core/src/b{number}.ts", form, "ENDERECO")

    def test_a_session_opened_by_a_whole_command_string_is_refused(self):
        self.refused("crates/hangar-core/src/a.rs", 'let c = format!("tmux new-session -d -s {n}");\n', "LANCADOR")

    def test_an_id_that_only_contains_new_session_passes(self):
        self.passes("crates/hangar-core/src/a.rs", 'let id = "launcher__new-session-sync";\nlet t = "new-session-x";\n')

    def test_an_unreadable_base_is_an_error_and_not_a_pass(self):
        self.base = "0" * 40
        self.assertEqual(self.check()[0], 2)


if __name__ == "__main__":
    unittest.main()
