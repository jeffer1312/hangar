#!/usr/bin/env python3
"""Sombra da lista com sessões reais no backend isolado (`docs/migracao-rust/pedidos/
2026-10-05-lista-estado-sombra-isolada.md`, no lugar do Step 34).

Sobe o backend desta árvore isolado (molde e limpeza de `prova-dono-unico.py`) com o Rust em release
e `CP_LIST_SHADOW=1`, mantém a lista do dono aberta (sem ela o Python não tem lista para comparar),
roda os cenários e, para cada um, lista as diferenças que o Rust gravou no diário
(`rust.list_shadow_diff`/`_failed`/`_blind`, sessão + campo) e o tempo por atualização da lista.

    scripts/prova-lista-sombra.py [--cenarios nascer,...] [--conta-b ~/.claude-outra] [--relatorio f.md] [--manter]

Claude só em Haiku (conta 02-200 e a `--conta-b` na troca de conta); Codex em gpt-6-luna, numa
`CODEX_HOME` temporária com a cópia do login da conta Codex indicada (`--codex`). Instrumentação só
no lançador do backend isolado: tempos (Python: início do `_cached_list` ao `_list_sig` do
refresher; Rust: intervalo entre pedidos de fatos da sombra menos o tique de 1,5 s) e, para o
diagnóstico, os valores divergentes num arquivo da pasta temporária, apagada no fim. O relatório só
traz nomes de sessão, nomes de campo, códigos e tempos.
"""
import argparse
import http.client
import importlib.util
import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import threading
import time
import traceback
from pathlib import Path

_spec = importlib.util.spec_from_file_location("prova_dono_unico", Path(__file__).with_name("prova-dono-unico.py"))
base = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(base)
esperar = base.esperar

CODEX_MODELO = "gpt-6-luna"
TICK_RUST = 1.5
# A sombra conta as diferenças e grava uma vez por janela de 60 s: o fim do cenário espera a janela fechar.
ASSENTAR = 9
JANELA_SOMBRA = 63
EVENTOS = ("rust.list_shadow_diff", "rust.list_shadow_failed", "rust.list_shadow_blind")
MEDIO = "Escreva os números de 1 a 120 por extenso, um por linha, sem mais nada."
PERMISSAO = "Use a ferramenta Bash para rodar exatamente: touch prova-permissao.txt"
PERGUNTA = ("Use a ferramenta AskUserQuestion para me perguntar se prefiro A ou B, com as opções A e B. "
            "Não faça mais nada.")

LANCADOR = r'''
import time as _t, json as _j
import app.api as _api
async def _sem_poda():
    return None
_api._prune_loop = _sem_poda
from app import sse as _sse
_py = open({py!r}, "a", buffering=1)
_rs = open({rs!r}, "a", buffering=1)
_dif = open({dif!r}, "a", buffering=1)
_vistos = set()
_ini = {{}}
_cl0 = _sse._cached_list
async def _cl(*a, **k):
    _ini.setdefault("t", _t.monotonic())
    return await _cl0(*a, **k)
_sse._cached_list = _cl
_sig0 = _sse._list_sig
def _sig(infos):
    t = _ini.pop("t", None)
    if t is not None:
        _py.write(f"{{_t.monotonic():.3f}} {{_t.monotonic() - t:.4f}} {{len(infos)}}\n")
    return _sig0(infos)
_sse._list_sig = _sig
'''


class ListaAberta(threading.Thread):
    """A lista do dono aberta o tempo todo, como o app: sem ela o refresher do Python não roda."""

    def __init__(self, prova):
        super().__init__(daemon=True)
        self.p, self.parar = prova, False

    def run(self):
        while not self.parar:
            try:
                c = http.client.HTTPConnection("127.0.0.1", self.p.porta, timeout=40)
                c.request("GET", "/api/sessions/events", headers=self.p.cab())
                r = c.getresponse()
                for _ in r:
                    if self.parar:
                        return
            except (OSError, http.client.HTTPException):
                pass
            time.sleep(0.5)


