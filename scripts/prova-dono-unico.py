#!/usr/bin/env python3
"""Prova de uso real do dono único (plano `docs/migracao-rust/dono-unico/plano.md`, Task 11).

Sobe o backend desta árvore isolado como unit transiente do systemd de usuário, roda pela API os
casos dos Steps 49–55, confere sozinho o que dá e imprime a tabela. No fim para tudo e apaga o
HOME temporário. Respostas sempre do Haiku, na conta 02-200 (e na `--conta-b` só na troca de conta).

    scripts/prova-dono-unico.py [--casos 49,50,51,52,53,54,55] [--n 10]
                                [--conta-b ~/.claude-outra]
                                [--relatorio arquivo.md] [--manter]

Isolamento: HOME, portas, token e servidor tmux (`-L`) próprios; `matar_orfaos` desligado (ele
varre o /proc do usuário e mataria canos reais); o `claude` é um embrulho que fixa o modelo e a
conta. As transcrições ficam na conta real (link `HOME/.claude/projects`), numa pasta só da prova,
apagada no fim. O relatório traz só marcadores, contagens e códigos: nenhum texto de conversa.

Ficam para a prova manual no app real: a transferência Claude → Codex (o backend isolado não vê a
conta Codex real sem gravar hooks e skills nela) e, quando sair inconclusiva, a entrega incerta
no terminal (congelar o agente faz o Rust adiar a entrada, não perder a prova do envio).
"""
import argparse
import http.client
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import uuid
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
BACKEND = REPO / "backend"
CONTA = Path.home() / ".claude-02-200"
CONTAS_PROIBIDAS = {".claude-claude-200-1", ".claude-claude-200-3"}
MODELO = "claude-haiku-4-5"
PORTAS_REAIS = {8765, 8766}
ENTREGA_S = 150
# Diário de sessão que troca de dono ou cuja abertura/fechamento no Rust falhou. Fica de fora
# `runtime.reopen_interrupted`: é a queda do Rust no meio da reabertura, e o resumo já a mostra.
PASSAGEM = {"runtime.unclaim_failed", "runtime.unclaim_skipped", "runtime.write_uncertain",
            "runtime.recover_failed", "runtime.reopen_failed", "runtime.close_unconfirmed"}
LONGO = "Escreva os números de 1 a 400 por extenso, um por linha, sem mais nada."


def esperar(cond, timeout, passo=0.5):
    fim = time.monotonic() + timeout
    while True:
        valor = cond()
        if valor or time.monotonic() >= fim:
            return valor
        time.sleep(passo)


def porta_livre():
    while True:
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            porta = s.getsockname()[1]
        if porta not in PORTAS_REAIS:
            return porta


def codigo(corpo):
    """Só o código do erro, nunca a frase (pode citar caminho ou texto)."""
    if isinstance(corpo, dict):
        d = corpo.get("detail", corpo)
        if isinstance(d, dict):
            return str(d.get("code") or d.get("error_code") or d.get("codigo") or "")
        return str(corpo.get("error_code") or "")
    return ""


def descendentes(raiz):
    filhos = {}
    for p in Path("/proc").iterdir():
        if p.name.isdigit():
            try:
                ppid = int((p / "stat").read_text().rsplit(")", 1)[1].split()[1])
            except (OSError, IndexError, ValueError):
                continue
            filhos.setdefault(ppid, []).append(int(p.name))
    saida, pilha = [], [raiz]
    while pilha:
        pid = pilha.pop()
        saida.append(pid)
        pilha.extend(filhos.get(pid, []))
    return saida


def sinalizar(pids, sinal):
    for pid in pids:
        try:
            os.kill(pid, sinal)
        except ProcessLookupError:
            pass


def exe(pid):
    try:
        return os.path.realpath(f"/proc/{pid}/exe")
    except OSError:
        return ""


class Linha:
    def __init__(self, step, caso, ok, evidencia):
        self.step, self.caso, self.ok, self.evidencia = step, caso, ok, evidencia


class Janela:
    """O que o log do Python e o diário ganharam desde a abertura."""

    def __init__(self, prova):
        self.p = prova
        self.log0 = prova.log.stat().st_size if prova.log.exists() else 0
        self.diario0 = {f: f.stat().st_size for f in prova.diarios()}

    def log(self):
        if not self.p.log.exists():
            return ""
        with self.p.log.open("rb") as f:
            f.seek(self.log0)
            return f.read().decode(errors="replace")

    def eventos(self):
        saida = []
        for f in self.p.diarios():
            with f.open("rb") as h:
                h.seek(self.diario0.get(f, 0))
                for bruta in h.read().decode(errors="replace").splitlines():
                    try:
                        o = json.loads(bruta)
                    except ValueError:
                        continue
                    if str(o.get("evento", "")).startswith("runtime."):
                        saida.append(o)
        return saida

    def contar(self, padrao):
        return len(re.findall(padrao, self.log()))

    def resumo(self):
        texto = self.log()
        rel, desl = len(re.findall(r"claude headless: religou", texto)), len(re.findall(r"claude headless: desligou", texto))
        ev = {}
        for o in self.eventos():
            chave = o["evento"] + (f"[{o['codigo']}]" if o.get("codigo") else "")
            ev[chave] = ev.get(chave, 0) + 1
        passagem = sum(n for k, n in ev.items() if k.split("[")[0] in PASSAGEM)
        return rel, desl, passagem, f"religou {rel}, desligou {desl}, runtime.* {ev or '{}'}"


