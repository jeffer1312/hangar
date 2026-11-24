"""Contrato público do CLI da orquestração com o armazenamento real do Rust."""

import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path


REPO = Path(__file__).resolve().parents[2]
ORQ = REPO / "skills/orquestrar/scripts/orq.py"
BINARY = REPO / "crates/target/debug" / ("hangar-server.exe" if os.name == "nt" else "hangar-server")


class OrqStorageCliTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(BINARY.is_file(), "compile hangar-server antes desta prova")
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.env = {**os.environ, "HOME": str(self.home), "USERPROFILE": str(self.home),
                    "CP_RUST_SERVER_BIN": str(BINARY), "GIT_CONFIG_GLOBAL": os.devnull}
        self.env.pop("ORQ_DIR", None)

    def run_cli(self, *args):
        return subprocess.run([sys.executable, str(ORQ), *map(str, args)], env=self.env,
                              capture_output=True, text=True, encoding="utf-8", timeout=30)

    def make_run(self, name="finished", ended=True):
        path = self.home / ".hangar/orq" / name
        path.mkdir(parents=True)
        (path / "orq.json").write_text('{"arbiter":"arbiter"}', encoding="utf-8")
        kind = "execucao_fim" if ended else "execucao_inicio"
        (path / "eventos.jsonl").write_text(json.dumps({"tipo": kind}) + "\n", encoding="utf-8")
        (path / "target").mkdir()
        (path / "target/output").write_bytes(b"build")
        (path / "registro.md").write_bytes(b"report")
        return path

    def test_preview_apply_and_check_preserve_the_evidence(self):
        path = self.make_run()
        preview = self.run_cli("--dir", path, "podar")
        self.assertEqual(preview.returncode, 0, preview.stderr)
        self.assertEqual(json.loads(preview.stdout)["report"]["paths"], ["target"])
        self.assertTrue((path / "target/output").exists())
        self.assertEqual(self.run_cli("--dir", path, "podar", "--check").returncode, 1)
        result = self.run_cli("--dir", path, "podar", "--apply")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((path / "target").exists())
        self.assertEqual((path / "registro.md").read_bytes(), b"report")
        self.assertEqual(self.run_cli("--dir", path, "podar", "--check").returncode, 0)

    def test_upgrade_cleans_old_runs_and_leaves_a_receipt_with_skips(self):
        old = self.make_run()
        active = self.make_run("active", ended=False)
        broken = self.make_run("broken")
        (broken / "eventos.jsonl").write_text("{broken", encoding="utf-8")
        result = self.run_cli("prune-archive", "--apply", "--receipt")
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads((self.home / ".hangar/orq-pruned-v1.json").read_text(encoding="utf-8"))
        self.assertEqual(receipt["bytes"], 5)
        self.assertEqual({row["run"] for row in receipt["runs"] if "skipped" in row}, {"active", "broken"})
        self.assertFalse((old / "target").exists())
        self.assertTrue((active / "target/output").exists())
        self.assertTrue((broken / "target/output").exists())
        self.assertEqual(self.run_cli("prune-archive", "--apply", "--receipt").returncode, 0)

    def test_failed_proof_discards_the_temporary_build(self):
        path = self.make_run(ended=False)
        code = (
            "import json, os, pathlib, sys; "
            "root = pathlib.Path(os.environ['ORQ_PROOF_ROOT']); "
            "target = pathlib.Path(os.environ['CARGO_TARGET_DIR']); "
            "(target / 'output').write_bytes(b'build'); "
            "(pathlib.Path(os.environ['ORQ_DIR']) / 'proof.json').write_text(json.dumps([str(root), str(target)])); "
            "sys.exit(7)"
        )
        result = self.run_cli("--dir", path, "proof", "--", sys.executable, "-c", code)
        self.assertEqual(result.returncode, 7, result.stderr)
        root, target = map(Path, json.loads((path / "proof.json").read_text(encoding="utf-8")))
        self.assertFalse(root.is_relative_to(path))
        self.assertTrue(target.is_relative_to(root))
        self.assertFalse(root.exists())

    @unittest.skipIf(os.name == "nt", "SIGHUP só existe no Unix")
    def test_hangup_on_the_cli_stops_the_proof_and_discards_the_build(self):
        path = self.make_run(ended=False)
        code = (
            "import json, os, sys, time; "
            "print(json.dumps([os.getpid(), os.environ['ORQ_PROOF_ROOT']]), flush=True); "
            "time.sleep(60)"
        )
        proc = subprocess.Popen([sys.executable, str(ORQ), "--dir", str(path), "proof", "--",
                                 sys.executable, "-c", code],
                                env=self.env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                text=True, encoding="utf-8")
        self.addCleanup(lambda: proc.poll() is None and proc.kill())
        ceiling = threading.Timer(30, proc.kill)
        ceiling.start()
        self.addCleanup(ceiling.cancel)
        assert proc.stdout is not None
        with proc.stdout as out:
            pid, root = json.loads(out.readline())
            self.addCleanup(lambda: self._kill_if_alive(pid))
            os.kill(proc.pid, signal.SIGHUP)
            proc.wait(timeout=30)
        self.assertFalse(Path(root).exists())
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)

    @staticmethod
    def _kill_if_alive(pid):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


if __name__ == "__main__":
    unittest.main()