class Prova(base.Prova):
    PERFIL = "release"

    def __init__(self, args):
        super().__init__(args)
        # Worktree só nasce dentro da raiz autorizada (o HOME do backend).
        self.work = self.home / "work"
        self.codex_origem = Path(args.codex).expanduser()
        self.py_ticks = self.raiz / "py_ticks.txt"
        self.rs_ticks = self.raiz / "rs_ticks.txt"
        self.difs = self.raiz / "difs.jsonl"
        self.resultados = []   # (cenário, {(sessão, campo): evento}, tempos)
        self.vivas = set()

    def preparar(self):
        super().preparar()
        # O `claude` roda na conta real e o hook dele grava marcador, registro nativo, pergunta e
        # statusline nas pastas dela: sem estes links nem o Python nem o Rust os veriam e os dois
        # cairiam no pane, que não é o caminho de uso. A poda fica desligada (lançador): ela apagaria
        # sidecar de sessão real que este backend não conhece.
        for sub in (".hangar-state", "sessions", ".hangar-askq", ".hangar-status"):
            (self.home / ".claude" / sub).symlink_to((base.CONTA / sub).resolve())
        codex = self.home / ".codex"
        codex.mkdir()
        shutil.copy2(self.codex_origem / "auth.json", codex / "auth.json")
        # O `codex` do usuário vem do Node do fnm, fora do PATH do backend isolado; ele precisa do `node` junto.
        fnm = Path.home() / ".local/share/fnm/aliases/default/bin"
        for prog in ("codex", "node"):
            (self.bin / prog).symlink_to((fnm / prog).resolve())
        (codex / "config.toml").write_text(f'model = "{CODEX_MODELO}"\nmodel_reasoning_effort = "low"\n'
                                           "check_for_update_on_startup = false\n")
        self.auth_codex0 = (codex / "auth.json").read_bytes()

    def lancador_extra(self):
        return LANCADOR.format(py=str(self.py_ticks), rs=str(self.rs_ticks), dif=str(self.difs))

    def ambiente_extra(self):
        # CP_LIST_SHADOW_DUMP só tem efeito num binário de diagnóstico (valores fora do diário).
        # A worktree só nasce numa raiz da allowlist do fs: a pasta da prova.
        return {"CP_LIST_SHADOW": "1", "CP_LIST_SHADOW_DUMP": str(self.raiz / "dump.jsonl"),
                "CP_SCAN_ROOTS": str(self.home)}

    def _fechar_tudo(self):
        for nome in self.vivas:
            self.api("DELETE", f"/api/sessions/{nome}", timeout=15)

    def _apagar(self):
        # Login do Codex renovado na cópia: a renovação gira o token, então volta para a conta real.
        auth = self.home / ".codex/auth.json"
        if auth.exists() and auth.read_bytes() != self.auth_codex0:
            shutil.copy2(auth, self.codex_origem / "auth.json")
            print("login Codex renovado na prova: copiado de volta para a conta", flush=True)
        # A worktree nasce dentro da pasta da prova: a transcrição dela tem o nome desta como prefixo.
        prefixo = base.re.sub(r"[^A-Za-z0-9]", "-", str(self.work))
        for raiz in self.raizes_projetos():
            for pasta in raiz.glob(prefixo + "?*"):
                if pasta.is_dir() and not pasta.is_symlink():
                    shutil.rmtree(pasta, ignore_errors=True)
        super()._apagar()

    # ---- sessões --------------------------------------------------------------------------
    def nova(self, nome, provider="claude", headless=False, **extra):
        corpo = {"name": nome, "cwd": str(extra.pop("cwd", self.work)), "headless": headless, "provider": provider,
                 **extra}
        if provider == "codex":
            corpo["model"] = CODEX_MODELO
        st, resp, *_ = self.api("POST", "/api/sessions", corpo, timeout=180)
        if st != 200:
            return f"criar {nome} {st} {base.codigo(resp)}"
        self.vivas.add(nome)
        if not headless and provider == "claude":
            self.responder_confianca(nome, 8)
        elif not headless:
            self.confiar_hooks_codex(nome)
        return None

    def confiar_hooks_codex(self, nome):
        """A TUI do Codex da prova abre com seletores antes do composer: confiança nos hooks do Hangar
        ("Trust all") e aviso de versão nova ("Skip until next version"). Responde como o usuário até
        o composer aparecer."""
        teclas = {"Hooks need review": ("Down", "Enter"), "Update available": ("Down", "Down", "Enter")}
        fim = time.monotonic() + 90
        while time.monotonic() < fim:
            tela = self.tela(nome)
            if "Ask Codex to do anything" in tela:
                return
            alvo = next((t for t in teclas if t in tela), None)
            if alvo:
                pane, _ = self.pane(nome)
                subprocess.run(["/usr/bin/tmux", "-L", self.tmux, "send-keys", "-t", pane, *teclas[alvo]], capture_output=True)
                esperar(lambda: alvo not in self.tela(nome), 15)
            time.sleep(1)

    def mandar(self, nome, texto):
        st, corpo, *_ = self.api("POST", f"/api/sessions/{nome}/input", {"text": texto}, timeout=120)
        return None if st == 200 else f"enviar {nome} {st} {base.codigo(corpo)}"

    def estado(self, nome):
        return (self.sessao(nome) or {}).get("state")

    def esperar_estado(self, nome, estados, timeout):
        return esperar(lambda: self.estado(nome) in estados, timeout, 0.5)

    def fechar(self, nomes):
        for nome in nomes:
            self.api("DELETE", f"/api/sessions/{nome}", timeout=30)
            self.vivas.discard(nome)

    # ---- medição ------------------------------------------------------------------------------
    def janela(self):
        return {"diario": {f: f.stat().st_size for f in self.diarios()},
                "py": self._linhas(self.py_ticks), "rs": self._linhas(self.rs_ticks), "dif": self._linhas(self.difs)}

    @staticmethod
    def _linhas(f):
        return len(f.read_text().splitlines()) if f.exists() else 0

    def fechar_janela(self, nome, j, notas):
        time.sleep(JANELA_SOMBRA)
        achados = {}
        for f in self.diarios():
            with f.open("rb") as h:
                h.seek(j["diario"].get(f, 0))
                for bruta in h.read().decode(errors="replace").splitlines():
                    try:
                        o = json.loads(bruta)
                    except ValueError:
                        continue
                    if o.get("evento") in EVENTOS:
                        achados[(o.get("sessao") or "—", o.get("codigo") or "—")] = o["evento"].removeprefix("rust.list_shadow_")
        py = [float(l.split()[1]) for l in self.py_ticks.read_text().splitlines()[j["py"]:]] if self.py_ticks.exists() else []
        rs_stamps = [float(l.split()[0]) for l in self.rs_ticks.read_text().splitlines()[j["rs"]:]] if self.rs_ticks.exists() else []
        rs = [b - a - TICK_RUST for a, b in zip(rs_stamps, rs_stamps[1:]) if b - a < 5]   # sem as pausas de 10 s
        tempos = {"py": py, "rs": rs}
        self.resultados.append((nome, achados, tempos, notas))
        print(f"[{nome}] diferenças {sorted(achados.items()) or 'nenhuma'}; {self.fmt(tempos)}; {notas or ''}", flush=True)

    @staticmethod
    def fmt(t):
        def m(v):
            return f"mediana {statistics.median(v) * 1000:.0f} ms, máx {max(v) * 1000:.0f} ms ({len(v)})" if v else "—"
        return f"Python {m(t['py'])}; Rust {m(t['rs'])}"

    def cenario(self, nome, corpo):
        j = self.janela()
        notas = []
        try:
            corpo(notas)
        except Exception as e:
            notas.append(f"cenário quebrou: {type(e).__name__}")
            traceback.print_exc()
        self.fechar_janela(nome, j, "; ".join(notas))

    # ---- cenários -----------------------------------------------------------------------------
    BASE = {"ct": ("claude", False), "ch": ("claude", True), "xt": ("codex", False), "xh": ("codex", True)}

    def c_nascer(self, notas):
        for nome, (prov, h) in self.BASE.items():
            if erro := self.nova(nome, prov, h):
                notas.append(erro)
        for nome in self.BASE:
            if nome in self.vivas and (erro := self.mandar(nome, "Responda só OK.")):
                notas.append(erro)
        for nome in self.BASE:
            if nome in self.vivas and not self.esperar_estado(nome, {"idle"}, 120):
                notas.append(f"{nome} não parou ({self.estado(nome)})")

    def c_trabalhando(self, notas):
        for nome in self.BASE:
            if nome in self.vivas:
                self.mandar(nome, MEDIO)
        vistas = [n for n in self.BASE if n in self.vivas and self.esperar_estado(n, {"working"}, 30)]
        notas.append(f"trabalhando vistas: {vistas}")
        for nome in self.BASE:
            if nome in self.vivas and not self.esperar_estado(nome, {"idle"}, 180):
                notas.append(f"{nome} não parou ({self.estado(nome)})")

    def c_permissao(self, notas):
        for nome, h in (("pt", False), ("ph", True)):
            if erro := self.nova(nome, "claude", h, permission_mode="manual"):
                notas.append(erro)
                continue
            self.mandar(nome, PERMISSAO)
        # O Codex do Hangar abre em modo YOLO: não há cartão de permissão para provocar.
        for nome in ("pt", "ph"):
            if nome in self.vivas:
                ok = self.esperar_estado(nome, {"awaiting_input"}, 90)
                notas.append(f"{nome} {'em' if ok else 'sem'} cartão ({self.estado(nome)})")
        time.sleep(12)   # parada no cartão por várias rodadas
        for nome in ("pt", "ph"):
            if nome in self.vivas:
                self.api("POST", f"/api/sessions/{nome}/interrupt", timeout=30)

    def c_pergunta(self, notas):
        for nome in ("ct", "ch"):
            self.mandar(nome, PERGUNTA)
        for nome in ("ct", "ch"):
            ok = self.esperar_estado(nome, {"awaiting_input"}, 90)
            notas.append(f"{nome} {'com' if ok else 'sem'} pergunta ({self.estado(nome)})")
        time.sleep(12)
        for nome in ("ct", "ch"):
            self.api("POST", f"/api/sessions/{nome}/interrupt", timeout=30)
            self.esperar_estado(nome, {"idle"}, 30)

    def c_clear(self, notas):
        for nome in ("ct", "ch"):
            if erro := self.mandar(nome, "/clear"):
                notas.append(erro)
        time.sleep(6)
        for nome in ("ct", "ch"):
            self.mandar(nome, "Responda só OK.")
            if not self.esperar_estado(nome, {"idle"}, 90):
                notas.append(f"{nome} não parou depois do /clear")

    def c_matar(self, notas):
        # Terminal: o servidor tmux da sessão; sem terminal: o cano.
        for nome in ("pt", "xt"):
            if nome in self.vivas:
                subprocess.run(["/usr/bin/tmux", "-L", self.tmux, "kill-session", "-t", nome], capture_output=True)
        meta = self.home / ".hangar/claude-headless/ph.json"
        if meta.exists():
            pid = int((json.loads(meta.read_text()).get("cano") or {}).get("pid") or 0)
            if pid:
                try:
                    os.killpg(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        time.sleep(12)
        for nome in ("pt", "xt", "ph"):
            notas.append(f"{nome}: {self.estado(nome) or 'fora da lista'}")

    def c_recriar(self, notas):
        self.fechar(["pt", "ph", "xt"])
        for nome, (prov, h) in (("ch", ("claude", True)), ("xt", ("codex", False)), ("ct", ("claude", False))):
            self.fechar([nome])
            if erro := self.nova(nome, prov, h):
                notas.append(erro)
                continue
            self.mandar(nome, "Responda só OK.")
        for nome in ("ch", "xt", "ct"):
            if nome in self.vivas and not self.esperar_estado(nome, {"idle"}, 120):
                notas.append(f"{nome} não parou")

    def c_par_grupo(self, notas):
        st, corpo, *_ = self.api("POST", "/api/sessions/ct/pair", {"peer": "xt", "task": "prova"})
        notas.append(f"par {st} {base.codigo(corpo)}")
        time.sleep(ASSENTAR)
        st, corpo, *_ = self.api("POST", "/api/sessions/ct/pair", {"peers": ["ch", "xh"], "task": "prova"})
        notas.append(f"grupo {st} {base.codigo(corpo)}")
        for nome in ("ct", "ch", "xt", "xh"):
            self.esperar_estado(nome, {"idle"}, 120)

    def c_worktree(self, notas):
        if erro := self.nova("wt", "claude", False, branch="prova-wt", new_branch=True):
            notas.append(erro)
            return
        self.mandar("wt", "Responda só OK.")
        if not self.esperar_estado("wt", {"idle"}, 120):
            notas.append("wt não parou")
        s = self.sessao("wt") or {}
        notas.append(f"branch {s.get('branch')!r}, worktree {s.get('worktree')}")

    def c_plano(self, notas):
        planos = self.work / "docs/superpowers/plans"
        planos.mkdir(parents=True, exist_ok=True)
        stem = "2026-10-05-prova-lista"
        (planos / f"{stem}.md").write_text(
            "# Prova\n\n### Task 1: Um\n\n- [ ] **Step 1: a**\n- [ ] **Step 2: b**\n\n"
            "### Task 2: Dois\n\n- [ ] **Step 3: c**\n")
        time.sleep(5)
        st, *_ = self.api("POST", "/api/sessions/ct/plan-step", {"stem": stem, "idx": 0, "done": True})
        notas.append(f"step {st}")
        time.sleep(5)
        (planos / "2026-10-05-outro.md").write_text("# Outro\n\n### Task 1: X\n\n- [ ] **Step 1: x**\n")
        st, *_ = self.api("POST", "/api/sessions/ct/plan-pin", {"stem": stem})
        notas.append(f"pino {st}")
        s = self.sessao("ct") or {}
        notas.append(f"barra {s.get('plan_done')}/{s.get('plan_total')}")

    def c_conta(self, notas):
        if not self.conta_b:
            notas.append("pulado: falta --conta-b")
            return
        st, corpo, *_ = self.api("POST", "/api/sessions/ct/conta", {"config_dir": str(self.home / ".claude-provab")},
                                 timeout=240)
        notas.append(f"troca {st} {base.codigo(corpo)}")
        self.responder_confianca("ct", 20)
        self.esperar_estado("ct", {"idle"}, 120)
        self.mandar("ct", "Responda só OK.")
        self.esperar_estado("ct", {"idle"}, 120)
        conta = str((self.sessao("ct") or {}).get("conta") or "")
        notas.append(f"conta na lista: {Path(conta.split(':', 1)[-1]).name or '—'}")

    def c_restart(self, notas):
        dur, kill = self.reiniciar()
        notas.append(f"restart {dur} s, SIGKILL {'sim' if kill else 'não'}")
        if not esperar(lambda: "hangar-server de pé" in self.log.read_text(errors="replace").rsplit("Uvicorn running", 1)[-1], 30):
            notas.append("Rust não voltou na frente")
        time.sleep(10)
        vivas = sorted(n for n in self.vivas if self.sessao(n))
        notas.append(f"na lista depois: {vivas}")


ORDEM = ["nascer", "trabalhando", "permissao", "pergunta", "clear", "matar", "recriar", "par_grupo",
         "worktree", "plano", "conta", "restart"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--cenarios", default=",".join(ORDEM))
    ap.add_argument("--conta-b", help="segunda conta Claude com login, destino da troca de conta "
                    "(~/.claude-jefferson; claude-200-1 e claude-200-3 são recusadas)")
    ap.add_argument("--codex", default="~/.codex-claude-200-2", help="conta Codex cujo login é copiado")
    ap.add_argument("--relatorio", help="grava a tabela neste arquivo")
    ap.add_argument("--manter", action="store_true", help="não apaga a pasta da prova")
    ap.add_argument("--interativo", action="store_true", help="só sobe e espera; os cenários ficam à mão")
    args = ap.parse_args()
    cenarios = [c for c in ORDEM if c in args.cenarios.split(",")]
    for sinal in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sinal, lambda *_: sys.exit(1))
    p = Prova(args)
    print(f"prova em {p.raiz}: porta {p.porta}, tmux -L {p.tmux}, unit {p.unit}", flush=True)
    lista = ListaAberta(p)
    try:
        p.preparar()
        p.subir()
        if not esperar(lambda: "hangar-server de pé" in p.log.read_text(errors="replace"), 30):
            raise SystemExit("o Python subiu sem o Rust na frente")
        lista.start()
        time.sleep(12)   # a sombra só compara com a lista do Python já servida
        if args.interativo:
            p.confiar_pasta()
            print(f"interativo: porta {p.porta} token {p.token} tmux -L {p.tmux}; SIGTERM encerra", flush=True)
            signal.pause()
        p.confiar_pasta()
        for c in cenarios:
            p.cenario(c, getattr(p, f"c_{c}"))
        # Para o diagnóstico de quem roda com --manter: os valores ficam na pasta da prova.
    finally:
        lista.parar = True
        try:
            p.limpar()
        finally:
            linhas = ["| Cenário | Diferenças (sessão: campo → evento) | Tempo por atualização | Notas |", "|---|---|---|---|"]
            for nome, achados, tempos, notas in p.resultados:
                difs = ", ".join(f"{s}: {c} → {e}" for (s, c), e in sorted(achados.items())) or "nenhuma"
                linhas.append(f"| {nome} | {difs} | {p.fmt(tempos)} | {notas.replace('|', '/')} |")
            todos = {"py": [], "rs": []}
            for *_, tempos, _ in p.resultados:
                for k in todos:
                    todos[k] += tempos[k]
            tabela = "\n".join(linhas) + f"\n\nTotal: {p.fmt(todos)}\n"
            print("\n" + tabela)
            if args.relatorio:
                Path(args.relatorio).write_text(tabela)


if __name__ == "__main__":
    main()