class Vigia(threading.Thread):
    """Chat aberto: registra abertura, queda e o tipo de cada evento SSE (sem conteúdo)."""

    def __init__(self, prova, nome):
        super().__init__(daemon=True)
        self.p, self.nome, self.parar = prova, nome, False
        self.aberturas, self.quedas, self.tipos, self.problemas = 0, 0, {}, []

    def run(self):
        while not self.parar:
            try:
                c = http.client.HTTPConnection("127.0.0.1", self.p.porta, timeout=40)
                c.request("GET", f"/api/sessions/{self.nome}/events", headers=self.p.cab())
                r = c.getresponse()
                if r.status != 200:
                    time.sleep(0.5)
                    continue
                self.aberturas += 1
                evento = ""
                for bruta in r:
                    if self.parar:
                        return
                    linha = bruta.decode(errors="replace").strip()
                    if linha.startswith("event:"):
                        evento = linha[6:].strip()
                        self.tipos[evento] = self.tipos.get(evento, 0) + 1
                    elif linha.startswith("data:") and evento == "state" and '"problema"' in linha:
                        try:
                            d = json.loads(linha[5:])
                        except ValueError:
                            continue
                        if d.get("problema"):
                            det = str(d.get("problema_detalhe") or "").split(":", 1)[0]
                            self.problemas.append(f"{d['problema']}:{det}")
            except (OSError, http.client.HTTPException):
                pass
            if not self.parar:
                self.quedas += 1
                time.sleep(0.5)


