"""Clientes de prova sobre as rotas e os arquivos nativos da orquestração."""
import json
import os
from pathlib import Path
import shutil
import time
from urllib.parse import quote

from orq_contract_client import call


class NativeResponse:
    def __init__(self, status, body):
        self.status_code = status
        self._body = body
        self.text = json.dumps(body, ensure_ascii=False)

    def json(self):
        return self._body


class NativeClient:
    def __init__(self, directory, monkeypatch):
        self.home = directory / "home"
        self.base = self.home / ".claude"
        self.pair = self.base / ".hangar-pair"
        self.pair.mkdir(parents=True)
        self.rules = self.pair / "regras-g1.md"
        self.policy = self.home / ".hangar/orquestracao-contas.md"
        account = self.home / ".claude-200-01"
        account.mkdir()
        (account / ".hangar-models.json").write_text(json.dumps({
            "ts": time.time(), "resp": {"models": [{"id": "opus[1m]"}, {"id": "sonnet"}]}}), encoding="utf-8")
        kimi = self.home / ".kimi-code"
        kimi.mkdir()
        (kimi / "config.toml").write_text(
            '[providers.apikey]\ntype="kimi"\napi_key="synthetic"\nbase_url="https://example.invalid"\n'
            '[models."apikey/k3"]\nmodel="k3"\nprovider="apikey"\n', encoding="utf-8")
        self.sidecars = self.home / ".hangar/claude-headless"
        self.sidecars.mkdir(parents=True)
        for index, name in enumerate(("arb", "exec", "solta")):
            (self.sidecars / f"{name}.json").write_text(json.dumps({
                "name": name, "session_id": f"fixture-{name}", "key": f"key-{name}",
                "cwd": str(directory), "last_activity": float(index + 1)}), encoding="utf-8")
        for name, peer in (("exec", "arb"), ("arb", "exec")):
            (self.pair / f"{name}.json").write_text(json.dumps({"gid": "g1", "peers": [peer]}), encoding="utf-8")
        git = shutil.which("git")
        assert git, "a prova exige Git instalado"
        programs = str(Path(git).parent)
        root = Path(__file__).resolve().parents[2]
        self.mux = Path(os.environ.get("HANGAR_ORQ_CONTRACT_BIN") or root / "crates/target/debug/examples" / ("orq_contract.exe" if os.name == "nt" else "orq_contract"))
        monkeypatch.setenv("PATH", programs)
        monkeypatch.setenv("HOME", str(self.home))
        monkeypatch.setenv("USERPROFILE", str(self.home))
        for key, name in (("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"),
                          ("XDG_STATE_HOME", ".local/state"), ("XDG_CACHE_HOME", ".cache"),
                          ("LOCALAPPDATA", "localappdata")):
            monkeypatch.setenv(key, str(self.home / name))
        account_programs = directory / "account-programs"
        account_programs.mkdir()
        self.env = {"HOME": str(self.home), "USERPROFILE": str(self.home),
                    "PATH": str(account_programs), "CP_CLAUDE_CONFIG_DIRS": f"Rafael:{account}"}
        self.dirs = {"home": str(self.home), "claude": str(self.base),
                     "codex_home": str(self.home / ".codex"), "pi_sessions": str(self.home / ".pi/sessions"),
                     "omp_config": str(self.home / ".omp"), "omp_agent": str(self.home / ".omp/agent"),
                     "kimi_home": str(kimi)}
        self.enviados = []
        self.public = False
        self.upstream_response = {"status": 200, "body": {"ok": True, "delivered": True}}
        self.deliveries = []
        self.poll_panel = False

    def request(self, method, path, *, json=None, headers=None):
        result = call("http.public" if self.public else "http.request", method=method, path=quote(path, safe="/%?=&"), body=json,
                      home=self.home, env=self.env, dirs=self.dirs, mux=str(self.mux),
                      upstream_response=self.upstream_response, authenticated=headers is not None,
                      state_only=path.endswith("/events"), poll_panel=self.poll_panel)
        self.deliveries.extend(result["deliveries"])
        self.enviados.extend((row["path"].split("/")[-2], row["body"]["text"])
                             for row in result["deliveries"] if row["method"] == "POST" and row["path"].endswith("/input"))
        return NativeResponse(result["status"], result["body"])

    def get(self, path, **kwargs): return self.request("GET", path, **kwargs)
    def put(self, path, **kwargs): return self.request("PUT", path, **kwargs)
    def post(self, path, **kwargs): return self.request("POST", path, **kwargs)
    def delete(self, path, **kwargs): return self.request("DELETE", path, **kwargs)


class NativePlan:
    @staticmethod
    def _orq(): return NativePlan
    @staticmethod
    def plan_sha(text): return call("plan.sha", text=text)
    @staticmethod
    def find_plan(repo): return call("plan.find", repo=repo)
    @staticmethod
    def readiness(cwd, grouped, roles): return call("plan.readiness", cwd=cwd, grouped=grouped, roles=roles)
    @staticmethod
    def kickoff(readiness, contract): return call("plan.kickoff", readiness=readiness, contract=contract)
