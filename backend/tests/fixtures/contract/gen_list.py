"""Entradas e saídas gravadas da lista de sessões, para o Rust repetir tique a tique.

Uso, de backend/: uv run python tests/fixtures/contract/gen_list.py [pasta]   (padrão: golden/)

Roda `SessionRegistry.list` e `list_with_state` de hoje num filho com HOME temporário e ambiente
limpo. `procinfo`, `tmux`, relógio e os adaptadores com estado em memória são trocados por fakes
que registram cada leitura; o resto (marcadores, sidecars, transcripts, planos) é arquivo de
verdade, criado pelas operações `fs` de cada tique. Cada caso é uma sequência de tiques sobre o
mesmo registro, porque os caches dependem da ordem. Nenhum texto real de conversa.
"""
import asyncio
import json
import logging
import os
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parent
BACKEND = HERE.parents[2]
NAMES = ("list_discovery.json", "list_decorate.json", "list_state.json")

ROOT, SAN = "⟦ROOT⟧", "⟦ROOT_SAN⟧"
HOME = ROOT + "/home"
CFG = HOME + "/.claude"
ALT = HOME + "/.claude-alt"
T0 = 1_800_000_000.0
# monotonic = wall − MONO_OFF: grande o bastante para os caches que começam em 0.0 já estarem vencidos.
MONO_OFF = 1_700_000_000.0

ABOUT = {
    "list_discovery.json": "registry.list(): só descoberta e resolução do transcript (estado padrão).",
    "list_decorate.json": "registry.list_with_state(): marcadores, registro nativo, pergunta aberta, "
                          "statusline do sidecar, contexto, plano, loop, última resposta, sem terminal.",
    "list_state.json": "registry.list_with_state(): classificação pelo pane, segunda captura, rebaixar "
                       "awaiting, _idle_conferido, teto e validade da statusline, limite.",
}
CONVENTIONS = [
    "⟦ROOT⟧ é a pasta do caso; ⟦ROOT_SAN⟧ é sanitize_cwd(⟦ROOT⟧); a pasta do Pi é '-' + ela + o resto + '--'.",
    "Cada caso começa com todos os caches vazios e um registro só, que atravessa os tiques.",
    "Tique: aplica `fs` (write grava texto e mtime, padrão `at`; rm; mkdir; touch), fixa o relógio em `at` "
    "(monotonic = at - mono_off), aplica `ops`, relê do disco os marcadores e o registro nativo e roda.",
    "ops: seed = _jsonl_cache[name] = jsonl; forget = _forget(name); rename = move jsonl, fd_locked, "
    "statusline, label, limite e última resposta para o nome novo; fresh = invalida o mapa de filhos.",
    "`procs` é o /proc do tique; o mapa pai→filhos tem cache de 3 s no relógio (processo que nasce some "
    "dele até vencer ou até `fresh`), em ordem crescente de pid; a descida é em pilha (o último filho "
    "primeiro). O pid raiz do pane entra sempre, mesmo fora do mapa. pid vivo = está em `procs`.",
    "`panes` é a saída de list-panes -a já separada; `tmux_down` faz list-panes levantar MuxIndisponivel.",
    "`captures`: cada captura do nome consome o primeiro quadro; o último se repete; nome sem quadro captura ''.",
    "`facts`: headless_snapshot (estado do runtime sem terminal), headless_problem e runtime_problem "
    "(código em lista); ausente = None. O Codex vivo devolve sempre nada (vem dos fatos do Python).",
    "`expected.rows` omite campo igual ao de `defaults`. `tmux` e `proc_reads` são informativos (detalhe do "
    "Python, não contrato). Caches `[idade em s, valor]`, idade arredondada em 3 casas. `effects` é o que o "
    "Python fez fora da lista (demote_awaiting), ordenado.",
    "asyncio.sleep avança o relógio. Git, varredura de pares mortos, orq e transferências ficam fora (vêm "
    "de fatos ou de outro laço). O rebaixamento só em memória do registro nativo não atravessa tiques.",
]


def san(path: str) -> str:
    assert path.startswith(ROOT), path
    return SAN + re.sub(r"[^A-Za-z0-9]", "-", path[len(ROOT):])


def proj(cwd: str, cfg: str = CFG) -> str:
    return f"{cfg}/projects/{san(cwd)}"


def work(name: str) -> str:
    return f"{ROOT}/work/{name}"


# ── operações de arquivo ──────────────────────────────────────────────────────────────────────

def W(path, text, mtime=None):
    return dict(op="write", path=path, text=text, mtime=mtime)


def J(path, obj, mtime=None):
    return W(path, json.dumps(obj, ensure_ascii=False), mtime)


def RM(path):
    return dict(op="rm", path=path)


def MK(path):
    return dict(op="mkdir", path=path)


def TOUCH(path, mtime):
    return dict(op="touch", path=path, mtime=mtime)


def P(pid, ppid, argv, cwd, env=None, start=T0 - 600, fds=()):
    return dict(pid=pid, ppid=ppid, argv=list(argv), cwd=cwd, env=dict(env or {}), start=start,
                fds=list(fds))


def PANE(name, pid, cwd, pane_id="%1", active=True, hidden=False, provider=None, created=int(T0 - 600)):
    return dict(name=name, pid=pid, cwd=cwd, pane_id=pane_id, active=active, hidden=hidden,
                provider=provider, session_created=created)


def user_line(text, cwd, ts="2027-01-15T08:00:00.000Z"):
    return json.dumps({"type": "user", "message": {"role": "user", "content": text}, "cwd": cwd,
                       "timestamp": ts, "uuid": "u-" + text[:8]}, ensure_ascii=False)


def reply_line(text, cwd, used=0, model="claude-opus-4-5", ts="2027-01-15T08:00:05.000Z", extra=None):
    message = {"role": "assistant", "model": model, "content": [{"type": "text", "text": text}]}
    if used:
        message["usage"] = {"input_tokens": used // 2, "cache_read_input_tokens": used - used // 2,
                            "cache_creation_input_tokens": 0, "output_tokens": 10}
    obj = {"type": "assistant", "message": message, "cwd": cwd, "timestamp": ts,
           "uuid": "a-" + text[:8], **(extra or {})}
    return json.dumps(obj, ensure_ascii=False)


def transcript(cwd, *pairs, used=0, model="claude-opus-4-5"):
    lines = []
    for q, a in pairs:
        lines += [user_line(q, cwd), reply_line(a, cwd, used, model)]
    return "\n".join(lines) + "\n"


CODEX_ROLLOUT = (f"{HOME}/.codex/sessions/2027/01/15/"
                 "rollout-2027-01-15T08-00-00-0199aaaa-bbbb-7ccc-8ddd-eeeeffff0001.jsonl")


def codex_rollout(cwd):
    def item(ts, payload_type, payload):
        return json.dumps({"timestamp": ts, "type": payload_type, "payload": payload}, ensure_ascii=False)
    return "\n".join([
        item("2027-01-15T08:00:00.000Z", "session_meta",
             {"id": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0001", "cwd": cwd, "cli_version": "0.151.0"}),
        item("2027-01-15T08:00:01.000Z", "response_item",
             {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "pergunta"}]}),
        item("2027-01-15T08:00:04.000Z", "response_item",
             {"type": "message", "role": "assistant",
              "content": [{"type": "output_text", "text": "resposta do codex"}]}),
    ]) + "\n"


def tick(at, **kw):
    return dict(at=T0 + at, **kw)


