#!/usr/bin/env python3
"""Prova de uso real isolada da parte 4 (`docs/migracao-rust/parte4/plano.md`, Task 12).

Sobe o backend desta árvore isolado (classe `Prova` de `prova-dono-unico.py`, montagem de
`prova-lista-sombra.py`: HOME, portas, token e `tmux -L` próprios, binários release, Codex numa
`CODEX_HOME` temporária) e prova por HTTP/WS/SSE o estado, a prévia e o terminal real no Rust.

    backend/.venv/bin/python scripts/prova-parte4.py [--casos 26,27,28,29] [--n 1,10,20]
        [--conta-b ~/.claude-jefferson] [--reserva] [--relatorio f.md] [--manter]

Contadores (lançador do backend isolado, despejados num arquivo da pasta da prova a cada 0,5 s):
`StateMonitor` e `PreviewBroker` criados no Python por sessão e provedor, PTY aberto pelo
`termsock` do Python por sessão, e o modo do dono do runtime. Capturas: o log do servidor tmux da
prova (ligado e desligado por SIGUSR2) conta cada `capture-pane` por pane, separando as que chegam
pelo cliente de controle (`-C`, o pool do Rust) das avulsas (processo `tmux` próprio).

`--reserva` sobe com `CP_RUST_SERVER=0` e roda só a volta pelo Python (estado e terminal). Claude
só Haiku (conta 02-200; a `--conta-b` só na troca de conta); Codex em gpt-6-luna fora do YOLO. O
relatório traz só nomes de sessão, estados, códigos, contagens e tempos: nenhum texto de conversa.
"""
import argparse
import base64
import http.client
import importlib.util
import json
import os
import signal
import socket
import statistics
import subprocess
import sys
import threading
import time
import traceback
from pathlib import Path

from websockets.exceptions import ConnectionClosed
from websockets.sync.client import connect

_spec = importlib.util.spec_from_file_location("prova_lista_troca", Path(__file__).with_name("prova-lista-troca.py"))
troca = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(troca)
sombra, base = troca.sombra, troca.base
esperar = base.esperar

TICK = os.sysconf("SC_CLK_TCK")
MEDIO = sombra.MEDIO
LONGO = base.LONGO
CARGA = "Escreva os números de 1 a 1200 por extenso, um por linha, sem mais nada."  # dura mais que a janela de 30 s
DECRESCENTE = "Escreva os números de 300 a 1 por extenso, em ordem decrescente, um por linha, sem mais nada."
PERGUNTA = sombra.PERGUNTA
PERMISSAO = sombra.PERMISSAO
# No modo "Ask for approval" o sandbox é só leitura e o modelo decide se pede escalada: sem pedir
# explícito, o `touch` falha calado e nenhum cartão aparece.
CODEX_COMANDO = ("Rode no shell `touch prova-codex.txt` com sandbox_permissions=\"require_escalated\" e uma "
                 "justificativa curta (a pasta é só leitura no sandbox). Não faça mais nada.")
JANELA = 15.0
RODADA = 0.75

LANCADOR = r'''
import threading as _th, time as _t, json as _j, os as _os, collections as _col
import app.api as _api
async def _sem_poda():
    return None
_api._prune_loop = _sem_poda
from app import runtime_coordinator as _rc, state as _state, preview as _prev, termsock as _ts
# O convite agora é do Rust: os casos 27 e 28 precisam de IP de rede local na máquina, sem
# substituto aqui como havia para o Python.
_cont = {{"sm": _col.Counter(), "pb": _col.Counter(), "pty": _col.Counter()}}
_sm0 = _state.StateMonitor.__init__
def _sm(self, name, *a, **k):
    _cont["sm"][f"{{name}}|{{k.get('provider')}}"] += 1
    return _sm0(self, name, *a, **k)
_state.StateMonitor.__init__ = _sm
_pb0 = _prev.PreviewBroker.__init__
def _pb(self, name, provider="claude", *a, **k):
    _cont["pb"][f"{{name}}|{{provider}}"] += 1
    return _pb0(self, name, provider, *a, **k)
_prev.PreviewBroker.__init__ = _pb
_pty0 = _ts._abrir_pty
def _pty(name, *a, **k):
    _cont["pty"][name] += 1
    return _pty0(name, *a, **k)
_ts._abrir_pty = _pty
def _despejo():
    while True:
        o = _rc.current()
        d = {{"t": _t.time(), "modo": o.mode if o else "sem_coordenador",
              **{{k: dict(v) for k, v in _cont.items()}}}}
        tmp = {arq!r} + ".tmp"
        with open(tmp, "w") as f:
            f.write(_j.dumps(d))
        _os.replace(tmp, {arq!r})
        _t.sleep(0.5)
_th.Thread(target=_despejo, daemon=True).start()
'''


def cpu(pid, filhos=True):
    try:
        f = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return sum(int(f[i]) for i in ((11, 12, 13, 14) if filhos else (11, 12))) / TICK
    except (OSError, IndexError, ValueError):
        return 0.0


class Chat(threading.Thread):
    """Um chat aberto (`/api/sessions/<nome>/events`) em qualquer porta, com o token dado. Guarda o
    tipo e os campos de controle de cada evento (estado, problema, fonte da prévia), nunca texto."""

    def __init__(self, p, nome, porta=None, token=None):
        super().__init__(daemon=True)
        self.p, self.nome = p, nome
        self.porta, self.token = porta or p.porta, token or p.token
        self.parar = False
        self.ev = []         # (monotonic, tipo, campos)
        self.aberturas, self.quedas, self.status = 0, 0, []
        self.trava = threading.Lock()
        self.start()

    def run(self):
        while not self.parar:
            try:
                c = http.client.HTTPConnection("127.0.0.1", self.porta, timeout=60)
                c.request("GET", f"/api/sessions/{self.nome}/events", headers={"Authorization": "Bearer " + self.token})
                r = c.getresponse()
                self.status.append(r.status)
                if r.status != 200:
                    time.sleep(1)
                    continue
                self.aberturas += 1
                tipo, data = None, []
                while not self.parar:
                    linha = r.fp.readline()
                    if not linha:
                        break
                    linha = linha.decode(errors="replace").rstrip("\r\n")
                    if linha.startswith("event:"):
                        tipo = linha[6:].strip()
                    elif linha.startswith("data:"):
                        data.append(linha[5:].lstrip())
                    elif linha == "" and tipo:
                        self.anotar(tipo, "\n".join(data))
                        tipo, data = None, []
            except (OSError, http.client.HTTPException):
                pass
            if not self.parar:
                self.quedas += 1
                time.sleep(0.5)

    def anotar(self, tipo, bruto):
        campos = {}
        try:
            d = json.loads(bruto) if bruto else {}
        except ValueError:
            d = {}
        if isinstance(d, dict):
            if tipo == "state":
                campos = {"state": d.get("state"), "problema": d.get("problema"),
                          "opcoes": len(d.get("options") or [])}
            elif tipo == "preview":
                campos = {"md": d.get("md"), "vazio": not d.get("text")}
            elif tipo == "ask_question":
                campos = {"perguntas": len(d.get("questions") or [])}
        with self.trava:
            self.ev.append((time.monotonic(), tipo, campos))

    def desde(self, t0, tipo=None):
        with self.trava:
            return [(t, k, c) for t, k, c in self.ev if t >= t0 and (tipo is None or k == tipo)]

    def estados(self, t0):
        return [c.get("state") for _, _, c in self.desde(t0, "state")]

    def ultimo(self):
        e = self.desde(0, "state")
        return e[-1][2].get("state") if e else None

    def fechar(self):
        self.parar = True


