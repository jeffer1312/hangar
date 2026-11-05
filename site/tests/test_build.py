import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

SITE = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SITE))
sys.path.insert(0, str(SITE / "src"))

import build  # noqa: E402
import strings  # noqa: E402

# home real, IP e host da tailnet; /home/dev/ e fixture.sintetico.ts.net são dado sintético
MACHINE = re.compile(r"/home/(?!dev/)[a-z]|/Users/[A-Za-z]|C:\\{1,2}Users\\|/tmp/claude-|(?<!fixture\.sintetico)\.ts\.net"
                     r"|100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.\d+\.\d+")
LEFTOVER = re.compile(r"«[^»]*»|\{\{|<sc-(for|if)|<dc-import|<x-dc|<helmet|/_blob/|\bonClick=|\bref=")


class BuildTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out = pathlib.Path(self.tmp.name)
        self.pages = build.build(self.out)

    def tearDown(self):
        self.tmp.cleanup()

    def test_writes_pt_and_en(self):
        self.assertEqual({p.relative_to(self.out).as_posix() for p in self.pages}, {"index.html", "en/index.html"})
        self.assertIn('<html lang="pt-BR"', (self.out / "index.html").read_text(encoding="utf-8"))
        self.assertIn('<html lang="en"', (self.out / "en/index.html").read_text(encoding="utf-8"))

    def test_no_template_syntax_left(self):
        for page in self.pages:
            m = LEFTOVER.search(page.read_text(encoding="utf-8"))
            self.assertIsNone(m, f"{page}: {m and m.group(0)}")

    def test_every_local_asset_exists(self):
        for page in self.pages:
            for ref in re.findall(r'(?:src|href|poster|data-src|data-poster)="(/[^"#]*)"', page.read_text(encoding="utf-8")):
                target = self.out / ref.lstrip("/")
                if ref.endswith("/"):
                    target = target / "index.html"
                self.assertTrue(target.exists(), f"{page}: {ref}")

    def test_hreflang_both_ways(self):
        for page in self.pages:
            html = page.read_text(encoding="utf-8")
            self.assertIn('hreflang="pt-BR" href="https://hangar.dev.br/"', html)
            self.assertIn('hreflang="en" href="https://hangar.dev.br/en/"', html)

    def test_same_structure_in_both_languages(self):
        def tags(p):
            return re.findall(r"<(section|article|video|button|a)\b", p.read_text(encoding="utf-8"))
        self.assertEqual(tags(self.out / "index.html"), tags(self.out / "en/index.html"))

    def test_every_string_has_both_languages(self):
        for key, pair in strings.T.items():
            self.assertEqual(len(pair), 2, key)
            self.assertTrue(pair[0] and pair[1], key)

    def test_no_machine_data_in_site_tree(self):
        # só o versionado: rascunho fora do git (.wrangler/, node_modules/) não publica nem reprova
        root = SITE.parent
        tracked = subprocess.run(["git", "ls-files", "site"], cwd=root, capture_output=True, text=True, check=True).stdout.splitlines()
        for f in (root / name for name in tracked):
            parts = set(f.relative_to(SITE).parts)
            if not f.is_file() or parts & {"_fonte", "public", "media", "out", "cfg"} or f.name == "test_build.py":
                continue
            if f.suffix in {".html", ".py", ".js", ".css", ".sh", ".json", ".md", ".mjs", ".txt"}:
                m = MACHINE.search(f.read_text(encoding="utf-8"))
                self.assertIsNone(m, f"{f}: {m and m.group(0)}")

    def test_every_agent_has_a_banner(self):
        for page in ("index.html", "en/index.html"):
            html = (self.out / page).read_text(encoding="utf-8")
            flags = html[html.index('<ol class="flags"'):html.index("</ol>", html.index('<ol class="flags"'))]
            for _, name in strings.AGENTS:
                self.assertIn(f"<span>{name}</span>", flags)

    def test_pt_uses_pt_videos(self):
        pt = (self.out / "index.html").read_text(encoding="utf-8")
        en = (self.out / "en/index.html").read_text(encoding="utf-8")
        self.assertIn('data-src="/assets/media/pt/agents.mp4"', pt)
        self.assertIn('class="shot loop" src="/assets/media/pt/orq.mp4"', pt)
        self.assertIn('class="shot loop" src="/assets/media/pt/pair.mp4"', pt)
        self.assertIn('data-src="/assets/media/agents.mp4"', en)
        self.assertNotIn("/assets/media/pt/", en)


if __name__ == "__main__":
    unittest.main()