SHELL = ["/bin/bash"]
IDLE = "● resposta curta\n────────────\n❯\n────────────\n🤖 Opus 4.5 │ 💬 1k/2k 50k/200k"
SPIN = "✻ Pensando… (%ds)\n────────────\n❯\n────────────\n🤖 Opus 4.5 │ 💬 1k/2k 60k/200k"
MENU = ("☐ Escolha\nQual caminho?\n❯ 1. Primeiro\n  2. Segundo\nEnter to select\n────────────\n"
        "🤖 Opus 4.5 │ 💬 1k/2k 50k/200k")
LIMIT = ("● resposta curta\n────────────\n❯\n────────────\n🤖 Opus 4.5 │ 💬 1k/2k 50k/200k\n"
         "Usage limit reached · resets 9:10pm")


# ── casos: descoberta ─────────────────────────────────────────────────────────────────────────

def discovery_cases():
    A, B = work("a"), work("b")
    pa = proj(A)
    cases = {}

    # fd aberto vence; some no meio de uma escrita e o _fd_locked segura; marcador do sid destrava.
    sid = "11111111-1111-4111-8111-111111111111"
    procs = [P(100, 1, SHELL, A), P(101, 100, ["claude", "--session-id", sid], A, fds=[f"{pa}/c0000001-0000-4000-8000-000000000000.jsonl"])]
    cases["claude_fd_open_then_locked"] = [
        tick(0, fs=[MK(A), W(f"{pa}/c0000001-0000-4000-8000-000000000000.jsonl", user_line("um", A) + "\n", T0 - 5),
                    W(f"{pa}/{sid}.jsonl", user_line("boot", A) + "\n", T0 - 50)],
             procs=procs, panes=[PANE("s", 100, A)]),
        tick(2, fs=[W(f"{pa}/c0000003-0000-4000-8000-000000000000.jsonl", user_line("outra", A) + "\n", T0 + 2)],
             procs=[procs[0], {**procs[1], "fds": []}], panes=[PANE("s", 100, A)]),
        tick(4, fs=[J(f"{CFG}/.hangar-active/{sid}.json", {"jsonl": f"{pa}/c0000003-0000-4000-8000-000000000000.jsonl"})],
             procs=[procs[0], {**procs[1], "fds": []}], panes=[PANE("s", 100, A)]),
    ]

    # fd de subagente (--agent) e de daemon não contam; fd de processo que não é agente não é lido.
    cases["claude_fd_aux_ignored"] = [
        tick(0, fs=[MK(A), W(f"{pa}/{sid}.jsonl", user_line("um", A) + "\n", T0 - 50),
                    W(f"{pa}/c000000b-0000-4000-8000-000000000000.jsonl", user_line("sub", A) + "\n", T0 - 1)],
             procs=[P(100, 1, SHELL, A), P(101, 100, ["claude", "--session-id", sid], A),
                    P(102, 101, ["claude", "--agent", "x", "--session-id", "22222222-2222-4222-8222-222222222222"], A,
                      fds=[f"{pa}/c000000b-0000-4000-8000-000000000000.jsonl"]),
                    P(103, 101, ["node", "mcp.js"], A, fds=[f"{pa}/c000000b-0000-4000-8000-000000000000.jsonl"])],
             panes=[PANE("s", 100, A)]),
    ]

    # --session-id sem irmão: arquivo ainda inexistente vale; depois do /clear segue o mais novo.
    sid2 = "33333333-3333-4333-8333-333333333333"
    base = [P(200, 1, ["claude", "--session-id", sid2], B)]
    pb = proj(B)
    cases["claude_session_id_then_clear"] = [
        tick(0, fs=[MK(B), W(f"{pb}/c000000d-0000-4000-8000-000000000000.jsonl", user_line("velho", B) + "\n", T0 - 900)],
             procs=base, panes=[PANE("b", 200, B)]),
        tick(2, fs=[W(f"{pb}/{sid2}.jsonl", user_line("boot", B) + "\n", T0 + 1)],
             procs=base, panes=[PANE("b", 200, B)]),
        tick(4, fs=[W(f"{pb}/c0000009-0000-4000-8000-000000000000.jsonl", user_line("depois", B) + "\n", T0 + 3)],
             procs=base, panes=[PANE("b", 200, B)]),
    ]

    # Com irmão no mesmo cwd o <sid>.jsonl é direto: o mais novo de uma não contamina a outra.
    s1, s2 = "44444444-4444-4444-8444-444444444444", "55555555-5555-4555-8555-555555555555"
    cases["claude_session_id_with_sibling"] = [
        tick(0, fs=[MK(B), W(f"{pb}/{s1}.jsonl", user_line("x", B) + "\n", T0 - 10),
                    W(f"{pb}/{s2}.jsonl", user_line("y", B) + "\n", T0 - 20),
                    W(f"{pb}/c0000007-0000-4000-8000-000000000000.jsonl", user_line("z", B) + "\n", T0 - 1)],
             procs=[P(300, 1, ["claude", "--session-id", s1], B), P(310, 1, ["claude", "--session-id", s2], B)],
             panes=[PANE("x", 300, B, "%3"), PANE("y", 310, B, "%4"),
                    PANE("escondida", 320, B, "%5", hidden=True)]),
    ]

    # Sessão sem --session-id: marcador casado por pid (o mais novo vence); sem marcador, cache;
    # sem cache, o mais novo do cwd e untracked.
    C = work("c")
    pc = proj(C)
    bare = [P(400, 1, SHELL, C), P(401, 400, ["claude"], C)]
    cases["claude_marker_by_pid_cache_newest"] = [
        tick(0, fs=[MK(C), W(f"{pc}/c0000005-0000-4000-8000-000000000000.jsonl", "{}\n", T0 - 30), W(f"{pc}/c0000006-0000-4000-8000-000000000000.jsonl", "{}\n", T0 - 20),
                    J(f"{CFG}/.hangar-active/boot1.json", {"jsonl": f"{pc}/c0000005-0000-4000-8000-000000000000.jsonl", "pid": 401, "ts": T0 - 30}),
                    J(f"{CFG}/.hangar-active/boot2.json", {"jsonl": f"{pc}/c0000006-0000-4000-8000-000000000000.jsonl", "pid": 401, "ts": T0 - 10}),
                    J(f"{CFG}/.hangar-active/boot3.json", {"jsonl": f"{pc}/c0000005-0000-4000-8000-000000000000.jsonl", "pid": 999, "ts": T0})],
             procs=bare, panes=[PANE("c", 400, C)]),
        tick(2, fs=[RM(f"{CFG}/.hangar-active/boot1.json"), RM(f"{CFG}/.hangar-active/boot2.json")],
             procs=bare[:1], panes=[PANE("c", 400, C)]),
        tick(4, ops=[dict(op="forget", name="c")], procs=bare[:1], panes=[PANE("c", 400, C)]),
        tick(6, procs=[], panes=[PANE("c", None, C)]),
    ]

    # Marcador do hook por sid do cmdline (resume: o <sid>.jsonl nunca nasce).
    sid3 = "66666666-6666-4666-8666-666666666666"
    D = work("d")
    pd = proj(D)
    cases["claude_marker_by_session_id"] = [
        tick(0, fs=[MK(D), W(f"{pd}/c000000a-0000-4000-8000-000000000000.jsonl", "{}\n", T0 - 40),
                    W(f"{pd}/c0000008-0000-4000-8000-000000000000.jsonl", "{}\n", T0 - 1),
                    J(f"{CFG}/.hangar-active/{sid3}.json", {"jsonl": f"{pd}/c000000a-0000-4000-8000-000000000000.jsonl"})],
             procs=[P(500, 1, ["claude", "--session-id", sid3], D)], panes=[PANE("d", 500, D)]),
        tick(2, fs=[J(f"{CFG}/.hangar-active/{sid3}.json", {"jsonl": f"{pd}/c000000c-0000-4000-8000-000000000000.jsonl"})],
             procs=[P(500, 1, ["claude", "--session-id", sid3], D)], panes=[PANE("d", 500, D)]),
    ]

    # Semente da criação vale antes do processo; rename leva cache junto; esquecer volta ao chute.
    E = work("e")
    pe = proj(E)
    cases["seed_rename_forget"] = [
        tick(0, fs=[MK(E), W(f"{pe}/c0000002-0000-4000-8000-000000000000.jsonl", "{}\n", T0 - 100)],
             ops=[dict(op="seed", name="nova", jsonl=f"{pe}/c000000e-0000-4000-8000-000000000000.jsonl")],
             procs=[P(600, 1, SHELL, E)], panes=[PANE("nova", 600, E)]),
        tick(2, ops=[dict(op="rename", old="nova", new="renomeada")],
             procs=[P(600, 1, SHELL, E)], panes=[PANE("renomeada", 600, E)]),
        tick(4, ops=[dict(op="forget", name="renomeada")],
             procs=[P(600, 1, SHELL, E)], panes=[PANE("renomeada", 600, E)]),
    ]

    # Criada há menos de 1 s: o mapa de filhos em cache (de antes da criação) não tem o agente, e o
    # pane conta só pelo CP_PROVIDER; `fresh` (o que a criação faz, api.py `_guardar_snap`) o enxerga.
    F = work("f")
    pf = proj(F)
    sid4 = "77777777-7777-4777-8777-777777777777"
    born = [P(700, 1, SHELL, F, start=T0 + 1), P(701, 700, ["claude", "--session-id", sid4], F, start=T0 + 1)]
    novo = [PANE("f", 700, F, provider="claude", created=int(T0 + 1))]
    cases["created_under_one_second"] = [
        tick(0, fs=[MK(F), W(f"{pf}/c000000d-0000-4000-8000-000000000000.jsonl", "{}\n", T0 - 100)], procs=[], panes=[]),
        tick(1.5, procs=born, panes=novo),
        tick(1.75, ops=[dict(op="fresh")], procs=born, panes=novo),
    ]

    # Multiplexador fora do ar: a lista levanta, nunca sai vazia.
    cases["mux_unavailable"] = [
        tick(0, fs=[MK(F)], procs=born, panes=novo),
        tick(2, procs=born, panes=novo, tmux_down=True),
    ]

    # Pi e omp por bilhete (fresco, velho, sem ts, subagente) e pelo CP_PI_SESSION; Kimi por bilhete
    # com índice; chave do bilhete pelo PSMUX_SESSION.
    G = work("g")
    pisid = "88888888-8888-4888-8888-888888888888"
    pi_dir = f"{HOME}/.pi/agent/sessions/-{san(G)}--"
    pi_file = f"{pi_dir}/2027-01-15T08-00-00-000Z_{pisid}.jsonl"
    sub = f"{pi_dir}/2027-01-15T08-00-00-000Z_{pisid}/44bad0fb/run-2/session.jsonl"
    pi_env = {"CP_PI_SESSION": pisid}
    pi = [P(800, 1, ["pi"], G, env=pi_env, start=T0 - 60)]
    tk = f"{CFG}/.hangar-pi/8.json"
    cases["pi_ticket"] = [
        tick(0, fs=[MK(G), W(pi_file, "{}\n", T0 - 50), W(sub, "{}\n", T0 - 40),
                    J(tk, {"file": f"{G}/declarado.jsonl", "ts": T0 - 59})],
             procs=pi, panes=[PANE("pi", 800, G, "%8")]),
        tick(2, fs=[J(tk, {"file": f"{G}/declarado.jsonl", "ts": T0 - 600})],
             procs=pi, panes=[PANE("pi", 800, G, "%8")]),
        tick(4, fs=[J(tk, {"file": f"{G}/declarado.jsonl"})], procs=pi, panes=[PANE("pi", 800, G, "%8")]),
        tick(6, fs=[J(tk, {"file": sub, "ts": T0})], procs=[{**pi[0], "env": {"CP_PI_SESSION": "sem-arquivo"}}], panes=[PANE("pi", 800, G, "%8")]),
        tick(8, fs=[RM(tk)], procs=[{**pi[0], "env": {}}], panes=[PANE("pi", 800, G, "%8")]),
        tick(10, procs=[P(810, 1, ["omp"], G, env={"PSMUX_SESSION": "omp s.1"}, start=T0 - 60)],
             fs=[J(f"{CFG}/.hangar-pi/omp-s.1.json", {"file": f"{G}/omp.jsonl", "ts": T0})],
             panes=[PANE("omp", 810, G, "%1")]),
    ]
    K = work("k")
    ksid = "kimi-sess-1"
    kdir = f"{HOME}/.kimi-code/sessions/wd/{ksid}"
    kimi = [P(900, 1, ["kimi-code"], K, start=T0 - 60)]
    cases["kimi_ticket"] = [
        tick(0, fs=[MK(K), W(f"{kdir}/agents/main/wire.jsonl", "{}\n", T0 - 30),
                    W(f"{HOME}/.kimi-code/session_index.jsonl",
                      json.dumps({"sessionId": ksid, "sessionDir": kdir}, ensure_ascii=False) + "\n"),
                    J(f"{CFG}/.hangar-kimi/9.json", {"session_id": ksid, "ts": T0 - 10, "cwd": K})],
             procs=kimi, panes=[PANE("kimi", 900, K, "%9")]),
        tick(2, fs=[J(f"{CFG}/.hangar-kimi/9.json", {"session_id": ksid, "ts": T0 - 600})],
             procs=kimi, panes=[PANE("kimi", 900, K, "%9")]),
        tick(4, fs=[W(f"{CFG}/.hangar-kimi/9.json", "null")], procs=kimi, panes=[PANE("kimi", 900, K, "%9")]),
    ]

    # Codex: com sidecar (o pane de mesmo nome sai) e sem thread ainda (pane sem transcript).
    X = work("x")
    rollout = f"{HOME}/.codex/sessions/2027/01/15/rollout-2027-01-15T08-00-00-0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000.jsonl"
    cases["codex_with_and_without_thread"] = [
        tick(0, fs=[MK(X), W(rollout, "{}\n", T0 - 20), MK(f"{HOME}/.codex"),
                    J(f"{HOME}/.hangar/codex-sessions/cx.json",
                      {"name": "cx", "cwd": X, "rollout_path": rollout, "thread_id": "t1",
                       "codex_home": f"{HOME}/.codex", "key": "k-cx", "service_tier": "priority"})],
             procs=[P(1000, 1, ["hangar-codex-tui"], X), P(1010, 1, ["codex"], X)],
             panes=[PANE("cx", 1000, X, "%10"), PANE("semthread", 1010, X, "%11", provider="codex")]),
    ]

    # Claude sem terminal (sidecar, conta pelo config_dir e motor com conta própria).
    H = work("h")
    hsid = "99999999-9999-4999-8999-999999999999"
    cases["claude_headless"] = [
        tick(0, fs=[MK(H), W(f"{proj(H)}/{hsid}.jsonl", "{}\n", T0 - 5),
                    J(f"{HOME}/.hangar/claude-headless/hl.json",
                      {"name": "hl", "provider": "claude", "headless": True, "key": "k-hl", "cwd": H,
                       "session_id": hsid, "config_dir": None}),
                    J(f"{HOME}/.hangar/claude-headless/hm.json",
                      {"name": "hm", "provider": "claude", "headless": True, "key": "k-hm", "cwd": H,
                       "session_id": hsid, "config_dir": ALT, "engine": "kimi",
                       "engine_account": "conta1", "engine_credential_id": "cred-1"})],
             procs=[], panes=[]),
    ]

    # Par, encadeamento, worktree, worktree sumida, motor e conta.
    R1 = work("repo")
    WT = work("repo-wt")
    gone_tr = (json.dumps({"type": "assistant", "cwd": R1, "message": {"role": "assistant", "content": [
        {"type": "tool_use", "name": "Bash", "input": {"command": f"cd {work('repo-sumida')} && ls"}}]}}, ensure_ascii=False)
        + "\n")
    lsid = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    cases["links_worktree_engine_account"] = [
        tick(0, fs=[MK(f"{R1}/.git"), W(f"{R1}/.git/HEAD", "ref: refs/heads/main\n"),
                    MK(f"{R1}/.git/worktrees/repo-wt"),
                    W(f"{R1}/.git/worktrees/repo-wt/HEAD", "ref: refs/heads/feat\n"),
                    W(f"{R1}/.git/worktrees/repo-wt/gitdir", f"{WT}/.git\n"),
                    MK(WT), W(f"{WT}/.git", f"gitdir: {R1}/.git/worktrees/repo-wt\n"),
                    W(f"{proj(R1)}/{lsid}.jsonl", gone_tr, T0 - 5),
                    J(f"{CFG}/.hangar-pair/l1.json", {"peers": ["l2"], "gid": "g1", "task": "tarefa"}),
                    J(f"{CFG}/.hangar-pair/l2.json", {"peers": ["l1"], "gid": "g1", "task": "tarefa"}),
                    J(f"{CFG}/.hangar-chain/l1.json", {"target": "l2", "text": "segue"}),
                    J(f"{HOME}/.hangar/worktrees-removidas.json", {work("repo-velha"): R1})],
             procs=[P(1100, 1, ["claude", "--session-id", lsid], R1),
                    P(1110, 1, SHELL, WT), P(1111, 1110, ["claude"], WT,
                                             env={"CP_ENGINE": "kimi", "CP_ENGINE_ACCOUNT": "c1",
                                                  "CP_ENGINE_CREDENTIAL_ID": "cred-9"}),
                    P(1120, 1, ["claude"], WT, env={"CLAUDE_CONFIG_DIR": ALT}),
                    P(1130, 1, ["claude"], WT, env={"CP_ENGINE": "glm"}),
                    P(1140, 1, ["claude"], work("repo-velha"))],
             panes=[PANE("l1", 1100, R1, "%12"), PANE("l2", 1110, WT, "%13"),
                    PANE("conta", 1120, WT, "%14"), PANE("motor", 1130, WT, "%15"),
                    PANE("removida", 1140, work("repo-velha"), "%16")]),
    ]

    # Duas janelas: o pane do agente vence o ativo; nenhum agente cai no ativo.
    M = work("m")
    cases["agent_pane_choice"] = [
        tick(0, fs=[MK(M), MK(work("m2")), MK(work("outra"))],
             procs=[P(1200, 1, SHELL, M), P(1210, 1, SHELL, work("m2")), P(1211, 1210, ["claude"], work("m2")),
                    P(1220, 1, SHELL, M), P(1230, 1, SHELL, work("outra"))],
             panes=[PANE("j", 1200, M, "%20", active=True), PANE("j", 1210, work("m2"), "%21", active=False),
                    PANE("so-shell", 1220, M, "%22", active=False),
                    PANE("so-shell", 1230, work("outra"), "%23", active=True)]),
    ]

    # Colisão: duas no mesmo transcript; a dona pelo sid fica, a outra perde; sem dona, desempate
    # por tracked único; dois tracked, as duas perdem.
    N = work("n")
    pn = proj(N)
    own = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
    cases["collision"] = [
        tick(0, fs=[MK(N), W(f"{pn}/{own}.jsonl", "{}\n", T0 - 1)],
             procs=[P(1300, 1, ["claude", "--session-id", own], N), P(1310, 1, ["claude"], N)],
             panes=[PANE("dona", 1300, N, "%30"), PANE("bare", 1310, N, "%31")]),
        tick(2, fs=[W(f"{pn}/c0000004-0000-4000-8000-000000000000.jsonl", "{}\n", T0 + 2)],
             ops=[dict(op="forget", name="dona")],
             procs=[P(1300, 1, ["claude"], N, fds=[f"{pn}/c0000004-0000-4000-8000-000000000000.jsonl"]), P(1310, 1, ["claude"], N)],
             panes=[PANE("dona", 1300, N, "%30"), PANE("bare", 1310, N, "%31")]),
        tick(4, procs=[P(1300, 1, ["claude"], N, fds=[f"{pn}/c0000004-0000-4000-8000-000000000000.jsonl"]),
                       P(1310, 1, ["claude"], N, fds=[f"{pn}/c0000004-0000-4000-8000-000000000000.jsonl"])],
             panes=[PANE("dona", 1300, N, "%30"), PANE("bare", 1310, N, "%31")]),
    ]
    return cases