class TmuxLog:
    """Liga o log do servidor tmux da prova (SIGUSR2) numa janela e conta os `capture-pane` por pane."""

    def __init__(self, p):
        self.p = p

    def pid(self):
        r = self.p.tmux_cru("list-sessions", "-F", "#{pid}")
        return int((r.stdout.split() or ["0"])[0])

    def janela(self, segundos):
        pid = self.pid()
        # O log cai na pasta de trabalho do servidor (a do backend que o abriu); apagado a cada janela.
        arq = Path(os.readlink(f"/proc/{pid}/cwd")) / f"tmux-server-{pid}.log"
        inicio = arq.stat().st_size if arq.exists() else 0
        os.kill(pid, signal.SIGUSR2)
        time.sleep(segundos)
        os.kill(pid, signal.SIGUSR2)
        time.sleep(0.2)
        if not arq.exists():
            raise RuntimeError("o tmux não gravou o log da janela")
        texto = arq.read_bytes()[inicio:].decode(errors="replace")
        arq.unlink(missing_ok=True)
        controle, avulsa = {}, {}
        for linha in texto.splitlines():
            if "capture-pane" not in linha:
                continue
            alvo = linha.rsplit("-t ", 1)[-1].split()[0].strip("'\"") if "-t " in linha else "?"
            if "control_read_callback:" in linha:
                controle[alvo] = controle.get(alvo, 0) + 1
            elif "cmd_parse_build_commands: capture-pane" in linha:
                avulsa[alvo] = avulsa.get(alvo, 0) + 1
        # `cmd_parse_build_commands` também aparece para os comandos do cliente de controle
        # (conferido num tmux 3.7b à mão: uma captura pelo `-C` gera as duas linhas, mesmo alvo).
        for alvo, n in controle.items():
            if alvo in avulsa:
                avulsa[alvo] = max(0, avulsa[alvo] - n)
        return controle, {k: v for k, v in avulsa.items() if v}


