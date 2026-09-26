"""Alvos sintéticos do controle do Windows; nunca executa SSH nem grava fora da fixture.

GET /control/t46 aceita reset=1, test=ok|failed, setup=ok|error e
create=ok|exists|error. O token é o mesmo sintético da fixture-base.
"""
import json
import pathlib
import re
import threading
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task44_windows_fixture.py").read_text(encoding="utf-8")
T44 = {"__name__": "task44_base", "__file__": str(HERE / "task44_windows_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nif __name__ ==")], "task44_windows_fixture.py", "exec"), T44)
LOCK = threading.Lock()
BASE_SNAPSHOT = T44["snapshot"]
ERROR = T44["error"]
CONTROL = {"test": "ok", "setup": "ok", "create": "ok"}
TARGETS = []


def snapshot(skipped=None):
    body = BASE_SNAPSHOT(skipped)
    if body["targets"]:
        root = "/fixture/computer-control/targets" if T44["STATE"]["mode"] == "package" else T44["STATE"]["project_dir"]
        body["targets"][0]["path"] = f"{root}/delphi-02-agent.json"
    body["targets"].extend(TARGETS)
    body["agent_configs"] = [target["path"] for target in body["targets"]]
    return body


T44["snapshot"] = snapshot


PROMPT = """Configure este Windows para ser controlado pelo Hangar (MCP hangar-computer-control) a partir de outra máquina, por SSH com chave. Faça na ordem, confira cada passo e pare pra me perguntar se algo não bater.

1. Rode tudo num PowerShell elevado (como Administrador). Se não estiver elevado, me peça pra abrir um.
2. OpenSSH Server instalado, ligado e iniciando sozinho:
   Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
   Set-Service -Name sshd -StartupType Automatic; Start-Service sshd
3. Porta 22 liberada no firewall, se ainda não houver regra de entrada pra ela:
   New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
4. Autorize esta chave pública pro usuário {user_line}:
   {key}
   - Usuário administrador: a chave vai em C:\\ProgramData\\ssh\\administrators_authorized_keys, com permissão só pra Administradores e SYSTEM (use os SIDs, que valem em Windows de qualquer idioma):
     icacls.exe C:\\ProgramData\\ssh\\administrators_authorized_keys /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F'
   - Usuário comum: em C:\\Users\\<usuário>\\.ssh\\authorized_keys.
   Acrescente sem apagar as chaves que já estiverem lá.
5. Esse usuário precisa ser administrador (o agente sobe como tarefa agendada elevada) e ficar logado numa sessão gráfica ATIVA: tela desbloqueada e sessão RDP não desconectada. Pra deixar a sessão ativa sem cliente RDP: tscon <id> /dest:console.
6. Confira: o serviço sshd está Running, a regra de firewall existe e o arquivo de chaves contém a chave acima.

Não mude outras configurações de segurança além dessas. No fim, me diga o nome desta máquina na rede (ou o IP) e o usuário, pra eu cadastrar no Hangar."""


class Handler(T44["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t46":
            query = parse_qs(url.query)
            allowed = {"reset": {"1"}, "test": {"ok", "failed"},
                       "setup": {"ok", "error"}, "create": {"ok", "exists", "error"}}
            if any(key not in allowed or len(values) != 1 or values[0] not in allowed[key] for key, values in query.items()):
                return self.send_json({"detail": "controle inválido"}, 400)
            with LOCK:
                if "reset" in query:
                    CONTROL.update(test="ok", setup="ok", create="ok")
                    TARGETS.clear()
                    T44["STATE"].clear()
                    T44["STATE"].update(T44["initial"]())
                for key in ("test", "setup", "create"):
                    if key in query:
                        CONTROL[key] = query[key][0]
                body = snapshot()
            return self.send_json(body)
        if url.path == "/api/computer-control/windows-setup":
            if not self.authorized():
                return
            host = parse_qs(url.query).get("host", [""])[0].strip() or "novo-windows"
            print("GET /api/computer-control/windows-setup", flush=True)
            if CONTROL["setup"] == "error":
                return self.send_json(ERROR("erro_computer_control_no_ssh_key",
                    "esta máquina não tem chave SSH: crie uma com ssh-keygen -t ed25519"), 400)
            if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9._@-]*", host):
                return self.send_json(ERROR("erro_computer_control_target_host", "informe o host SSH (sem espaços)"), 400)
            user = host.split("@", 1)[0] if "@" in host else "fixture-user"
            line = f"{user} (é o usuário com que o Hangar vai entrar; se ele não existir aqui, use o administrador logado e me diga qual é)"
            return self.send_json({"prompt": PROMPT.format(user_line=line, key="ssh-ed25519 AAAAfixture-only fixture@hangar"), "user": user})
        return super().do_GET()

    def do_POST(self):
        path = urlparse(self.path).path
        if path not in ("/api/computer-control/test-host", "/api/computer-control/targets"):
            return super().do_POST()
        if not self.authorized():
            return
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
        if path.endswith("test-host"):
            print("POST /api/computer-control/test-host synthetic", flush=True)
            host = str(body.get("host") or "").strip()
            if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9._@-]*", host):
                return self.send_json(ERROR("erro_computer_control_target_host", "informe o host SSH (sem espaços)"), 400)
            return self.send_json({"ok": CONTROL["test"] == "ok", "detail": "" if CONTROL["test"] == "ok" else "sem resposta em 20 s"})
        print("POST /api/computer-control/targets synthetic", flush=True)
        if CONTROL["create"] == "error":
            directory = "/fixture/computer-control/targets" if T44["STATE"]["mode"] == "package" else str(body.get("project_dir") or "/fixture/project").strip()
            return self.send_json(ERROR("erro_computer_control_dir", f"{directory} não tem servidor_mcp.py e .venv/bin/python",
                dir=directory), 400)
        name = str(body.get("name") or "").strip()
        if not name and body.get("transport") == "ssh":
            name = re.sub(r"[^a-z0-9._-]+", "-", str(body.get("host") or "").split("@")[-1].lower()).strip("-._")[:41]
        if not re.fullmatch(r"[a-z0-9][a-z0-9._-]{0,40}", name):
            return self.send_json(ERROR("erro_computer_control_target_name",
                "nome do alvo: letras minúsculas, números, ponto, hífen ou sublinhado"), 400)
        if CONTROL["create"] == "exists" or any(t["name"] == name for t in snapshot()["targets"]):
            return self.send_json(ERROR("erro_computer_control_target_exists", f"o alvo {name} já existe", name=name), 409)
        if body.get("transport") == "local":
            return self.send_json(ERROR("erro_computer_control_local_only_windows",
                "este computador só pode ser alvo quando o Hangar roda no Windows"), 400)
        host = str(body.get("host") or "").strip()
        if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9._@-]*", host):
            return self.send_json(ERROR("erro_computer_control_target_host", "informe o host SSH (sem espaços)"), 400)
        root = "/fixture/computer-control/targets" if T44["STATE"]["mode"] == "package" else body.get("project_dir", "/fixture/project")
        TARGETS.append({"name": name, "path": f"{root}/{name}-agent.json", "transport": "ssh", "host": host})
        self.send_json(snapshot())


if __name__ == "__main__":
    T44["T43"]["Handler"] = Handler
    T44["T43"]["main"]()