# ── casos: decoração ──────────────────────────────────────────────────────────────────────────

def claude_session(name, pid, cwd, sid, env=None, argv_extra=()):
    return (P(pid, 1, ["claude", "--session-id", sid, *argv_extra], cwd, env=env),
            PANE(name, pid, cwd, f"%{pid}"))


def decorate_cases():
    A = work("a")
    pa = proj(A)
    cases = {}
    s1, s2, s3 = ("c1c1c1c1-0000-4000-8000-000000000001", "c1c1c1c1-0000-4000-8000-000000000002",
                  "c1c1c1c1-0000-4000-8000-000000000003")
    p1, n1 = claude_session("m1", 2000, A, s1)
    p2, n2 = claude_session("m2", 2010, A, s2)
    p3, n3 = claude_session("m3", 2020, A, s3)
    # Marcador working/idle; registro nativo com pid vivo vence o marcador; pid morto, não.
    cases["markers_and_native_registry"] = [
        tick(0, fs=[MK(A), W(f"{pa}/{s1}.jsonl", transcript(A, ("pergunta", "resposta um")), T0 - 30),
                    W(f"{pa}/{s2}.jsonl", transcript(A, ("pergunta", "resposta dois")), T0 - 30),
                    W(f"{pa}/{s3}.jsonl", transcript(A, ("pergunta", "resposta três")), T0 - 30),
                    J(f"{CFG}/.hangar-state/{s1}.json", {"state": "working", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-state/{s2}.json", {"state": "idle", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-state/{s3}.json", {"state": "working", "ts": T0 - 20}),
                    J(f"{CFG}/sessions/2010.json", {"sessionId": s2, "pid": 2010, "status": "busy",
                                                    "updatedAt": (T0 - 5) * 1000}),
                    J(f"{CFG}/sessions/4242.json", {"sessionId": s3, "pid": 4242, "status": "idle",
                                                    "updatedAt": (T0 - 5) * 1000})],
             procs=[p1, p2, p3], panes=[n1, n2, n3]),
        # Estado `shell` do registro conta como parado.
        tick(2, fs=[J(f"{CFG}/sessions/2010.json", {"sessionId": s2, "pid": 2010, "status": "shell",
                                                     "updatedAt": (T0 + 1) * 1000})],
             procs=[p1, p2, p3], panes=[n1, n2, n3]),
    ]

    # Pergunta aberta fora da tela (.hangar-askq) e já respondida no transcript.
    q = {"tool_input": {"questions": [{"question": "Qual?", "header": "h", "multiSelect": False,
                                        "options": [{"label": "Um", "description": "d"},
                                                    {"label": "Dois", "description": "d"}]}]}}
    cases["open_question_sidecar"] = [
        tick(0, fs=[MK(A), W(f"{pa}/{s1}.jsonl", transcript(A, ("pergunta", "resposta um")), T0 - 30),
                    J(f"{CFG}/.hangar-state/{s1}.json", {"state": "working", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-askq/{s1}.json", {**q, "transcript_path": f"{pa}/{s1}.jsonl"}, T0 - 10)],
             procs=[p1], panes=[n1]),
        # Respondida pela TUI: o tool_result depois do sidecar fecha a pergunta.
        tick(4, fs=[W(f"{pa}/{s1}.jsonl", transcript(A, ("pergunta", "resposta um")) + "\n".join([
            json.dumps({"type": "assistant", "timestamp": "2027-01-15T08:00:01.000Z", "message": {
                "role": "assistant", "content": [{"type": "tool_use", "id": "q1", "name": "AskUserQuestion",
                                                  "input": q["tool_input"]}]}}, ensure_ascii=False),
            json.dumps({"type": "user", "timestamp": "2027-01-15T08:00:03.000Z", "message": {
                "role": "user", "content": [{"type": "tool_result", "tool_use_id": "q1", "content": "Um"}]}},
                ensure_ascii=False)]) + "\n", T0 + 3)],
             procs=[p1], panes=[n1]),
    ]

    # Statusline do sidecar (vence o pane), velha de mais de um dia (cai no pane), modelo da escolha.
    cases["statusline_sidecar_age"] = [
        tick(0, fs=[MK(A), W(f"{pa}/{s1}.jsonl", transcript(A, ("p", "r")), T0 - 30),
                    W(f"{pa}/{s2}.jsonl", transcript(A, ("p", "r")), T0 - 30),
                    J(f"{CFG}/.hangar-state/{s1}.json", {"state": "idle", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-state/{s2}.json", {"state": "idle", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-status/{s1}.json", {"line": "🤖 Sonnet 4.5 (high) │ 💬 3k/4k 40k/200k",
                                                         "model": "claude-sonnet-4-5", "effort": "high",
                                                         "ts": T0 - 5}),
                    J(f"{CFG}/.hangar-status/{s2}.json", {"line": "🤖 Velha │ 💬 1k/1k 1k/200k",
                                                         "ts": T0 - 90_000})],
             procs=[p1, p2], panes=[n1, n2], captures={"m2": [IDLE]}),
    ]

    # Contexto 200k (padrão), 1M pelo modelo [1m] e pela janela declarada; modelo da resposta.
    pc200, nc200 = claude_session("c200", 2100, A, s1)
    pc1m, nc1m = claude_session("c1m", 2110, A, s2, argv_extra=("--model", "claude-opus-4-5[1m]"))
    pcw, ncw = claude_session("cwin", 2120, A, s3, env={"CLAUDE_CODE_MAX_CONTEXT_TOKENS": "1000000"})
    cases["context_200k_and_1m"] = [
        tick(0, fs=[MK(A),
                    W(f"{pa}/{s1}.jsonl", transcript(A, ("p", "r"), used=150_000), T0 - 30),
                    W(f"{pa}/{s2}.jsonl", transcript(A, ("p", "r"), used=150_000), T0 - 30),
                    W(f"{pa}/{s3}.jsonl", transcript(A, ("p", "r"), used=250_000,
                                                     model="claude-sonnet-4-5-20250929"), T0 - 30),
                    J(f"{CFG}/.hangar-state/{s1}.json", {"state": "idle", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-state/{s2}.json", {"state": "idle", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-state/{s3}.json", {"state": "idle", "ts": T0 - 20})],
             procs=[pc200, pc1m, pcw], panes=[nc200, nc1m, ncw]),
        # Dentro dos 20 s o contexto não é relido; depois relê o transcript novo.
        tick(5, fs=[W(f"{pa}/{s1}.jsonl", transcript(A, ("p", "r"), used=190_000), T0 + 4)],
             procs=[pc200, pc1m, pcw], panes=[nc200, nc1m, ncw]),
        tick(26, procs=[pc200, pc1m, pcw], panes=[nc200, nc1m, ncw]),
    ]

    # Plano sem pino (o mais recente) e com pino; esconder com !none.
    plan = ("# Plano\n\n### Task 1: Um\n\n- [x] **Step 1: a**\n- [ ] **Step 2: b**\n\n"
            "### Task 2: Dois\n\n- [ ] **Step 3: c**\n")
    other = "# Outro\n\n### Task 1: Só\n\n- [x] **Step 1: a**\n- [ ] **Step 2: b**\n"
    B = work("b")
    pb = proj(B)
    pp, np_ = claude_session("plano", 2200, B, s1)
    cases["plan_with_and_without_pin"] = [
        tick(0, fs=[MK(f"{B}/.git"), W(f"{B}/.git/HEAD", "ref: refs/heads/main\n"),
                    W(f"{B}/docs/superpowers/plans/2027-01-01-a.md", plan, T0 - 50),
                    W(f"{B}/docs/superpowers/plans/2027-01-02-b.md", other, T0 - 100),
                    W(f"{pb}/{s1}.jsonl", transcript(B, ("p", "r")), T0 - 30),
                    J(f"{CFG}/.hangar-state/{s1}.json", {"state": "idle", "ts": T0 - 20})],
             procs=[pp], panes=[np_]),
        tick(4, fs=[W(f"{B}/.git/cp-plan-pin", "2027-01-02-b\n")], procs=[pp], panes=[np_]),
        tick(8, fs=[W(f"{B}/.git/cp-plan-pin", "!none\n")], procs=[pp], panes=[np_]),
    ]

    # Loop, travada (working velho), última resposta só na parada, e sem terminal pelo runtime.
    H = work("h")
    ph = proj(H)
    pl, nl = claude_session("loop", 2300, H, s1)
    ps, ns = claude_session("travada", 2310, H, s2)
    cases["loop_stalled_last_reply_headless"] = [
        tick(0, fs=[MK(H), W(f"{ph}/{s1}.jsonl", transcript(H, ("p", "resposta final do loop")), T0 - 30),
                    W(f"{ph}/{s2}.jsonl", transcript(H, ("p", "r")), T0 - 900),
                    W(f"{ph}/{s3}.jsonl", transcript(H, ("p", "resposta sem terminal")), T0 - 30),
                    J(f"{CFG}/.hangar-state/{s1}.json", {"state": "idle", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-state/{s2}.json", {"state": "working", "ts": T0 - 900}),
                    J(f"{CFG}/.hangar-state/{s3}.json", {"state": "idle", "ts": T0 - 20}),
                    J(f"{CFG}/.hangar-loop/loop.json", {"status": "running", "iter": 2, "max_iters": 5}),
                    W(CODEX_ROLLOUT, codex_rollout(H), T0 - 30),
                    J(f"{HOME}/.hangar/codex-sessions/cx.json",
                      {"name": "cx", "cwd": H, "rollout_path": CODEX_ROLLOUT, "thread_id": "t1",
                       "codex_home": f"{HOME}/.codex", "key": "k-cx"}),
                    J(f"{HOME}/.hangar/claude-headless/sem.json",
                      {"name": "sem", "provider": "claude", "headless": True, "key": "k-sem", "cwd": H,
                       "session_id": s3, "config_dir": None})],
             procs=[pl, ps], panes=[nl, ns], captures={"travada": [IDLE]}),
        # Runtime vivo responde pelo sem terminal; problema do runtime na com terminal.
        tick(2, facts={"headless_snapshot": {"sem": {"state": "awaiting_input", "label": None,
                                                    "question": "Permitir?", "options": ["Sim", "Não"],
                                                    "status_line": "🤖 Haiku 4.5"}},
                       "headless_problem": {"sem": ["claude_login"]},
                       "runtime_problem": {"loop": ["runtime_x"]}},
             procs=[pl, ps], panes=[nl, ns]),
    ]
    return cases


# ── casos: estado ─────────────────────────────────────────────────────────────────────────────

def state_cases():
    A = work("a")
    pa = proj(A)
    cases = {}
    s = [f"d0d0d0d0-0000-4000-8000-00000000000{i}" for i in range(1, 7)]
    sess = [claude_session(f"s{i}", 3000 + 10 * i, A, s[i - 1]) for i in range(1, 7)]
    procs = [p for p, _ in sess]
    panes = [n for _, n in sess]
    files = [MK(A)] + [W(f"{pa}/{x}.jsonl", transcript(A, ("p", "r")), T0 - 30) for x in s]

    # Sem marcador: o pane decide; spinner que não anda na segunda captura vira parado.
    cases["pane_without_marker"] = [
        tick(0, fs=files, procs=procs[:4], panes=panes[:4],
             captures={"s1": [IDLE], "s2": [SPIN % 1, SPIN % 2], "s3": [SPIN % 1, SPIN % 1], "s4": [MENU]}),
    ]

    # Marcador awaiting: pane sem menu rebaixa só depois de 10 s do marcador.
    cases["awaiting_demote_grace"] = [
        tick(0, fs=files + [J(f"{CFG}/.hangar-state/{s[0]}.json", {"state": "awaiting_input", "ts": T0 - 5}),
                            J(f"{CFG}/.hangar-state/{s[1]}.json", {"state": "awaiting_input", "ts": T0 - 60})],
             procs=procs[:2], panes=panes[:2], captures={"s1": [IDLE], "s2": [IDLE]}),
        tick(2, procs=procs[:2], panes=panes[:2], captures={"s1": [MENU], "s2": [IDLE]}),
        tick(8, procs=procs[:2], panes=panes[:2], captures={"s1": [IDLE], "s2": [IDLE]}),
    ]

    # Ocioso velho: transcript escrito depois do idle (fora a folga de 1 s) olha o pane uma vez por mtime.
    cases["idle_checked_once_per_mtime"] = [
        tick(0, fs=files + [J(f"{CFG}/.hangar-state/{s[0]}.json", {"state": "idle", "ts": T0 - 60}),
                            TOUCH(f"{pa}/{s[0]}.jsonl", T0 - 10)],
             procs=procs[:1], panes=panes[:1], captures={"s1": [IDLE]}),
        tick(2, procs=procs[:1], panes=panes[:1], captures={"s1": [IDLE]}),
        tick(4, fs=[TOUCH(f"{pa}/{s[0]}.jsonl", T0 + 3)], procs=procs[:1], panes=panes[:1],
             captures={"s1": [SPIN % 1, SPIN % 2]}),
        # Escrita dentro da folga de 1 s depois do idle não abre turno: fica no marcador.
        tick(6, fs=[J(f"{CFG}/.hangar-state/{s[0]}.json", {"state": "idle", "ts": T0 + 5}),
                    TOUCH(f"{pa}/{s[0]}.jsonl", T0 + 5.5)], procs=procs[:1], panes=panes[:1],
             captures={"s1": [IDLE]}),
    ]

    # Teto de 2 capturas de statusline por tique, das mais velhas; validade de 20 s; label do spinner.
    markers = [J(f"{CFG}/.hangar-state/{x}.json", {"state": "working" if i < 2 else "idle", "ts": T0 - 5})
               for i, x in enumerate(s)]
    cap = {f"s{i}": [SPIN % i] for i in range(1, 7)}
    cases["statusline_budget_and_ttl"] = [
        tick(0, fs=files + markers, procs=procs, panes=panes, captures=cap),
        tick(2, procs=procs, panes=panes, captures=cap),
        tick(4, procs=procs, panes=panes, captures=cap),
        tick(10, procs=procs, panes=panes, captures=cap),
        tick(23, procs=procs, panes=panes, captures={**cap, "s1": [IDLE]}),
    ]

    # Limite: a travada que a varredura de statusline não alcançou paga a própria captura; depois
    # a varredura a alcança e a mesma captura serve o radar.
    radar = [procs[1], procs[2], procs[0]]
    radar_panes = [panes[1], panes[2], panes[0]]
    cases["rate_limit_radar"] = [
        tick(0, fs=[MK(A)] + [W(f"{pa}/{x}.jsonl", transcript(A, ("p", "r")), T0 - 900) for x in s[:3]]
             + [J(f"{CFG}/.hangar-state/{s[0]}.json", {"state": "working", "ts": T0 - 900}),
                J(f"{CFG}/.hangar-state/{s[1]}.json", {"state": "idle", "ts": T0 - 5}),
                J(f"{CFG}/.hangar-state/{s[2]}.json", {"state": "idle", "ts": T0 - 5})],
             procs=radar, panes=radar_panes, captures={"s1": [LIMIT]}),
        tick(10, procs=radar, panes=radar_panes, captures={"s1": [LIMIT]}),
        tick(25, procs=radar, panes=radar_panes, captures={"s1": [IDLE]}),
    ]

    # Codex sem thread: etapas do lançador e menu pelo pane.
    X = work("x")
    cases["codex_without_thread_pane"] = [
        tick(0, fs=[MK(X)], procs=[P(3100, 1, ["codex"], X), P(3110, 1, ["codex"], X)],
             panes=[PANE("cx1", 3100, X, "%40", provider="codex"), PANE("cx2", 3110, X, "%41", provider="codex")],
             captures={"cx1": ["hangar-codex-tui: preparando\nhangar-codex-tui: abrindo thread\n"],
                       "cx2": ["Aprovar?\n› 1. Sim\n  2. Não\nPress enter to confirm"]}),
    ]
    return cases


# ── execução no filho ─────────────────────────────────────────────────────────────────────────

# Os que os casos provocam de propósito (so-shell, bilhete do Pi recusado, rollout sem turno).
EXPECTED_WARNINGS = {
    ("hangar.registry", "list: %r tem %d panes e nenhum parece do agente; caindo no pane ATIVO"),
    ("hangar.registry", "pi: bilhete de %s recusado (%s); usando CP_PI_SESSION"),
    ("hangar.state", "codex: nenhuma fronteira de turno no fim de %s"),
}


class World:
    def __init__(self, root: Path):
        self.root = root
        self.procs: dict[int, dict] = {}
        self.panes: list[dict] = []
        self.captures: dict[str, list[str]] = {}
        self.facts: dict = {}
        self.reads: set[str] = set()
        self.calls: list[str] = []
        self.effects: list[list] = []
        self.tmux_down = False
        self.wall = T0

    def real(self, value):
        if isinstance(value, str):
            from app.registry import sanitize_cwd
            return value.replace(SAN, sanitize_cwd(str(self.root))).replace(ROOT, str(self.root))
        if isinstance(value, list):
            return [self.real(v) for v in value]
        if isinstance(value, dict):
            return {k: self.real(v) for k, v in value.items()}
        return value

    def ph(self, value):
        if isinstance(value, str):
            from app.registry import sanitize_cwd
            return value.replace(str(self.root), ROOT).replace(sanitize_cwd(str(self.root)), SAN)
        if isinstance(value, (list, tuple)):
            return [self.ph(v) for v in value]
        if isinstance(value, dict):
            return {self.ph(k): self.ph(v) for k, v in value.items()}
        return value

    # leitores de processo
    def proc(self, pid, what):
        self.reads.add(f"{what} {pid}")
        return self.procs.get(pid)

    def children_map(self):
        self.reads.add("scan")
        out: dict[int, list[int]] = {}
        for pid in sorted(self.procs):
            out.setdefault(self.procs[pid]["ppid"], []).append(pid)
        return out

    def cmdline(self, pid):
        p = self.proc(pid, "cmdline")
        return " ".join(p["argv"]) + " " if p and p["argv"] else ""

    def argv(self, pid):
        p = self.proc(pid, "cmdline")
        return list(p["argv"]) if p else []

    def env(self, pid, name):
        p = self.proc(pid, "environ")
        return (p["env"].get(name) or None) if p else None

    def config_dir(self, pid):
        v = self.env(pid, "CLAUDE_CONFIG_DIR")
        return Path(v) if v else None

    def open_jsonl(self, pid, projects_dir):
        p = self.proc(pid, "fd")
        base = str(projects_dir)
        for target in (p or {}).get("fds", []):
            if target.endswith(".jsonl") and target.startswith(base + os.sep):
                return target
        return None

    def start(self, pid):
        p = self.proc(pid, "stat")
        return p["start"] if p else None

    # tmux
    def list_panes_all(self):
        self.calls.append("list-panes")
        if self.tmux_down:
            from app.tmux import MuxIndisponivel
            raise MuxIndisponivel("multiplexador fora do ar (fake)")
        out: dict[str, list[dict]] = {}
        for pane in self.panes:
            out.setdefault(pane["name"], []).append(dict(pane))
        return out

    def capture_pane(self, name, *args, **kwargs):
        self.calls.append(f"capture {name}" + (f" {args} {kwargs}" if args or kwargs else ""))
        frames = self.captures.get(name) or [""]
        return frames.pop(0) if len(frames) > 1 else frames[0]

    def pane_pid(self, name):
        return next((p["pid"] for p in self.panes if p["name"] == name and p["active"]), None)


def _clock_patches(world):
    async def sleep(delay, result=None):
        world.wall += delay
        return result
    return [mock.patch.object(time, "time", lambda: world.wall),
            mock.patch.object(time, "monotonic", lambda: world.wall - MONO_OFF),
            mock.patch.object(asyncio, "sleep", sleep)]


def _apply_fs(world, ops):
    for op in ops:
        path = Path(world.real(op["path"]))
        kind = op["op"]
        if kind == "mkdir":
            path.mkdir(parents=True, exist_ok=True)
        elif kind == "rm":
            # Caminho errado num rm deixaria o caso diferente do pretendido, calado.
            assert path.exists(), op["path"]
            if path.is_dir():
                shutil.rmtree(path)
            else:
                path.unlink(missing_ok=True)
        elif kind == "write":
            path.parent.mkdir(parents=True, exist_ok=True)
            text = world.real(op["text"])
            # Marcador escapado (⟦) passaria calado como caminho literal.
            assert "⟦" not in text and "\\u27e6" not in text, op["path"]
            path.write_text(text, encoding="utf-8")
            mtime = op["mtime"] if op["mtime"] is not None else world.wall
            os.utime(path, (mtime, mtime))
        elif kind == "touch":
            os.utime(path, (op["mtime"], op["mtime"]))
        else:
            raise ValueError(kind)


def _run_case(world, ticks, mode):
    from app import hook_state as hs_mod, procinfo, registry, statusline, askquestion
    from app.config import settings
    from app.registry import SessionRegistry
    from app import sse
    from app.tmux import MuxIndisponivel

    home = world.root / "home"
    home.mkdir(parents=True, exist_ok=True)
    os.environ["HOME"] = str(home)
    settings.projects_dir = home / ".claude" / "projects"
    dirs = [home / ".claude", home / ".claude-alt"]
    for cls_cache in ("_jsonl_cache", "_pair_ausencias", "_last_res", "_status_cache", "_context_cache",
                      "_agent_pid", "_label_cache", "_limit_cache", "_reply_cache"):
        getattr(SessionRegistry, cls_cache).clear()
    SessionRegistry._fd_locked.clear()
    SessionRegistry._SEM_AGENTE_AVISADAS.clear()
    registry._marker_cache.clear()
    registry._git_ultimo.clear()
    registry._git_em_voo.clear()
    procinfo._mapa_cache = None
    reg = SessionRegistry()
    hook = hs_mod.hook_state
    hook._dirs = []
    hs_mod._REGISTRO_DESCONHECIDOS.clear()
    out = []
    for t in ticks:
        assert t["at"] >= world.wall, "relógio voltou"
        world.wall = t["at"]
        world.tmux_down = t.get("tmux_down", False)
        _apply_fs(world, t.get("fs", []))
        world.procs = {p["pid"]: world.real(p) for p in t["procs"]}
        world.panes = world.real(t["panes"])
        world.captures = {k: list(v) for k, v in world.real(t.get("captures", {})).items()}
        world.facts = t.get("facts", {})
        world.reads, world.calls, world.effects = set(), [], []
        for op in t.get("ops", []):
            if op["op"] == "seed":
                reg._jsonl_cache[op["name"]] = world.real(op["jsonl"])
            elif op["op"] == "forget":
                reg._forget(op["name"])
            elif op["op"] == "rename":
                # Só a parte do rename() que mexe nos caches da resolução (registry.py, rename).
                old, new = op["old"], op["new"]
                j = reg._jsonl_cache.pop(old, None)
                if j is not None:
                    reg._jsonl_cache[new] = j
                if old in reg._fd_locked:
                    reg._fd_locked.discard(old)
                    reg._fd_locked.add(new)
                st = reg._status_cache.pop(old, None)
                if st is not None:
                    reg._status_cache[new] = st
                for cache in (reg._label_cache, reg._limit_cache, reg._reply_cache):
                    if old in cache:
                        cache[new] = cache.pop(old)
            elif op["op"] == "fresh":
                procinfo._invalidar_children_map()
            else:
                raise ValueError(op)
        hook._map.clear()
        hook._registro.clear()
        hook._registro_arquivo.clear()
        hook.load_existing(dirs)
        try:
            rows = reg.list() if mode == "discovery" else asyncio.run(reg.list_with_state())
        except MuxIndisponivel:
            out.append({**t, "expected": {"raises": "MuxIndisponivel", "tmux": sorted(world.calls)}})
            continue
        expected = {
            "rows": [r.model_dump(mode="json", exclude_defaults=True) for r in rows],
            "jsonl_cache": dict(sorted(reg._jsonl_cache.items())),
            "fd_locked": sorted(reg._fd_locked),
            "tmux": sorted(world.calls),
            "proc_reads": sorted(world.reads),
        }
        if mode != "discovery":
            mono = time.monotonic()
            expected.update(
                sig=sse._list_sig(rows),
                effects=sorted(world.effects),
                idle_checked=dict(sorted(reg._idle_conferido.items())),
                status_cache={k: [round(mono - v[0], 3), v[1]] for k, v in sorted(reg._status_cache.items())},
                label_cache=dict(sorted(reg._label_cache.items())),
                limit_cache={k: [round(mono - v[0], 3), v[1]] for k, v in sorted(reg._limit_cache.items())},
            )
        out.append({**t, "expected": world.ph(expected)})
    return out


def _child(out_dir: Path):
    # Aviso fora da lista conhecida é fake com assinatura errada sendo engolido pelo registry: o
    # golden gravaria o caminho de erro como se fosse o caso.
    warnings = _Warnings()
    logging.basicConfig(level=logging.WARNING, handlers=[warnings], force=True)
    sys.path.insert(0, str(BACKEND))
    from app import agentpane, askquestion, hook_state as hs_mod, procinfo, registry, statusline, tmux
    from app import codex_contas, runtime_adapter, worktrees
    from app.models import SessionInfo
    from app.adapters import get_adapter, CLAUDE_HEADLESS
    from app.registry import SessionRegistry

    tmp = Path(tempfile.gettempdir()) / f"hangarlist{secrets.token_hex(6)}"
    groups = {"list_discovery.json": ("discovery", discovery_cases()),
              "list_decorate.json": ("decorate", decorate_cases()),
              "list_state.json": ("state", state_cases())}
    try:
        results = {}
        index = 0
        for fname, (mode, cases) in groups.items():
            rendered = []
            for name, ticks in cases.items():
                index += 1
                world = World(tmp / f"c{index}")
                hl = get_adapter(CLAUDE_HEADLESS)
                cx = get_adapter("codex")

                def snapshot(self, name, world=world):
                    snap = world.facts.get("headless_snapshot", {}).get(name)
                    return None if snap is None else SimpleNamespace(codex_question=None, **snap)

                def demote(sid, world=world, original=hs_mod.hook_state.demote_awaiting):
                    world.effects.append(["demote_awaiting", sid])
                    original(sid)

                patches = _clock_patches(world) + [
                    mock.patch.object(procinfo, "_varrer_children_map", world.children_map),
                    mock.patch.object(procinfo, "_cmdline", world.cmdline),
                    mock.patch.object(procinfo, "_env_var_of", world.env),
                    mock.patch.object(procinfo, "pid_vivo", lambda pid: pid in world.procs),
                    mock.patch.object(hs_mod, "pid_vivo", lambda pid: pid in world.procs),
                    mock.patch.object(agentpane, "_cmdline", world.cmdline),
                    mock.patch.object(registry, "_cmdline", world.cmdline),
                    mock.patch.object(registry, "_argv", world.argv),
                    mock.patch.object(registry, "_open_jsonl", world.open_jsonl),
                    mock.patch.object(registry, "_config_dir_of", world.config_dir),
                    mock.patch.object(registry, "_proc_start_time", world.start),
                    mock.patch.object(registry, "_engine_of", lambda pid: world.env(pid, "CP_ENGINE")),
                    mock.patch.object(registry, "_pi_sid_of", lambda pid: world.env(pid, "CP_PI_SESSION")),
                    mock.patch.object(worktrees, "REMOVED_FILE",
                                      world.root / "home" / ".hangar" / "worktrees-removidas.json"),
                    mock.patch.object(codex_contas, "default_home", lambda w=world: w.root / "home" / ".codex"),
                    mock.patch.object(tmux, "list_panes_all", world.list_panes_all),
                    mock.patch.object(tmux, "capture_pane", world.capture_pane),
                    mock.patch.object(tmux, "pane_pid", world.pane_pid),
                    mock.patch.object(statusline, "dirs_de_config",
                                      lambda w=world: [w.root / "home" / ".claude", w.root / "home" / ".claude-alt"]),
                    mock.patch.object(askquestion, "dirs_de_config",
                                      lambda w=world: [w.root / "home" / ".claude", w.root / "home" / ".claude-alt"]),
                    mock.patch.object(SessionRegistry, "_varrer_pares_mortos", lambda self, vivos, agora=None: None),
                    mock.patch.object(registry, "_atualizar_git", _no_git),
                    mock.patch.object(hs_mod.hook_state, "demote_awaiting", demote),
                    mock.patch.object(runtime_adapter, "runtime_problem",
                                      lambda n, w=world: w.facts.get("runtime_problem", {}).get(n)),
                    mock.patch.object(type(hl), "snapshot", snapshot),
                    mock.patch.object(type(hl), "problema_de",
                                      lambda self, n, w=world: w.facts.get("headless_problem", {}).get(n)),
                    mock.patch.object(type(cx), "snapshot", lambda self, n, t: None),
                    mock.patch.object(type(cx), "async_question_status", lambda self, n: (0, None)),
                    mock.patch.object(type(cx), "aprovacao_pendente", lambda self, n: (None, None)),
                    mock.patch.object(type(cx), "problema_de", lambda self, n: None),
                ]
                for p in patches:
                    p.start()
                try:
                    rendered.append({"name": name, "ticks": _run_case(world, ticks, mode)})
                finally:
                    for p in reversed(patches):
                        p.stop()
            results[fname] = rendered
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    unexpected = sorted(warnings.seen - EXPECTED_WARNINGS)
    if unexpected:
        raise SystemExit(f"aviso inesperado na geração: {unexpected}")
    out_dir.mkdir(parents=True, exist_ok=True)
    for fname, cases in results.items():
        defaults = SessionInfo(name="").model_dump(mode="json")
        defaults.pop("name")
        doc = {"about": ABOUT[fname], "conventions": CONVENTIONS, "t0": T0, "mono_off": MONO_OFF,
               "defaults": defaults, "cases": cases}
        text = json.dumps(doc, ensure_ascii=False, indent=1) + "\n"
        # Caminho da máquina que gerou não pode vazar: o golden tem de sair igual em qualquer uma.
        assert str(tmp) not in text and tempfile.gettempdir() + "/" not in text, fname
        (out_dir / fname).write_text(text, encoding="utf-8")


class _Warnings(logging.Handler):
    def __init__(self):
        super().__init__(logging.WARNING)
        self.seen: set[tuple[str, str]] = set()

    def emit(self, record):
        self.seen.add((record.name, str(record.msg)))


async def _no_git(cwd):
    return None


def main():
    out = Path(sys.argv[2] if sys.argv[1:2] == ["--child"] else (sys.argv[1] if len(sys.argv) > 1
                                                                  else HERE / "golden"))
    if sys.argv[1:2] == ["--child"]:
        _child(out)
        return
    # Filho com ambiente limpo e cwd fora do repositório: o Settings lê `.env` do cwd e variáveis CP_*.
    env = {"PATH": os.environ.get("PATH", ""), "HOME": tempfile.gettempdir(), "LANG": "C.UTF-8",
           "TZ": "UTC", "PYTHONHASHSEED": "0"}
    subprocess.run([sys.executable, __file__, "--child", str(Path(out).resolve())], env=env,
                   cwd=tempfile.gettempdir(), check=True)
    print(f"gravado em {out}")


if __name__ == "__main__":
    main()