class Prova(sombra.Prova):
    PERFIL = "release"

    def __init__(self, args):
        super().__init__(args)
        self.contador = self.raiz / "contadores.json"
        self.chats = []

    def preparar(self):
        super().preparar()
        # A prévia do hook (MessageDisplay) é a 1ª fonte do Monitor: sem o link ela cairia no pane.
        (self.home / ".claude/.hangar-preview").symlink_to((base.CONTA / ".hangar-preview").resolve())
        if self.conta_b:
            for sub in (".hangar-state", "sessions", ".hangar-askq", ".hangar-status", ".hangar-preview"):
                destino = self.conta_b / sub
                destino.mkdir(exist_ok=True)
                (self.home / ".claude-provab" / sub).symlink_to(destino.resolve())

    def lancador_extra(self):
        return LANCADOR.format(arq=str(self.contador))

    def ambiente_extra(self):
        env = {"CP_SCAN_ROOTS": str(self.home)}
        if self.args.reserva:
            env["CP_RUST_SERVER"] = "0"
        return env

    def limpar(self):
        for c in self.chats:
            c.fechar()
        super().limpar()

    # ---- utilidades --------------------------------------------------------------------------
    def tmux_cru(self, *args):
        return subprocess.run(["/usr/bin/tmux", "-L", self.tmux, *args], capture_output=True, text=True,
                              env={**os.environ, "HOME": str(self.home)})

    def ler_contador(self):
        """O despejo mais recente do lançador; sem ele (ou velho) não há como afirmar "nenhum"."""
        try:
            c = json.loads(self.contador.read_text())
        except (OSError, ValueError) as e:
            raise RuntimeError(f"contador ilegível ({type(e).__name__})") from None
        if time.time() - c.get("t", 0) > 3:
            raise RuntimeError("contador parado: o despejo do lançador não roda")
        return c

    def python_estado(self, nomes):
        """StateMonitor/PreviewBroker que o Python criou para estas sessões."""
        time.sleep(1.1)   # o lançador despeja o contador a cada 0,5 s
        c = self.ler_contador()
        sm = {k: v for k, v in (c.get("sm") or {}).items() if k.split("|")[0] in nomes}
        pb = {k: v for k, v in (c.get("pb") or {}).items() if k.split("|")[0] in nomes}
        return sm, pb

    def chat(self, nome, porta=None, token=None):
        c = Chat(self, nome, porta, token)
        self.chats.append(c)
        esperar(lambda: c.aberturas or len(c.status) > 3, 15, 0.2)
        return c

    def pane_id(self, nome):
        r = self.tmux_cru("list-panes", "-t", f"={nome}:", "-F", "#{pane_id}")
        return r.stdout.split()[0] if r.returncode == 0 and r.stdout.split() else None

    def sessao_bash(self, nome):
        r = self.tmux_cru("new-session", "-d", "-s", nome, "-x", "200", "-y", "50", "-c", str(self.work),
                          "env", "-i", f"HOME={self.home}", "TERM=xterm-256color", "PS1=$ ",
                          "/bin/bash", "--norc", "--noprofile")
        if r.returncode != 0:
            raise RuntimeError(f"tmux new-session {nome} falhou ({r.returncode})")
        self.vivas.add(nome)

    def tamanho(self, nome):
        # Com cliente anexado a janela tem uma linha a menos que o painel: a barra de status do tmux.
        return self.tmux_cru("display", "-p", "-t", f"={nome}:", "#{window_width}x#{window_height}").stdout.strip()

    def opcao_tamanho(self, nome):
        # `={nome}` sozinho não acha a sessão no show-options; com `:` acha.
        r = self.tmux_cru("show-options", "-v", "-t", f"={nome}:", "@hangar_term_size")
        if r.returncode != 0:
            # Opção não definida responde "invalid option"; qualquer outro erro não prova que limpou.
            return "" if "invalid option" in r.stderr else None
        return r.stdout.strip()

    def clientes(self, nome):
        """Clientes anexados; None quando o tmux não respondeu (nunca vale como "nenhum")."""
        r = self.tmux_cru("list-clients", "-t", f"={nome}", "-F", "#{client_tty}")
        return [x for x in r.stdout.split() if x] if r.returncode == 0 else None

    def ws(self, caminho, porta=None, token=None, cols=100, rows=30, extra=""):
        url = (f"ws://127.0.0.1:{porta or self.porta}{caminho}?token={token or self.token}"
               f"&cols={cols}&rows={rows}{extra}")
        return connect(url, max_size=None, open_timeout=10, compression=None)

    @staticmethod
    def eco(ws, marca, prazo=10):
        ws.send(f"echo {marca}\r".encode())
        fim, junto = time.monotonic() + prazo, b""
        while time.monotonic() < fim:
            try:
                q = ws.recv(timeout=max(0.1, fim - time.monotonic()))
            except TimeoutError:
                break
            if isinstance(q, bytes):
                junto = (junto + q)[-4096:]
                if junto.count(marca.encode()) >= 2:   # o eco da digitação e a saída do echo
                    return True
        return False

    @staticmethod
    def fechamento(ws, prazo=15):
        """(código, motivo) do fechamento que o servidor mandar, ou None."""
        fim = time.monotonic() + prazo
        try:
            while time.monotonic() < fim:
                ws.recv(timeout=max(0.1, fim - time.monotonic()))
        except ConnectionClosed as e:
            rc = e.rcvd
            return (rc.code, rc.reason) if rc else (None, "sem quadro de fechamento")
        except TimeoutError:
            return None
        return None

    def convite(self, nome):
        """Cria um convite pela porta isolada e resgata: devolve (token do convidado, id) ou erro."""
        st, corpo, *_ = self.api("POST", f"/api/sessions/{nome}/share?local=true", timeout=30)
        if st != 200:
            return None, f"convite {st} {base.codigo(corpo)}"
        code = corpo["link"].rsplit("/convite/", 1)[1]
        c = http.client.HTTPConnection("127.0.0.1", self.porta_convite, timeout=30)
        c.request("POST", "/api/guest/redeem", body=json.dumps({"code": code, "device": "prova"}),
                  headers={"Content-Type": "application/json"})
        r = c.getresponse()
        d = json.loads(r.read() or b"null")
        if r.status != 200:
            return None, f"resgate {r.status} {base.codigo(d)}"
        return d["token"], corpo["id"]

    def conferir(self, step, caso, itens, notas=""):
        """`itens`: [(descrição, ok, detalhe)]. Uma linha por caso, falha listada."""
        ok = all(o for _, o, _ in itens if o is not None)
        falhas = [d for d, o, _ in itens if o is False]
        evid = "; ".join(f"{d}: {'sim' if o else 'não' if o is False else '—'}{f' ({det})' if det else ''}"
                         for d, o, det in itens)
        self.registrar(step, caso, ok, (f"FALHOU {falhas}. " if falhas else "") + evid + (f"; {notas}" if notas else ""))

    def criar_claude(self, nome, headless=False, **extra):
        if erro := self.nova(nome, "claude", headless, **extra):
            return erro
        if erro := self.mandar(nome, "Responda só OK."):
            return erro
        if not self.esperar_estado(nome, {"idle"}, 120):
            return f"{nome} não parou ({self.estado(nome)})"
        return None

    # ---- Step 26: Claude com terminal --------------------------------------------------------
    def caso_26(self):
        if erro := self.criar_claude("ct"):
            self.registrar("26", "abrir ct", False, erro)
            return
        self.c26_capturas()
        self.c26_estados_previa()
        self.c26_pergunta()
        self.c26_permissao()
        self.c26_clear()
        self.c26_conta()
        self.c26_morta()
        nomes = {"ct", "cp", "cq", "cd"}
        sm, pb = self.python_estado(nomes)
        sm = {k: v for k, v in sm.items() if not k.endswith("|claude_headless")}
        modo = self.ler_contador().get("modo")
        self.conferir("26", "nenhum StateMonitor/PreviewBroker Python para Claude com terminal (fim do Step)",
                      [("modo rust", modo == "rust", str(modo)),
                       ("StateMonitor", not sm, str(sm or 0)), ("PreviewBroker", not pb, str(pb or 0))])

    def c26_capturas(self):
        log = TmuxLog(self)
        pane = self.pane_id("ct")
        self.esperar_estado("ct", {"idle"}, 60)
        time.sleep(3)
        so_lista = log.janela(JANELA)
        dono = self.chat("ct")
        esperar(lambda: dono.ultimo(), 15)
        time.sleep(3)
        um = log.janela(JANELA)
        token_g, share = self.convite("ct")
        convidado = self.chat("ct", self.porta_convite, token_g) if token_g else None
        esperar(lambda: convidado is None or convidado.ultimo(), 20)
        time.sleep(3)
        tres = log.janela(JANELA)
        sm, pb = self.python_estado({"ct"})

        def taxa(j):
            return round(j[0].get(pane, 0) / JANELA, 2), j[1].get(pane, 0) + sum(n for a, n in j[1].items() if "ct" in a)

        (a_c, a_v), (b_c, b_v), (c_c, c_v) = taxa(so_lista), taxa(um), taxa(tres)
        esperado = 1 / RODADA
        self.conferir("26", "uma captura por rodada (ct parada, contagem no log do tmux)", [
            ("convite resgatado", bool(token_g), "" if token_g else share),
            ("convidado recebe state", bool(convidado and convidado.ultimo()),
             f"convidado {convidado.ultimo() if convidado else '—'}"),
            ("chat do dono recebe state", bool(dono.ultimo()), str(dono.ultimo())),
            ("com 1 chat: ~1 captura por 0,75 s", abs(b_c - esperado) <= 0.35, f"{b_c}/s pelo -C"),
            ("com 2 chats (dono, convidado): igual a 1", abs(c_c - b_c) <= 0.3, f"{c_c}/s pelo -C"),
            ("nenhuma captura avulsa de ct com chat aberto", b_v == 0 and c_v == 0, f"{b_v} e {c_v}"),
            ("StateMonitor/PreviewBroker Python para ct", not sm and not pb, f"{sm or 0} / {pb or 0}"),
        ], f"só a lista aberta: {a_c}/s pelo -C, {a_v} avulsas")
        convidado and convidado.fechar()
        self.chat_ct = dono
        self.share_ct = share

    def c26_estados_previa(self):
        dono, log, pane = self.chat_ct, TmuxLog(self), self.pane_id("ct")
        # Os `.hangar-preview` das duas contas apontam para a mesma pasta real: para cair no pane,
        # o arquivo some das duas.
        links = [l for l in (self.home / ".claude/.hangar-preview", self.home / ".claude-provab/.hangar-preview")
                 if l.is_symlink()]
        alvos = {l: os.readlink(l) for l in links}
        for fonte, pedido in (("arquivo", LONGO), ("pane", DECRESCENTE)):
            if fonte == "pane":
                for l in links:
                    l.unlink()
            try:
                t0 = time.monotonic()
                # Pedido longo: resposta curta termina antes de aparecer no pane. E diferente do
                # anterior: prévia contida na resposta já gravada é descartada (`is_committed`).
                self.mandar("ct", pedido)
                trab = esperar(lambda: "working" in dono.estados(t0), 30, 0.2)
                taxas = []
                for _ in range(3):
                    time.sleep(1)
                    cont, _ = log.janela(2)
                    taxas.append(round(cont.get(pane, 0) / 2, 2))
                parou = self.esperar_estado("ct", {"idle"}, 240) and esperar(lambda: dono.ultimo() == "idle", 20)
                prev = dono.desde(t0, "preview")
                md = sum(1 for *_, c in prev if c.get("md") and not c.get("vazio"))
                pane_n = sum(1 for *_, c in prev if c.get("md") is False and not c.get("vazio"))
                taxa = max(taxas)
                if fonte == "arquivo":
                    itens = [("chat viu working e idle", bool(trab and parou), ""),
                             ("prévia do arquivo (md)", md > 0, f"{md} prévias md, {pane_n} do pane"),
                             ("sem captura rápida com o arquivo", 0 < taxa <= 2, f"{taxa}/s")]
                else:
                    itens = [("chat viu working e idle", bool(trab and parou), ""),
                             ("prévia do pane", pane_n > 0, f"{pane_n} prévias do pane, {md} md"),
                             ("captura rápida só para a prévia", taxa >= 3, f"{taxa}/s")]
                limpa = prev and prev[-1][2].get("vazio")
                itens.append(("prévia limpa no fim", bool(limpa), ""))
                self.conferir("26", f"trabalhando/parada e prévia pelo {fonte}", itens)
            finally:
                if fonte == "pane":
                    for l, alvo in alvos.items():
                        l.symlink_to(alvo)

    def c26_pergunta(self):
        dono = self.chat_ct
        t0 = time.monotonic()
        self.mandar("ct", PERGUNTA)
        esperou = self.esperar_estado("ct", {"awaiting_input"}, 90)
        ask = esperar(lambda: dono.desde(t0, "ask_question"), 20, 0.2)
        estado_chat = esperar(lambda: "awaiting_input" in dono.estados(t0), 10, 0.2)
        time.sleep(4)
        n_ask = len(dono.desde(t0, "ask_question"))
        self.api("POST", "/api/sessions/ct/interrupt", timeout=30)
        voltou = self.esperar_estado("ct", {"idle"}, 60)
        self.conferir("26", "pergunta nativa (AskUserQuestion)", [
            ("lista awaiting_input", bool(esperou), ""), ("chat awaiting_input", bool(estado_chat), ""),
            ("ask_question uma vez", n_ask == 1, f"{n_ask}"), ("volta a idle depois do Esc", bool(voltou), "")])

    def c26_permissao(self):
        """Cartão de permissão do Claude com terminal (modo manual, Bash), com e sem chat aberto."""
        for nome, com_chat in (("cp", False), ("cq", True)):
            if erro := self.criar_claude(nome, permission_mode="manual"):
                self.registrar("26", f"cartão de permissão {'com' if com_chat else 'sem'} chat", False, erro)
                continue
            chat = self.chat(nome) if com_chat else None
            t0 = time.monotonic()
            vistos = []
            self.mandar(nome, PERMISSAO)
            fim = time.monotonic() + 90
            while time.monotonic() < fim:
                s = self.sessao(nome) or {}
                vistos.append((s.get("state"), bool(s.get("options"))))
                if s.get("state") == "awaiting_input" and s.get("options"):
                    break
                time.sleep(0.5)
            time.sleep(6)   # várias rodadas paradas no cartão
            s = self.sessao(nome) or {}
            parado = s.get("state") == "awaiting_input" and bool(s.get("options"))
            tela_cartao = "Do you want" in self.tela(nome) or "❯ 1." in self.tela(nome)
            itens = [("lista awaiting_input com opções", parado, f"estado {s.get('state')}, opções {len(s.get('options') or [])}"),
                     ("pane mostra o cartão", None if not tela_cartao else True, "" if tela_cartao else "cartão segurado pelo hook")]
            if chat:
                itens.append(("chat awaiting_input", "awaiting_input" in chat.estados(t0), ""))
            st, corpo, *_ = self.api("POST", f"/api/sessions/{nome}/select", {"option": 2, "interaction_id": s.get("interaction_id")}, timeout=30)
            saiu = esperar(lambda: (self.sessao(nome) or {}).get("state") not in (None, "awaiting_input"), 30, 0.5)
            itens += [("negar pelo app", st == 200, f"{st} {base.codigo(corpo)}"), ("sai do cartão", bool(saiu), "")]
            working_preso = sum(1 for e, _ in vistos if e == "working")
            self.conferir("26", f"cartão de permissão, terminal, {'com' if com_chat else 'sem'} chat aberto", itens,
                          f"leituras antes do cartão: {len(vistos)}, working {working_preso}")
            if chat:
                chat.fechar()
            self.fechar([nome])

    def c26_clear(self):
        dono = self.chat_ct
        q0, a0 = dono.quedas, dono.aberturas
        t0 = time.monotonic()
        st, corpo, *_ = self.api("POST", "/api/sessions/ct/input", {"text": "/clear"}, timeout=120)
        reset = esperar(lambda: dono.desde(t0, "reset"), 20, 0.2)
        time.sleep(4)
        t1 = time.monotonic()
        self.mandar("ct", "Responda só OK.")
        ciclo = esperar(lambda: "working" in dono.estados(t1) and dono.ultimo() == "idle", 120, 0.5)
        self.conferir("26", "/clear com o chat aberto", [
            ("/clear aceito", st == 200, f"{st} {base.codigo(corpo)}"), ("reset no chat", bool(reset), ""),
            ("estado segue depois (working → idle)", bool(ciclo), ""),
            ("chat não caiu", dono.quedas == q0 and dono.aberturas == a0, f"quedas +{dono.quedas - q0}")])

    def c26_conta(self):
        if not self.conta_b:
            self.registrar("26", "troca de conta (em_troca)", None, "pulado: falta --conta-b")
            return
        dono = self.chat_ct
        t0 = time.monotonic()
        st, corpo, dur, _ = self.api("POST", "/api/sessions/ct/conta", {"config_dir": str(self.home / ".claude-provab")},
                                     timeout=240)
        self.responder_confianca("ct", 20)
        self.esperar_estado("ct", {"idle"}, 120)
        t1 = time.monotonic()
        self.mandar("ct", "Responda só OK.")
        ciclo = esperar(lambda: "working" in dono.estados(t1) and dono.ultimo() == "idle", 120, 0.5)
        mortos = [e for e in dono.estados(t0) if e == "dead"]
        conta = str((self.sessao("ct") or {}).get("conta") or "")
        self.conferir("26", "troca de conta com o chat aberto", [
            ("troca 200", st == 200, f"{st} {base.codigo(corpo)} em {dur} s"),
            ("nenhum state dead durante a troca", not mortos, f"{len(mortos)}"),
            ("estado segue", bool(ciclo), ""),
            ("sessão na conta B", conta.endswith(str(self.conta_b)), Path(conta.split(":", 1)[-1]).name or "—")])

    def c26_morta(self):
        if erro := self.criar_claude("cd"):
            self.registrar("26", "sessão morta", False, erro)
            return
        chat = self.chat("cd")
        esperar(lambda: chat.ultimo(), 15)
        t0 = time.monotonic()
        self.tmux_cru("kill-session", "-t", "=cd")
        morta = esperar(lambda: "dead" in chat.estados(t0), 20, 0.2)
        fora = esperar(lambda: self.sessao("cd") is None, 20, 1)
        self.vivas.discard("cd")
        self.conferir("26", "sessão morta com o chat aberto", [
            ("state dead no chat", bool(morta), f"{round(time.monotonic() - t0, 1)} s" if morta else ""),
            ("sai da lista", bool(fora), "")])
        chat.fechar()

    def sugestoes(self):
        n = sum(len(c.desde(0, "suggest")) for c in self.chats)
        self.registrar("26", "sugestão (suggest) vista em algum chat", True if n else None,
                       f"{n} eventos" if n else "nenhuma: o Claude não ofereceu sugestão nesta rodada")

    # ---- Step 27: Codex e Claude sem terminal com cartão --------------------------------------
    def caso_27(self):
        casos = [("xh", "codex", True), ("xt", "codex", False), ("ph", "claude", True)]
        for nome, prov, headless in casos:
            modo = "sem terminal" if headless else "com terminal"
            extra = {"permission_mode": "Ask for approval"} if prov == "codex" and headless else \
                {"permission_mode": "manual"} if prov == "claude" else {}
            if erro := self.nova(nome, prov, headless, **extra):
                self.registrar("27", f"{prov} {modo}", False, erro)
                continue
            notas = []
            if prov == "codex" and not headless:
                self.esperar_estado(nome, {"idle"}, 90)
                # Recém-nascida, a TUI recusa o picker (409) até o app-server dizer que aceita texto.
                leitura = []
                esperar(lambda: leitura.append(self.api("GET", f"/api/sessions/{nome}/codex-permissions", timeout=60))
                        or leitura[-1][0] == 200, 60, 2)
                st, modos, *_ = leitura[-1]
                notas.append(f"GET modos {st} {base.codigo(modos)} em {len(leitura)} tentativa(s)")
                nomes = [m.get("nome") for m in (modos or {}).get("modes") or []] if st == 200 else []
                alvo = next((n for n in nomes if n and ("approval" in n.lower() or "read" in n.lower()
                                                        or "default" in n.lower())), None)
                st2, corpo, *_ = self.api("POST", f"/api/sessions/{nome}/codex-permissions", {"mode": alvo}, timeout=60) \
                    if alvo else (0, None)
                notas.append(f"modos {nomes}, escolhido {alvo!r} → {st2} {base.codigo(corpo)}")
                if st2 != 200:
                    notas.append(f"pelo teclado: {self.codex_pedir_aprovacao(nome)}")
            chat = self.chat(nome)
            t0 = time.monotonic()
            self.mandar(nome, CODEX_COMANDO if prov == "codex" else PERMISSAO)
            cartao = esperar(lambda: (lambda s: s.get("state") == "awaiting_input" and bool(s.get("options")))(
                self.sessao(nome) or {}), 120, 0.5)
            time.sleep(4)
            s = self.sessao(nome) or {}
            no_chat = "awaiting_input" in chat.estados(t0)
            opc_chat = max([c.get("opcoes", 0) for *_, c in chat.desde(t0, "state")] or [0])
            tela = "" if headless else self.tela(nome)
            st, corpo, *_ = self.api("POST", f"/api/sessions/{nome}/select", {"option": 2, "interaction_id": s.get("interaction_id")}, timeout=60)
            saiu = esperar(lambda: (self.sessao(nome) or {}).get("state") not in (None, "awaiting_input"), 60, 0.5)
            itens = [("lista awaiting_input com opções", bool(cartao), f"{s.get('state')}, {len(s.get('options') or [])} opções"),
                     ("chat awaiting_input com opções", no_chat and opc_chat > 0, f"{opc_chat} opções no state"),
                     ("negar pelo app", st == 200, f"{st} {base.codigo(corpo)}"), ("sai do cartão", bool(saiu), "")]
            if not headless:
                itens.insert(0, ("menu na TUI", "Would you like to run" in tela, ""))
            self.conferir("27", f"{'Codex' if prov == 'codex' else 'Claude'} {modo} fora do YOLO", itens, "; ".join(notas))
            chat.fechar()
            self.fechar([nome])

    def codex_pedir_aprovacao(self, nome):
        """`/permissions` → "Ask for approval" pelo teclado, como o usuário faria na TUI. Só quando a
        rota recusa: o picker do Codex mudou o rodapé e a leitura do Python não o reconhece."""
        pane, _ = self.pane(nome)
        tm = lambda *a: self.tmux_cru("send-keys", "-t", pane, *a)   # noqa: E731
        tm("-l", "/permissions")
        time.sleep(0.6)
        tm("Enter")
        if not esperar(lambda: "Update Model Permissions" in self.tela(nome), 10, 0.3):
            return "picker não abriu"
        # O picker abre com "Loading permission profiles…" e só depois lista os modos.
        if not esperar(lambda: "Ask for approval" in self.tela(nome), 15, 0.3):
            return "sem a opção Ask for approval"
        tm("1")
        time.sleep(0.5)
        if "Update Model Permissions" in self.tela(nome):
            tm("Enter")
        fechou = esperar(lambda: "Update Model Permissions" not in self.tela(nome), 10, 0.3)
        return "Ask for approval escolhido" if fechou else "picker não fechou"

    # ---- Step 28: terminal real --------------------------------------------------------------
    def caso_28(self):
        for f in (self.t_dono, self.t_convidado, self.t_duas, self.t_409, self.t_atalho,
                  self.t_term_nome, self.t_ping, self.t_queda):
            try:
                f()
            except Exception as e:
                self.registrar("28", f.__name__, False, f"quebrou: {type(e).__name__}: {str(e)[:80]}")
                traceback.print_exc()

    def pty_python(self):
        time.sleep(1.1)   # o lançador despeja o contador a cada 0,5 s
        return sum((self.ler_contador().get("pty") or {}).values())

    def t_dono(self):
        self.sessao_bash("b1")
        st, cfg, *_ = self.api("GET", "/api/config", timeout=20)
        pty0 = self.pty_python()
        ws = self.ws("/api/sessions/b1/term")
        ok = self.eco(ws, "PROVA-DONO-1")
        tam = self.tamanho("b1")
        ws.close()
        sumiu = esperar(lambda: self.clientes("b1") == [], 15, 0.3)
        time.sleep(0.6)
        self.conferir("28", "terminal do dono (porta isolada do Rust)", [
            ("eco", ok, ""), ("tamanho do painel aplicado", tam == "100x29", tam),
            ("cliente tmux sai ao fechar", bool(sumiu), ""), ("tamanho reposto", self.tamanho("b1") == "200x50", self.tamanho("b1")),
            ("@hangar_term_size limpo", self.opcao_tamanho("b1") == "", self.opcao_tamanho("b1") or "vazio"),
            ("nenhum PTY no Python", self.pty_python() == pty0, ""),
            ("/api/config terminal_panel", st == 200 and ((cfg or {}).get("somente_leitura") or {}).get("terminal_panel") is True, str(((cfg or {}).get("somente_leitura") or {}).get("terminal_panel")))])

    def t_convidado(self):
        token, share = self.convite("b1")
        if not token:
            self.registrar("28", "terminal do convidado de convite", False, share)
            return
        pty0 = self.pty_python()
        g = self.ws("/api/sessions/b1/term", self.porta_convite, token)
        ok = self.eco(g, "PROVA-CONV-1")
        # Um painel em todas as portas: o dono abrindo pela porta do Rust tira o convidado.
        d = self.ws("/api/sessions/b1/term")
        fech_g = self.fechamento(g)
        ok_d = self.eco(d, "PROVA-CONV-2")
        d.close()
        esperar(lambda: self.clientes("b1") == [], 15, 0.3)
        g2 = self.ws("/api/sessions/b1/term", self.porta_convite, token)
        ok2 = self.eco(g2, "PROVA-CONV-3")
        st, *_ = self.api("DELETE", f"/api/sessions/b1/share/{share}", timeout=30)
        fech_rev = self.fechamento(g2, 20)
        sumiu = esperar(lambda: self.clientes("b1") == [], 15, 0.3)
        self.conferir("28", "terminal do convidado de convite (porta isolada do convite)", [
            ("eco do convidado", ok, ""), ("dono assume: convidado fecha 1000", (fech_g or (None,))[0] == 1000, str(fech_g)),
            ("eco do dono depois", ok_d, ""), ("convidado reabre", ok2, ""),
            ("revogar fecha 4410", st == 200 and (fech_rev or (None,))[0] == 4410, f"DELETE {st}, {fech_rev}"),
            ("cliente tmux sai", bool(sumiu), ""), ("nenhum PTY no Python", self.pty_python() == pty0, "")])

    def t_duas(self):
        a = self.ws("/api/sessions/b1/term")
        ok_a = self.eco(a, "PROVA-DUAS-1")
        b = self.ws("/api/sessions/b1/term", cols=120, rows=40)
        fech = self.fechamento(a)
        ok_b = self.eco(b, "PROVA-DUAS-2")
        n = len(self.clientes("b1"))
        tam = self.tamanho("b1")
        b.close()
        esperar(lambda: self.clientes("b1") == [], 15, 0.3)
        time.sleep(0.6)
        self.conferir("28", "duas conexões do dono na mesma sessão", [
            ("1ª ecoa", ok_a, ""),
            ("1ª fecha com 1000 'outra conexao assumiu'", bool(fech) and fech[0] == 1000 and "assumiu" in (fech[1] or ""), str(fech)),
            ("2ª ecoa", ok_b, ""), ("um cliente tmux só", n == 1, str(n)), ("tamanho da 2ª", tam == "120x39", tam),
            ("tamanho reposto", self.tamanho("b1") == "200x50", self.tamanho("b1"))])

    def t_409(self):
        shown = self.sessao("b1") or {}
        selection = {"option": 1, "interaction_id": shown.get("interaction_id")}
        ws = self.ws("/api/sessions/b1/term")
        self.eco(ws, "PROVA-409")
        st, corpo, *_ = self.api("POST", "/api/sessions/b1/select", selection, timeout=30)
        det = (corpo or {}).get("detail") if isinstance(corpo, dict) else None
        msg = isinstance(det, dict) and bool(det.get("message") or det.get("mensagem") or det.get("msg"))
        ws.close()
        esperar(lambda: self.clientes("b1") == [], 15, 0.3)
        st2, corpo2, *_ = self.api("POST", "/api/sessions/b1/select", selection, timeout=30)
        self.conferir("28", "409 com o painel aberto", [
            ("409 erro_terminal_aberto", st == 409 and base.codigo(corpo) == "erro_terminal_aberto", f"{st} {base.codigo(corpo)}"),
            ("resposta traz o texto", msg, ""),
            ("sem painel não é 409", st2 != 409, f"{st2} {base.codigo(corpo2)}")])

    def t_atalho(self):
        st, corpo, *_ = self.api("POST", "/api/sessions/b1/shortcut-shell", {"command": "bash --norc", "label": "prova"},
                                 timeout=60)
        ident = ((corpo or {}).get("terminal") or {}).get("id") if st in (200, 202) else None
        ok = False
        if ident:
            ws = self.ws("/api/sessions/b1/term", extra=f"&shortcut={ident}")
            ok = self.eco(ws, "PROVA-ATALHO-1")
            ws.close()
        st_h, corpo_h, *_ = self.api("POST", "/api/sessions/b1/shortcut-shell",
                                     {"command": "bash --norc", "label": "prova-h", "runs_in": "hangar", "key": "prova-h"},
                                     timeout=60)
        ident_h = ((corpo_h or {}).get("terminal") or {}).get("id") if st_h in (200, 202) else None
        ok_h = False
        if ident_h:
            ws = self.ws(f"/api/hangar-terminals/{ident_h}/term")
            ok_h = self.eco(ws, "PROVA-ATALHO-2")
            ws.close()
        self.conferir("28", "terminal de atalho (da sessão e No Hangar)", [
            ("atalho criado", bool(ident), f"{st} {base.codigo(corpo)}"), ("eco no atalho da sessão", ok, ""),
            ("No Hangar criado", bool(ident_h), f"{st_h} {base.codigo(corpo_h)}"), ("eco no No Hangar", ok_h, "")])

    def t_term_nome(self):
        st, corpo, *_ = self.api("POST", "/api/sessions/b1/shell", timeout=30)
        nome = (corpo or {}).get("shell") if st == 200 else None
        ok = False
        if nome:
            ws = self.ws(f"/api/sessions/{nome}/term")
            ok = self.eco(ws, "PROVA-TERMNOME-1")
            ws.close()
        self.conferir("28", "term-<nome> (shell escondido da sessão)", [
            ("shell criado", nome == "term-b1", f"{st} {nome}"), ("eco", ok, "")])

    def t_ping(self):
        """Cliente que some sem fechar: não responde ping. O Rust fecha e desmonta em ~40 s."""
        caminho = f"/api/sessions/b1/term?token={self.token}&cols=90&rows=25"
        s = socket.create_connection(("127.0.0.1", self.porta), timeout=10)
        chave = base64.b64encode(os.urandom(16)).decode()
        s.sendall((f"GET {caminho} HTTP/1.1\r\nHost: 127.0.0.1:{self.porta}\r\nUpgrade: websocket\r\n"
                   f"Connection: Upgrade\r\nSec-WebSocket-Key: {chave}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        try:
            resp = s.recv(256)
        except OSError:
            s.close()
            raise
        aberto = b" 101 " in resp
        anexou = esperar(lambda: self.clientes("b1"), 10, 0.3)
        tam = self.tamanho("b1")
        t0 = time.monotonic()
        sumiu = esperar(lambda: self.clientes("b1") == [], 75, 0.5)
        dur = round(time.monotonic() - t0, 1)
        time.sleep(0.6)
        s.close()
        self.conferir("28", "queda de rede (ping sem resposta)", [
            ("upgrade 101", aberto, ""), ("anexou com o tamanho do cliente", bool(anexou) and tam == "90x24", tam),
            ("desmonta entre 20 e 60 s", bool(sumiu) and 20 <= dur <= 60, f"{dur} s"),
            ("tamanho reposto", self.tamanho("b1") == "200x50", self.tamanho("b1"))])

    def t_queda(self):
        ws = self.ws("/api/sessions/b1/term", cols=110, rows=33)
        ok = self.eco(ws, "PROVA-QUEDA-1")
        tam = self.tamanho("b1")
        guardado = self.opcao_tamanho("b1")
        velho = self.rust_pid()
        if not velho:
            self.registrar("28", "queda do Rust com o painel aberto", False, "sem hangar-server")
            return
        os.kill(velho, signal.SIGKILL)
        fech = self.fechamento(ws, 15)
        novo = esperar(lambda: (lambda p: p if p and p != velho else 0)(self.rust_pid()), 30)
        time.sleep(3)
        sumiu = esperar(lambda: self.clientes("b1") == [], 20, 0.5)
        reposto = esperar(lambda: self.tamanho("b1") == "200x50", 20, 0.5)
        tam_reposto, opcao_depois = self.tamanho("b1"), self.opcao_tamanho("b1")
        ws2 = self.ws("/api/sessions/b1/term")
        ok2 = self.eco(ws2, "PROVA-QUEDA-2")
        ws2.close()
        self.conferir("28", "queda do Rust com o painel aberto", [
            ("eco antes", ok, ""), ("tamanho do painel e opção gravada", tam == "110x32" and bool(guardado), f"{tam}, {guardado}"),
            ("cliente fecha", fech is not None, str(fech)), ("Rust novo", bool(novo), f"{velho} → {novo}"),
            ("cliente tmux órfão sai", bool(sumiu), ""), ("tamanho reposto pelo Rust novo", bool(reposto), tam_reposto),
            ("@hangar_term_size limpo", opcao_depois == "", opcao_depois or "vazio"),
            ("painel reabre e ecoa", ok2, "")])

    # ---- Step 29: carga e reserva ------------------------------------------------------------
    def caso_29(self):
        ns = [int(x) for x in self.args.n.split(",")]
        criadas = []
        py, rs = self.main_pid(), self.rust_pid()
        if not (py and rs):
            self.registrar("29", "processos medidos", False, f"pids Python {py}, Rust {rs}")
            return
        # Logo depois de subir, o Rust ainda monta índices (custos, transcripts): a 1ª janela media
        # isso, não o repouso. Espera ele assentar, com teto.
        t0, assentou = time.monotonic(), False
        while time.monotonic() - t0 < 300 and not assentou:
            assentou = self.janela_cpu(py, rs, 0, 10)["rust"] < 20
        self.registrar("29", "Rust assentado antes da 1ª janela", assentou, f"{round(time.monotonic() - t0)} s")
        for n in ns:
            while len(criadas) < n:
                i = len(criadas)
                nome, headless = f"m{i:02d}", i % 2 == 1
                if erro := self.criar_claude(nome, headless):
                    self.registrar("29", f"abrir {nome}", False, erro)
                    return
                criadas.append((nome, headless))
            # O servidor tmux só existe com sessão: o pid sai depois de criá-las.
            tmux_pid = TmuxLog(self).pid()
            if not tmux_pid:
                self.registrar("29", f"{n} sessões: servidor tmux", False, "sem pid")
                return
            if n == ns[0]:
                # O 1º chat aberto dispara o índice de transcripts do Python (~3 s de CPU, uma vez):
                # aquece antes, para a janela medir o chat e não o índice.
                aquece = [self.chat(nome) for nome, _ in criadas]
                esperar(lambda: all(c.ultimo() for c in aquece), 30)
                for c in aquece:
                    c.fechar()
                t1, quieto = time.monotonic(), False
                while time.monotonic() - t1 < 300 and not quieto:
                    j = self.janela_cpu(py, rs, tmux_pid, 10)
                    quieto = j["python"] < 40 and j["rust"] < 20
                self.registrar("29", "Python e Rust assentados depois do 1º chat", quieto, f"{round(time.monotonic() - t1)} s")
            time.sleep(10)
            linha = {}
            linha["repouso"] = self.janela_cpu(py, rs, tmux_pid)
            chats = [self.chat(nome) for nome, _ in criadas]
            com_estado = esperar(lambda: all(c.ultimo() for c in chats), 30)
            time.sleep(8)
            linha["chats"] = self.janela_cpu(py, rs, tmux_pid)
            ativos = [nome for i, (nome, _) in enumerate(criadas) if i % 4 == 0]
            erros = [e for nome in ativos if (e := self.mandar(nome, CARGA))]
            trabalharam = esperar(lambda: all(self.estado(x) == "working" for x in ativos), 30, 0.5)
            time.sleep(3)
            linha["ativo"] = self.janela_cpu(py, rs, tmux_pid)
            # A janela "ativo" só vale se as sessões ainda trabalhavam no fim dela.
            ainda = sum(1 for x in ativos if self.estado(x) == "working")
            for nome in ativos:
                self.api("POST", f"/api/sessions/{nome}/interrupt", timeout=30)
            for c in chats:
                c.fechar()
            esperar(lambda: all(self.estado(x) == "idle" for x in ativos), 60, 1)
            sm, pb = self.python_estado({x for x, h in criadas if not h})
            sm = {k: v for k, v in sm.items() if not k.endswith("|claude_headless")}
            self.carga.append((n, len(ativos), linha))
            ok = not sm and not pb and bool(com_estado) and bool(trabalharam) and not erros and ainda == len(ativos)
            self.registrar("29", f"{n} sessões (metade com terminal): CPU ms/s Python+filhos / Rust / tmux", ok,
                           "; ".join(f"{k} {v['python']} / {v['rust']} / {v['tmux']}" for k, v in linha.items())
                           + f"; todos os chats com state: {'sim' if com_estado else 'não'}; ativas {len(ativos)}, "
                           f"working no fim da janela {ainda}{f', envio {erros}' if erros else ''}; "
                           f"StateMonitor/PreviewBroker Python com terminal {sm or 0}/{pb or 0}")
        self.fechar([x for x, _ in criadas])

    def janela_cpu(self, py, rs, tm, segundos=30.0):
        pids = {"python": py, "rust": rs, "tmux": tm}
        antes = {k: cpu(p) for k, p in pids.items()}
        t0 = time.monotonic()
        time.sleep(segundos)
        dur = time.monotonic() - t0
        return {k: round((cpu(p) - antes[k]) / dur * 1000, 1) for k, p in pids.items()}

    def volta_python(self, rotulo):
        """Estado de Claude com terminal e terminal real servidos pelo Python (reserva ou desistência)."""
        if not self.sessao("ct") and (erro := self.criar_claude("ct")):
            self.registrar("29", rotulo, False, erro)
            return
        if "b1" not in self.vivas:
            self.sessao_bash("b1")
        sm0, _ = self.python_estado({"ct"})
        chat = self.chat("ct")
        t0 = time.monotonic()
        esperar(lambda: chat.desde(t0, "state"), 20, 0.3)
        self.mandar("ct", "Responda só OK.")
        ciclo = esperar(lambda: "working" in chat.estados(t0) and chat.ultimo() == "idle", 120, 0.5)
        sm1, _ = self.python_estado({"ct"})
        pty0 = self.pty_python()
        ws = self.ws("/api/sessions/b1/term")
        ok = self.eco(ws, "PROVA-RESERVA-1")
        ws.close()
        sumiu = esperar(lambda: self.clientes("b1") == [], 15, 0.3)
        c = self.ler_contador()
        chat.fechar()
        self.conferir("29", rotulo, [
            ("modo python", c.get("modo") == "python", str(c.get("modo"))),
            ("chat working → idle", bool(ciclo), ""),
            ("StateMonitor Python para ct", sum(sm1.values()) > sum(sm0.values()), f"{sm0 or 0} → {sm1}"),
            ("eco no terminal", ok, ""), ("PTY aberto pelo Python", self.pty_python() > pty0, ""),
            ("cliente tmux sai", bool(sumiu), "")])

    def quedas(self):
        log0 = len(self.log.read_text(errors="replace"))   # a queda do Step 28 já escreveu no log
        mortos = []
        t0 = time.monotonic()
        for i in range(3):
            pid = esperar(self.rust_pid, 30)
            if not pid:
                break
            os.kill(pid, signal.SIGKILL)
            mortos.append(pid)
            if i < 2:
                esperar(lambda: self.rust_pid() not in (0, pid), 30)
        assumiu = esperar(lambda: "o Python assume a porta" in self.log.read_text(errors="replace")[log0:], 30)
        self.registrar("29", "Rust derrubado 3 vezes em 60 s", bool(assumiu) and len(mortos) == 3,
                       f"{len(mortos)} kills em {round(time.monotonic() - t0)} s; Python assumiu: {'sim' if assumiu else 'não'}")
        time.sleep(6)
        self.volta_python("depois de 3 quedas: estado e terminal pelo Python")


ORDEM = ["26", "27", "28", "29"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--casos", default=",".join(ORDEM))
    ap.add_argument("--n", default="1,10,20", help="sessões do Step 29")
    ap.add_argument("--conta-b", help="segunda conta Claude com login (~/.claude-jefferson)")
    ap.add_argument("--codex", default="~/.codex-claude-200-2", help="conta Codex cujo login é copiado")
    ap.add_argument("--reserva", action="store_true", help="CP_RUST_SERVER=0: só a volta pelo Python")
    ap.add_argument("--relatorio")
    ap.add_argument("--manter", action="store_true")
    args = ap.parse_args()
    casos = [c for c in ORDEM if c in args.casos.split(",")]
    for sinal in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sinal, lambda *_: sys.exit(1))
    p = Prova(args)
    p.carga = []
    print(f"prova em {p.raiz}: porta {p.porta}, convite {p.porta_convite}, "
          f"tmux -L {p.tmux}, reserva {args.reserva}", flush=True)
    lista = troca.ListaSSE(p)
    try:
        p.preparar()
        p.subir()
        rust = esperar(lambda: "hangar-server de pé" in p.log.read_text(errors="replace"), 30)
        if args.reserva == bool(rust):
            raise SystemExit(f"Rust na frente: {bool(rust)}, esperado {not args.reserva}")
        lista.start()
        time.sleep(3)
        p.confiar_pasta()
        if args.reserva:
            p.volta_python("reserva CP_RUST_SERVER=0: estado e terminal pelo Python")
        else:
            for caso in casos:
                try:
                    getattr(p, f"caso_{caso}")()
                except Exception as e:
                    p.registrar(caso, "execução do caso", False, f"{type(e).__name__}: {str(e)[:100]}")
                    traceback.print_exc()
            for caso, fim in (("26", p.sugestoes), ("29", p.quedas)):
                if caso in casos:
                    try:
                        fim()
                    except Exception as e:
                        p.registrar(caso, fim.__name__, False, f"{type(e).__name__}: {str(e)[:100]}")
                        traceback.print_exc()
    finally:
        lista.parar = True
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