class Prova:
    PERFIL = "debug"   # pasta do binário em crates/target

    def __init__(self, args):
        self.args = args
        self.raiz = Path(tempfile.mkdtemp(prefix="hangar-prova-"))
        self.home = self.raiz / "home"
        self.bin = self.raiz / "bin"
        self.work = self.raiz / "work"
        self.log = self.raiz / "backend.log"
        self.token = uuid.uuid4().hex + uuid.uuid4().hex[:16]
        self.porta, self.porta_convite = porta_livre(), porta_livre()
        self.tmux = "hangar-prova-" + self.raiz.name.rsplit("-", 1)[-1].lower()
        self.unit = self.tmux
        self.pids_backend = []
        self.marcas = {}   # marcador -> sessão
        self.linhas = []
        self.conta_b = Path(args.conta_b).expanduser().resolve() if args.conta_b else None
        # Contas de uso real do dono: prova nenhuma gasta cota nem grava nelas.
        if self.conta_b and self.conta_b.name in CONTAS_PROIBIDAS:
            raise SystemExit(f"--conta-b {self.conta_b.name} é de uso real; use ~/.claude-jefferson")
        # O embrulho executa pelo nome `claude` (o link do PATH): é por ele que o Hangar reconhece o
        # agente do pane. O caminho real só serve para achar o processo.
        self.claude = shutil.which("claude") or ""
        self.real_claude = os.path.realpath(self.claude)

    # ---- ambiente ----------------------------------------------------------------------------
    def preparar(self):
        server = REPO / f"crates/target/{self.PERFIL}/hangar-server"
        cano = REPO / f"crates/target/{self.PERFIL}/hangar-cano"
        if not (server.exists() and cano.exists()):
            perfil = ["--release"] if self.PERFIL == "release" else []
            subprocess.run(["cargo", "build", "--locked", *perfil, "-p", "hangar-server", "-p", "hangar-cano"],
                           cwd=REPO / "crates", check=True)
        if not Path(self.real_claude).is_file():
            raise SystemExit("claude não encontrado no PATH")
        if not (CONTA / ".credentials.json").is_file():
            raise SystemExit(f"conta {CONTA} sem login")
        for d in (self.home / ".claude", self.bin, self.work):
            d.mkdir(parents=True)
        # Transcrições na conta real; tudo que o Hangar grava ao lado de projects/ fica no HOME.
        (self.home / ".claude/projects").symlink_to(CONTA / "projects")
        mapa = ""
        if self.conta_b:
            b = self.home / ".claude-provab"
            b.mkdir()
            (b / "projects").symlink_to(self.conta_b / "projects")
            (b / ".credentials.json").write_text("{}")   # só para a lista de contas enxergá-la
            mapa = f'"{b}") export CLAUDE_CONFIG_DIR="{self.conta_b}" ;;\n  '
        (self.bin / "claude").write_text(
            "#!/bin/sh\n"
            f'case "$CLAUDE_CONFIG_DIR" in\n  {mapa}*) export CLAUDE_CONFIG_DIR="{CONTA}" ;;\nesac\n'
            f'exec "{self.claude}" --model {MODELO} "$@"\n')
        (self.bin / "tmux").write_text(f'#!/bin/sh\nexec /usr/bin/tmux -L {self.tmux} "$@"\n')
        lento = self.raiz / "slow-receive-pack"
        lento.write_text('#!/bin/sh\nsleep 20\nexec git-receive-pack "$@"\n')
        for f in (self.bin / "claude", self.bin / "tmux", lento):
            f.chmod(0o755)
        git = lambda *a, cwd=self.work: subprocess.run(["git", *a], cwd=cwd, check=True, capture_output=True)
        git("init", "--bare", "-b", "main", str(self.raiz / "remote.git"), cwd=self.raiz)
        git("init", "-b", "main")
        git("config", "user.email", "prova@hangar.invalid")
        git("config", "user.name", "prova")
        (self.work / "README.md").write_text("prova\n")
        git("add", "README.md")
        git("commit", "-m", "init")
        git("remote", "add", "origin", str(self.raiz / "remote.git"))
        git("push", "-u", "origin", "main")
        git("config", "remote.origin.receivepack", str(lento))

    def subir(self):
        subprocess.run(["systemctl", "--user", "reset-failed", self.unit], capture_output=True)
        lancador = (
            f"import sys; sys.path.insert(0, {str(BACKEND)!r})\n"
            # Antes de importar o api: a porta do convite vai ao Rust pelo `rust_server`.
            "from app import config, contas\n"
            f"config.GUEST_PORT = {self.porta_convite}\n"
            # A conta real não é reconciliada por este backend.
            "contas.e_conta = lambda p: False\n"
            "from app.adapters.claude_headless import adapter\n"
            "adapter.matar_orfaos = lambda: 0\n"
            + self.lancador_extra() +
            "from app import main; main.main()\n")
        env = {
            "HOME": str(self.home), "USER": os.environ.get("USER", ""), "LANG": "C.UTF-8",
            "TERM": "xterm-256color", "XDG_RUNTIME_DIR": os.environ["XDG_RUNTIME_DIR"],
            "DBUS_SESSION_BUS_ADDRESS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", ""),
            "PATH": f"{self.bin}:{Path.home() / '.local/bin'}:/usr/local/bin:/usr/bin:/bin",
            "COLORTERM": "truecolor", "CLAUDE_CODE_TMUX_TRUECOLOR": "1", "PYTHONFAULTHANDLER": "1",
            "CP_AUTH_TOKEN": self.token, "CP_PORT": str(self.porta), "CP_LAN_BIND_IP": "127.0.0.1",
            "CP_RUST_SERVER_BIN": str(REPO / f"crates/target/{self.PERFIL}/hangar-server"),
            "CP_RUST_CANO_BIN": str(REPO / f"crates/target/{self.PERFIL}/hangar-cano"),
            "CP_AUTO_RESUME": "0", "CP_SYNC": "0", "HANGAR_SEM_PASSO": "1",
            **self.ambiente_extra(),
        }
        subprocess.run(["systemd-run", "--user", "-q", f"--unit={self.unit}", "-p", "TimeoutStopSec=10",
                        "-p", f"WorkingDirectory={BACKEND}", "-p", f"StandardOutput=append:{self.log}",
                        "-p", f"StandardError=append:{self.log}", "env", "-i",
                        *[f"{k}={v}" for k, v in env.items()],
                        str(BACKEND / ".venv/bin/python"), "-c", lancador], check=True)
        self.esperar_de_pe()

    def lancador_extra(self):
        """Código a mais no lançador, antes do `main` (instrumentação de outra prova)."""
        return ""

    def ambiente_extra(self):
        return {}

    def esperar_de_pe(self):
        if not esperar(lambda: self.api("GET", "/api/sessions", timeout=5)[0] == 200, 120, 1):
            raise RuntimeError("backend isolado não respondeu em 120 s")
        pid = self.main_pid()
        if pid:
            self.pids_backend.append(pid)

    def main_pid(self):
        r = subprocess.run(["systemctl", "--user", "show", "-p", "MainPID", "--value", self.unit],
                           capture_output=True, text=True)
        return int(r.stdout.strip() or 0)

    def parar(self):
        """Para a unit; devolve (segundos, SIGKILL visto no journal)."""
        desde = time.time()
        t0 = time.monotonic()
        subprocess.run(["systemctl", "--user", "stop", self.unit], capture_output=True)
        return round(time.monotonic() - t0, 2), self.sigkill(desde)

    def reiniciar(self):
        desde = time.time()
        t0 = time.monotonic()
        r = subprocess.run(["systemctl", "--user", "restart", self.unit], capture_output=True, text=True)
        dur = round(time.monotonic() - t0, 2)
        if r.returncode != 0:
            raise RuntimeError(f"systemctl restart falhou ({r.returncode})")
        self.esperar_de_pe()
        return dur, self.sigkill(desde)

    def sigkill(self, desde):
        r = subprocess.run(["journalctl", "--user", "-u", self.unit, f"--since=@{int(desde)}", "-o", "cat",
                            "--no-pager"], capture_output=True, text=True)
        return bool(re.search(r"SIGKILL|timed out\. Killing", r.stdout))

    def rust_pid(self):
        main = self.main_pid()
        if not main:
            return 0
        return next((p for p in descendentes(main)
                     if p != main and os.path.basename(exe(p)) == "hangar-server"), 0)

    def diarios(self):
        return sorted((self.home / ".hangar/logs/diario").glob("uso-*.jsonl"))

    def limpar(self):
        # Cada etapa segue mesmo se a anterior falhar: sobra nenhuma unit, tmux ou agente da prova.
        etapas = (self._fechar_tudo, self._parar_final, self._matar_restos, self._apagar)
        for etapa in etapas:
            try:
                etapa()
            except Exception as e:
                print(f"limpeza: {etapa.__name__} falhou ({type(e).__name__})", file=sys.stderr)

    def _fechar_tudo(self):
        for nome in list({s for s in self.marcas.values()}):
            self.api("DELETE", f"/api/sessions/{nome}", timeout=15)

    def _parar_final(self):
        if self.main_pid():
            dur, kill = self.parar()
            self.linhas.append(Linha("52", "parada final da unit", not kill, f"{dur} s, SIGKILL {'sim' if kill else 'não'}"))

    def _matar_restos(self):
        subprocess.run(["/usr/bin/tmux", "-L", self.tmux, "kill-server"], capture_output=True)
        # Canos e agentes desta prova: só os processos com o HOME temporário no ambiente.
        alvo = f"HOME={self.home}".encode()
        for sinal in (signal.SIGTERM, signal.SIGKILL):
            vivos = []
            for p in Path("/proc").iterdir():
                if p.name.isdigit() and int(p.name) != os.getpid():
                    try:
                        if alvo in (p / "environ").read_bytes().split(b"\0"):
                            os.kill(int(p.name), sinal)
                            vivos.append(p.name)
                    except OSError:
                        pass
            if vivos:
                time.sleep(3)

    def _apagar(self):
        for pid in self.pids_backend:
            Path(os.environ["XDG_RUNTIME_DIR"], "cc-socks", f"{pid}.sock").unlink(missing_ok=True)
        for raiz in self.raizes_projetos():
            pasta = raiz / re.sub(r"[^A-Za-z0-9]", "-", str(self.work))
            if pasta.is_dir() and not pasta.is_symlink():
                shutil.rmtree(pasta, ignore_errors=True)
        subprocess.run(["systemctl", "--user", "reset-failed", self.unit], capture_output=True)
        if self.args.manter:
            print(f"pasta mantida: {self.raiz}")
        else:
            shutil.rmtree(self.raiz, ignore_errors=True)

    # ---- API ---------------------------------------------------------------------------------
    def cab(self):
        return {"Authorization": "Bearer " + self.token, "Content-Type": "application/json"}

    def api(self, metodo, caminho, corpo=None, timeout=90):
        t0 = time.monotonic()
        try:
            c = http.client.HTTPConnection("127.0.0.1", self.porta, timeout=timeout)
            c.request(metodo, caminho, body=json.dumps(corpo) if corpo is not None else None, headers=self.cab())
            r = c.getresponse()
            bruto = r.read()
            try:
                dados = json.loads(bruto or b"null")
            except ValueError:
                dados = None
            return r.status, dados, round(time.monotonic() - t0, 2), dict(r.getheaders())
        except (OSError, http.client.HTTPException) as e:
            return 0, {"error_code": type(e).__name__}, round(time.monotonic() - t0, 2), {}

    def sessao(self, nome):
        st, lista, *_ = self.api("GET", "/api/sessions", timeout=20)
        return next((s for s in lista or [] if s.get("name") == nome), None) if st == 200 else None

    def criar(self, nome, headless):
        st, corpo, *_ = self.api("POST", "/api/sessions", {"name": nome, "cwd": str(self.work), "headless": headless},
                                 timeout=180)
        return None if st == 200 else f"criar {st} {codigo(corpo)}"

    def marcador(self, nome):
        m = f"PROVA-{uuid.uuid4().hex[:10].upper()}"
        self.marcas[m] = nome
        return m

    def enviar(self, nome, texto=None, marcador=None):
        """Manda `texto` (ou o pedido de OK) com um marcador novo; devolve (marcador, status, corpo)."""
        m = marcador or self.marcador(nome)
        st, corpo, *_ = self.api("POST", f"/api/sessions/{nome}/input",
                                 {"text": f"{texto or 'Responda só OK.'} {m}"}, timeout=120)
        return m, st, corpo

    def enviar_na_queda(self, nome, prazo=30):
        """Com a porta fechada pela queda, repete o mesmo texto como o app faz, sem marcador novo."""
        m = self.marcador(nome)
        fim = time.monotonic() + prazo
        while True:
            m, st, corpo = self.enviar(nome, marcador=m)
            if st not in (0, 502, 503) or time.monotonic() >= fim:
                return m, st, corpo
            time.sleep(0.3)

    def fechar(self, nomes):
        for nome in nomes:
            self.api("DELETE", f"/api/sessions/{nome}", timeout=30)
        self.marcas = {m: s for m, s in self.marcas.items() if s not in nomes}

    def ociosa(self, nome, timeout=120):
        return esperar(lambda: (self.sessao(nome) or {}).get("state") == "idle", timeout, 1)

    def trabalhando(self, nome, timeout=30):
        return esperar(lambda: (self.sessao(nome) or {}).get("state") == "working", timeout, 0.3)

    def aquecer(self, nome, headless):
        """Cria a sessão e espera a primeira resposta: devolve o erro ou None."""
        erro = self.criar(nome, headless)
        if erro:
            return erro
        m, st, corpo = self.enviar(nome)
        if st != 200:
            return f"enviar {st} {codigo(corpo)}"
        if self.esperar_entregas([m])[m][0] != 1 or not self.ociosa(nome):
            return "primeira mensagem não saiu"
        return None

    def pane(self, nome):
        r = subprocess.run(["/usr/bin/tmux", "-L", self.tmux, "list-panes", "-a", "-F", "#{session_name} #{pane_id} #{pane_pid}"],
                           capture_output=True, text=True)
        return next(((i, int(p)) for s, i, p in (l.split() for l in r.stdout.splitlines()) if s == nome), (None, 0))

    def tela(self, nome):
        pane, _ = self.pane(nome)
        return subprocess.run(["/usr/bin/tmux", "-L", self.tmux, "capture-pane", "-p", "-t", pane or "none"],
                              capture_output=True, text=True).stdout

    def confiar_pasta(self):
        """A pasta da prova é nova para a conta: o primeiro terminal pergunta se ela é confiável. A
        prova responde uma vez pelo teclado, como o usuário; a conta guarda a resposta."""
        nome = "c00"
        if self.criar(nome, False):
            return
        self.responder_confianca(nome, 30)
        self.fechar([nome])

    def responder_confianca(self, nome, prazo):
        # A primeira opção do diálogo é "No, exit": Down leva a "Yes, I trust this folder".
        if esperar(lambda: "trust this folder" in self.tela(nome), prazo):
            pane, _ = self.pane(nome)
            subprocess.run(["/usr/bin/tmux", "-L", self.tmux, "send-keys", "-t", pane, "Down", "Enter"], capture_output=True)
            esperar(lambda: "trust this folder" not in self.tela(nome), 15)

    # ---- transcrição -------------------------------------------------------------------------
    def raizes_projetos(self):
        return [CONTA / "projects"] + ([self.conta_b / "projects"] if self.conta_b else [])

    def entregas(self, marcadores):
        """Marcador -> (entregas, [(arquivo, linha, posição)]), contadas no transcript cru: mensagem do
        usuário ou `queued_command`; o mesmo `uuid` em dois arquivos conta uma vez."""
        achados = {m: {} for m in marcadores}
        nome = re.sub(r"[^A-Za-z0-9]", "-", str(self.work))
        for raiz in self.raizes_projetos():
            for f in sorted((raiz / nome).glob("*.jsonl")):
                with f.open(errors="replace") as h:
                    for i, bruta in enumerate(h):
                        presentes = [m for m in marcadores if m in bruta]
                        if not presentes:
                            continue
                        try:
                            o = json.loads(bruta)
                        except ValueError:
                            continue
                        texto = ""
                        if o.get("type") == "user" and not o.get("isMeta"):
                            c = (o.get("message") or {}).get("content")
                            texto = c if isinstance(c, str) else " ".join(
                                b.get("text", "") for b in c or [] if isinstance(b, dict) and b.get("type") == "text")
                        elif o.get("type") == "attachment" and (o.get("attachment") or {}).get("type") == "queued_command":
                            texto = json.dumps(o["attachment"], ensure_ascii=False)
                        for m in presentes:
                            if m in texto:
                                achados[m].setdefault(o.get("uuid") or f"{f.name}:{i}", (f.name, i, texto.index(m)))
        return {m: (len(v), sorted(v.values())) for m, v in achados.items()}

    def esperar_entregas(self, marcadores, timeout=ENTREGA_S):
        marcadores = list(marcadores)
        esperar(lambda: all(n >= 1 for n, _ in self.entregas(marcadores).values()), timeout, 2)
        time.sleep(5)   # tempo para uma duplicata aparecer
        return self.entregas(marcadores)

    def registrar(self, step, caso, ok, evidencia):
        self.linhas.append(Linha(step, caso, ok, evidencia))
        print(f"[{step}] {'ok' if ok else 'FALHOU' if ok is False else 'pulado/inconclusivo'} — {caso}: {evidencia}", flush=True)

    # ---- casos -------------------------------------------------------------------------------
    def caso_49(self):
        for headless in (True, False):
            modo = "sem terminal" if headless else "com terminal"
            j = Janela(self)
            nomes, marcas, erros = [], [], []
            for i in range(self.args.n):
                nome = f"c49{'h' if headless else 't'}{i}"
                nomes.append(nome)
                erro = self.criar(nome, headless)
                if erro:
                    erros.append(erro)
                    continue
                m, st, corpo = self.enviar(nome)
                marcas.append(m)
                if st != 200:
                    erros.append(f"enviar {st} {codigo(corpo)}")
            contagem = self.esperar_entregas(marcas)
            certas = sum(1 for n, _ in contagem.values() if n == 1)
            dist = sorted(n for n, _ in contagem.values())
            rel, desl, passagem, resumo = j.resumo()
            ok = certas == self.args.n and not erros and rel == desl == passagem == 0
            self.registrar("49", f"criar e mandar na hora, {modo}", ok,
                           f"{certas}/{self.args.n} com 1 entrega (contagens {dist}); erros {erros or 'nenhum'}; {resumo}")
            self.fechar(nomes)

    def caso_50(self):
        for headless in (True, False):
            modo = "sem terminal" if headless else "com terminal"
            nome = f"c50{'h' if headless else 't'}"
            j = Janela(self)
            erro = self.aquecer(nome, headless)
            if erro:
                self.registrar("50", f"fila com o Claude ocupado, {modo}", False, erro)
                self.fechar([nome])
                continue
            longo, st, _ = self.enviar(nome, LONGO)
            ocupada = self.trabalhando(nome)
            fila = []
            for _ in range(3):
                m, st2, corpo = self.enviar(nome)
                fila.append((m, st2, codigo(corpo)))
                time.sleep(0.5)
            contagem = self.esperar_entregas([m for m, *_ in fila], timeout=240)
            posicoes = [contagem[m][1][0] if contagem[m][1] else None for m, *_ in fila]
            em_ordem = None not in posicoes and posicoes == sorted(posicoes)
            uma = all(contagem[m][0] == 1 for m, *_ in fila)
            rel, desl, passagem, resumo = j.resumo()
            ok = bool(ocupada) and uma and em_ordem and all(s == 200 for _, s, _ in fila) and rel == desl == passagem == 0
            self.registrar("50", f"3 mensagens durante um turno longo, {modo}", ok,
                           f"ocupada ao enviar: {'sim' if ocupada else 'não'}; entregas {[contagem[m][0] for m, *_ in fila]}; "
                           f"em ordem: {'sim' if em_ordem else 'não'}; status {[s for _, s, _ in fila]}; {resumo}")
            self.fechar([nome])

    def caso_51(self):
        for headless in (True, False):
            modo = "sem terminal" if headless else "com terminal"
            nome = f"c51{'h' if headless else 't'}"
            erro = self.aquecer(nome, headless)
            if erro:
                self.registrar("51", f"/clear com o chat aberto, {modo}", False, erro)
                self.fechar([nome])
                continue
            vigia = Vigia(self, nome)
            vigia.start()
            esperar(lambda: vigia.aberturas, 15)
            j = Janela(self)
            st_clear, corpo, *_ = self.api("POST", f"/api/sessions/{nome}/input", {"text": "/clear"}, timeout=120)
            time.sleep(8)
            self.ociosa(nome, 60)
            m, st, _ = self.enviar(nome)
            n = self.esperar_entregas([m])[m][0]
            time.sleep(3)
            vigia.parar = True
            rel, desl, passagem, resumo = j.resumo()
            ok = st_clear == 200 and n == 1 and vigia.aberturas == 1 and vigia.quedas == 0 and rel == desl == passagem == 0
            self.registrar("51", f"/clear com o chat aberto, {modo}", ok,
                           f"/clear {st_clear} {codigo(corpo)}; mensagem seguinte: {n} entrega(s); chat abriu "
                           f"{vigia.aberturas}x, caiu {vigia.quedas}x; {resumo}")
            self.fechar([nome])

    def caso_52(self):
        # Restart com fila: mensagem enfileirada com a sessão ocupada, restart, sai uma vez depois.
        nomes = {"c52h": True, "c52t": False}
        prontas = {n: self.aquecer(n, h) for n, h in nomes.items()}
        j = Janela(self)
        marcas = {}
        for nome, erro in prontas.items():
            if erro:
                self.registrar("52", f"restart com fila ({nome})", False, erro)
                continue
            self.enviar(nome, LONGO)
            self.trabalhando(nome)
            marcas[nome] = self.enviar(nome)
        try:
            dur, kill = self.reiniciar()
        except RuntimeError as e:
            self.registrar("52", "restart com fila", False, str(e))
            return
        contagem = self.esperar_entregas([m for m, *_ in marcas.values()], timeout=300)
        rel, desl, passagem, resumo = j.resumo()
        for nome, (m, st, corpo) in marcas.items():
            n = contagem[m][0]
            ok = st == 200 and n == 1 and not kill and rel == 0 and passagem == 0
            self.registrar("52", f"restart com fila, {'sem' if nomes[nome] else 'com'} terminal", ok,
                           f"envio {st} {codigo(corpo)}; depois do restart: {n} entrega(s); parada {dur} s, "
                           f"SIGKILL {'sim' if kill else 'não'}; {resumo}")
        self.fechar(list(nomes))
        # O mesmo com o cano morto antes da subida.
        nome = "c52m"
        erro = self.aquecer(nome, True)
        if erro:
            self.registrar("52", "restart com fila e cano morto", False, erro)
            return
        self.enviar(nome, LONGO)
        self.trabalhando(nome)
        m, st, corpo = self.enviar(nome)
        j = Janela(self)
        dur, kill = self.parar()
        meta = json.loads((self.home / ".hangar/claude-headless" / f"{nome}.json").read_text())
        pid = int((meta.get("cano") or {}).get("pid") or 0)
        morto = False
        if pid:
            try:
                os.killpg(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            morto = esperar(lambda: not Path(f"/proc/{pid}").exists(), 10)
        self.subir()
        n = self.esperar_entregas([m], timeout=300)[m][0]
        rel, desl, passagem, resumo = j.resumo()
        ok = bool(morto) and st == 200 and n == 1 and not kill and rel == 0 and passagem == 0
        self.registrar("52", "restart com fila e cano morto antes da subida", ok,
                       f"cano {pid} morto: {'sim' if morto else 'não'}; envio {st} {codigo(corpo)}; depois: {n} entrega(s); "
                       f"parada {dur} s, SIGKILL {'sim' if kill else 'não'}; {resumo}")
        self.fechar([nome])

    def caso_53(self):
        nomes = {"c53h": True, "c53t": False}
        for nome, h in nomes.items():
            erro = self.aquecer(nome, h)
            if erro:
                self.registrar("53", f"queda do Rust ({nome})", False, erro)
                return
        # Uma queda: a sessão continua no Rust novo e a mensagem mandada durante a queda sai uma vez.
        j = Janela(self)
        velho = self.rust_pid()
        if not velho:   # os.kill(0, …) mataria o grupo do próprio roteiro
            self.registrar("53", "queda do Rust", False, "nenhum hangar-server filho do backend")
            return
        os.kill(velho, signal.SIGKILL)
        durante = {nome: self.enviar_na_queda(nome) for nome in nomes}
        novo = esperar(lambda: (lambda p: p if p and p != velho else 0)(self.rust_pid()), 30)
        contagem = self.esperar_entregas([m for m, *_ in durante.values()])
        rel, desl, passagem, resumo = j.resumo()
        assumiu = j.contar(r"o Python assume a porta")
        for nome, (m, st, corpo) in durante.items():
            ok = bool(novo) and contagem[m][0] == 1 and not assumiu and rel == desl == passagem == 0
            self.registrar("53", f"uma queda do Rust, {'sem' if nomes[nome] else 'com'} terminal", ok,
                           f"Rust {velho} → {novo or 'não voltou'}; envio durante a queda {st} {codigo(corpo)}, "
                           f"{contagem[m][0]} entrega(s); Python assumiu: {'sim' if assumiu else 'não'}; {resumo}")
        # Três quedas em 60 s: o Python assume tudo, cada sessão retomada uma vez, nada duplicado.
        # A queda da fase anterior já conta na janela de 60 s do Supervisor: o Python pode assumir
        # depois de duas mortes aqui, e aí não há um terceiro Rust para matar.
        j = Janela(self)
        t0 = time.monotonic()
        assumiu_ja = lambda: j.contar(r"o Python assume a porta")
        mortes = 0
        while mortes < 3 and not assumiu_ja():
            pid = esperar(lambda: self.rust_pid() or assumiu_ja() and -1, 30)
            if not pid or pid == -1:
                break
            os.kill(pid, signal.SIGKILL)
            mortes += 1
            esperar(lambda: assumiu_ja() or (lambda p: p and p != pid)(self.rust_pid()), 30)
        decorrido = round(time.monotonic() - t0, 1)
        assumiu = esperar(assumiu_ja, 30)
        esperar(lambda: self.api("GET", "/api/sessions", timeout=5)[0] == 200, 60, 1)
        depois = {nome: self.enviar(nome) for nome in nomes}
        contagem = self.esperar_entregas([m for m, *_ in depois.values()])
        todas = self.entregas([m for m, s in self.marcas.items() if s in nomes])
        duplicadas = sorted(m for m, (n, _) in todas.items() if n > 1)
        retomadas = {nome: j.contar(rf"religou name={nome}\b") for nome, h in nomes.items() if h}
        _, _, passagem, resumo = j.resumo()
        ok = (bool(assumiu) and all(contagem[m][0] == 1 for m, *_ in depois.values())
              and not duplicadas and all(n == 1 for n in retomadas.values()))
        self.registrar("53", "três quedas do Rust em 60 s", ok,
                       f"{mortes} kill(s) nesta fase em {decorrido} s, mais a queda anterior; Python assumiu: "
                       f"{'sim' if assumiu else 'não'}; retomadas sem terminal "
                       f"{retomadas}; mensagem depois {[contagem[m][0] for m, *_ in depois.values()]}; "
                       f"duplicadas {duplicadas or 'nenhuma'}; {resumo}")
        self.fechar(list(nomes))

    def caso_54(self):
        # a) Trava de escrita no estado da fila: erro com código, sessão segue no Rust.
        nome = "c54h"
        erro_h = self.aquecer(nome, True)
        if erro_h:
            self.registrar("54", "trava de escrita na fila", False, erro_h)
        else:
            estado = self.home / ".claude/.hangar-queue/runtime"
            j = Janela(self)
            estado.chmod(0o500)
            try:
                barrada, st, corpo = self.enviar(nome)
            finally:
                estado.chmod(0o700)
            time.sleep(2)
            m, st2, _ = self.enviar(nome)
            self.esperar_entregas([m])
            contagem = self.entregas([barrada, m])
            rel, desl, passagem, resumo = j.resumo()
            ok = st != 200 and bool(codigo(corpo)) and contagem[m][0] == 1 and contagem[barrada][0] == 0 and rel == desl == passagem == 0
            self.registrar("54", "trava de escrita no estado da fila", ok,
                           f"envio travado {st} código {codigo(corpo) or '—'} ({contagem[barrada][0]} entrega); destravado "
                           f"{st2}, {contagem[m][0]} entrega(s); {resumo}")
        # b) Entrega incerta no terminal: o agente congelado não deixa provar o envio.
        nome_t = "c54t"
        erro = self.aquecer(nome_t, False)
        if erro:
            self.registrar("54", "entrega incerta no terminal", False, erro)
        else:
            pane_pid = self.pane(nome_t)[1]
            # Sem pane, descendentes(0) varreria a máquina e congelaria os `claude` reais.
            agentes = [p for p in descendentes(pane_pid) if exe(p) == self.real_claude] if pane_pid else []
        if not erro and not agentes:
            self.registrar("54", "entrega incerta no terminal", False, "agente do pane não achado")
        elif not erro:
            j = Janela(self)
            vigia = Vigia(self, nome_t)
            vigia.start()
            # Congelado antes do envio, o Rust só adia a entrada; congelado no meio da digitação, a
            # submissão fica sem prova. O ponto exato varia, então tenta alguns atrasos.
            incertas, faixa, atraso = [], None, None
            for atraso in (0.2, 0.5, 1.0):
                resposta = []
                envio = threading.Thread(target=lambda: resposta.append(self.enviar(nome_t)))
                envio.start()
                time.sleep(atraso)
                try:
                    sinalizar(agentes, signal.SIGSTOP)
                    envio.join(timeout=120)
                    faixa = esperar(lambda: (self.sessao(nome_t) or {}).get("problema"), 15)
                finally:
                    sinalizar(agentes, signal.SIGCONT)
                incerta, st, corpo = resposta[0] if resposta else (None, 0, None)
                if incerta:
                    incertas.append(incerta)
                if faixa:
                    break
                self.ociosa(nome_t, 60)
            m, st2, corpo2 = self.enviar(nome_t)
            contagem = self.esperar_entregas([*incertas, m])
            vigia.parar = True
            saidas = [contagem[x][0] for x in incertas]
            reabriu = sum(1 for o in j.eventos() if o["evento"] == "runtime.reopened")
            rel, desl, passagem, resumo = j.resumo()
            ok = bool(faixa) and reabriu >= 1 and contagem[m][0] == 1 and all(n <= 1 for n in saidas) and passagem == 0
            if not faixa and contagem[m][0] == 1 and all(n == 1 for n in saidas):
                # O Rust adiou e entregou depois: a incerta não foi forçada; o caso fica para a mão.
                ok = None
            self.registrar("54", "entrega incerta forçada no terminal", ok,
                           f"agente congelado: {len(agentes)} processo(s), {atraso} s depois do envio; envio {st} "
                           f"{codigo(corpo)}; faixa {str(faixa)[:40] if faixa else 'nenhuma'}; "
                           f"no chat {sorted(set(vigia.problemas)) or 'nada'}; reabertura {reabriu}; seguinte {st2} {codigo(corpo2)}, "
                           f"{contagem[m][0]} entrega(s); as tentativas saíram {saidas}; {resumo}")
        # c) Git ocupado: 4 pushes lentos e um commit, que recebe 503 sem passar pelo Python.
        alvo = nome if not erro_h else nome_t if not erro else None
        if not alvo:
            self.registrar("54", "Git ocupado", False, "sem sessão viva")
            return
        (self.work / "prova.txt").write_text(uuid.uuid4().hex)
        j = Janela(self)
        pushes = [threading.Thread(target=self.api, args=("POST", f"/api/sessions/{alvo}/git/push"), kwargs={"timeout": 120})
                  for _ in range(4)]
        for t in pushes:
            t.start()
        time.sleep(1.5)
        st, corpo, dur, cab = self.api("POST", f"/api/sessions/{alvo}/git/commit",
                                       {"message": "prova", "paths": ["prova.txt"]}, timeout=60)
        no_python = j.contar(rf"/api/sessions/{alvo}/git/commit")
        for t in pushes:
            t.join(timeout=150)
        retry = {k.lower(): v for k, v in cab.items()}.get("retry-after")
        ok = st == 503 and codigo(corpo) == "workspace_busy" and dur < 1 and bool(retry) and no_python == 0
        self.registrar("54", "Git ocupado com 4 pushes lentos", ok,
                       f"commit {st} {codigo(corpo) or '—'} em {dur} s, Retry-After {retry or '—'}; pedidos do commit no "
                       f"Python: {no_python}")
        self.fechar([nome, nome_t])

    def caso_55(self):
        if not self.conta_b:
            self.registrar("55", "troca de conta", None, "pulado: falta --conta-b (segunda conta Claude com login)")
        else:
            destino = str(self.home / ".claude-provab")
            for headless in (True, False):
                modo = "sem terminal" if headless else "com terminal"
                nome = f"c55{'h' if headless else 't'}"
                erro = self.aquecer(nome, headless)
                if erro:
                    self.registrar("55", f"troca de conta, {modo}", False, erro)
                    continue
                chaves = {f.name for f in (self.home / ".claude/.hangar-queue/runtime").glob("*.json")}
                j = Janela(self)
                st, corpo, dur, _ = self.api("POST", f"/api/sessions/{nome}/conta", {"config_dir": destino}, timeout=240)
                if not headless:   # a pasta da prova também é nova para a conta B
                    self.responder_confianca(nome, 20)
                self.ociosa(nome, 120)
                m, st2, _ = self.enviar(nome)
                n = self.esperar_entregas([m])[m][0]
                novas = {f.name for f in (self.home / ".claude/.hangar-queue/runtime").glob("*.json")} - chaves
                pasta_b = self.conta_b / "projects" / re.sub(r"[^A-Za-z0-9]", "-", str(self.work))
                na_b = any(m in f.read_text(errors="replace") for f in pasta_b.glob("*.jsonl"))
                rel, desl, passagem, resumo = j.resumo()
                ok = st == 200 and n == 1 and na_b and not novas and rel == desl == passagem == 0
                self.registrar("55", f"troca de conta, {modo}", ok,
                               f"troca {st} {codigo(corpo)} em {dur} s; mensagem seguinte na conta B: {'sim' if na_b else 'não'}; "
                               f"chave nova: {'sim' if novas else 'não'}; "
                               f"mensagem seguinte {st2}, {n} entrega(s); {resumo}")
                self.fechar([nome])
        # Isolado, o backend só vê a conta Codex do HOME temporário; ver a real exige CODEX_HOME nela,
        # e a subida grava hooks e skills ali. A transferência é provada à mão no app real.
        self.registrar("55", "transferência Claude → Codex", None,
                       "manual no app real: o backend isolado não enxerga a conta Codex real sem gravar nela")

    def tabela(self):
        linhas = ["| Step | Caso | Resultado | Evidência |", "|---|---|---|---|"]
        for l in self.linhas:
            res = "ok" if l.ok else "FALHOU" if l.ok is False else "pulado/inconclusivo"
            linhas.append(f"| {l.step} | {l.caso} | {res} | {l.evidencia.replace('|', '/')} |")
        return "\n".join(linhas)


ORDEM = ["49", "50", "51", "54", "55", "52", "53"]   # 53 por último: deixa o Python dono da porta


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--casos", default=",".join(ORDEM), help="steps a rodar, ex.: 49,52")
    ap.add_argument("--n", type=int, default=10, help="sessões por modo no step 49")
    ap.add_argument("--conta-b", help="segunda conta Claude com login, destino da troca de conta "
                    "(~/.claude-jefferson; claude-200-1 e claude-200-3 são recusadas)")
    ap.add_argument("--relatorio", help="grava a tabela neste arquivo")
    ap.add_argument("--manter", action="store_true", help="não apaga a pasta da prova (logs, HOME)")
    args = ap.parse_args()
    casos = [c for c in ORDEM if c in args.casos.split(",")]
    if not casos:
        ap.error(f"--casos sem step conhecido; use {','.join(ORDEM)}")
    # Terminal fechado ou kill: o finally ainda para a unit e os agentes pagos.
    for sinal in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sinal, lambda *_: sys.exit(1))
    p = Prova(args)
    print(f"prova em {p.raiz}: porta {p.porta}, convite {p.porta_convite}, "
          f"tmux -L {p.tmux}, unit {p.unit}", flush=True)
    try:
        p.preparar()
        p.subir()
        if not esperar(lambda: "hangar-server de pé" in p.log.read_text(errors="replace"), 30):
            p.registrar("—", "hangar-server de pé", False, "o Python subiu sem o Rust na frente")
            casos = []   # sem o Rust na frente, nenhum caso prova o dono único
        if casos:
            p.confiar_pasta()
        for caso in casos:
            try:
                getattr(p, f"caso_{caso}")()
            except Exception as e:   # um caso quebrado não impede os outros nem a limpeza
                p.registrar(caso, "execução do caso", False, type(e).__name__)
                traceback.print_exc()
    finally:
        try:
            p.limpar()
        finally:
            tabela = p.tabela()
            print("\n" + tabela)
            if args.relatorio:
                Path(args.relatorio).write_text(tabela + "\n")
    sys.exit(0 if all(l.ok is not False for l in p.linhas) else 1)


if __name__ == "__main__":
    main()
