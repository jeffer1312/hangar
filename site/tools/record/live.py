"""Synthetic LIVE backend for landing-page recordings: fixture.py + scripted scenes that change over time.
GET /control/setup?scene=X[&lang=en|pt]  resets sessions for scene X (unknown scene or lang -> 400).
GET /control/go  starts its timeline (409 without setup or while running).   GET /control/status  progress and first error.
"""
import pathlib, threading, time, traceback
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SRC = (HERE / "fixture.py").read_text(encoding="utf-8")
FX = {"__name__": "landing_fx", "__file__": str(HERE / "fixture.py")}
exec(compile(SRC[:SRC.rindex("\nserver = BASE[")], "fixture.py", "exec"), FX)
BASE, SESSIONS = FX["BASE"], FX["SESSIONS"]
LOCK, bump, info, state, msg, call, result = (BASE[k] for k in ("LOCK", "bump", "info", "state", "msg", "call", "result"))
LIVE = {}
EPOCH = {"n": 0}
SCRIPT = []


def live(name):
    return LIVE.setdefault(name, {"preview": "", "thinking": "", "tool": "", "label": None, "ask": None})


SL = {"claude": "🤖 Opus5.5·1M (high✦) │ 📁 {repo} [{branch}*] │ 💬 9k/1.2k {ctx}k/1M │ 💵 $0.74 │ ⏱ 6m",
      "codex": "🤖 gpt-6 (high) │ 📁 {repo} [{branch}] │ 💬 ctx {ctx}k/400k", "kimi": "🤖 K3 (high✦) │ 📁 {repo} [{branch}] │ 💬 ctx {ctx}k/1M",
      "pi": "🤖 deepseek-v4 │ 📁 {repo} [{branch}] │ 💬 ctx {ctx}k/128k"}
STATS = {"turns": 2, "steps": 11, "in_tok": 148000, "out_tok": 5200, "llm_ms": 31200, "tool_ms": 8100, "tok_s": 71, "cache_pct": 92, "ttft_ms": 1500}


def add(name, prov, st, cwd, events, when, model=None, **extra):
    data = info(name, prov, state=st, **extra)
    data["cwd"] = cwd
    data["last_activity"] = time.time() - when
    SESSIONS[name] = {"info": data, "stats": STATS if when <= 1 else None,
                      "events": list(events), "modes": [],
                      "sl": SL.get(prov, "").format(repo=cwd.rsplit("/", 1)[-1], branch=extra.get("branch", "main"), ctx=148 if when <= 1 else 96),
                      "model": model or {"claude": "Opus 5.5", "codex": "gpt-6", "pi": "deepseek-v4", "kimi": "K3"}.get(prov)}
    SESSIONS[name]["state"] = state(st, status_line=SESSIONS[name]["sl"], **({"label": extra["label"]} if "label" in extra else {}))


def st(name, value, label=None, **extra):
    s = SESSIONS[name]
    s["info"]["state"] = value
    s["info"]["label"] = label
    s["state"] = state(value, label=label, status_line=s.get("sl"), **extra)
    for k, v in extra.items():
        s["info"][k] = v
    if value != "awaiting_input":
        s["info"].pop("question", None); s["info"].pop("options", None)
    s["info"]["last_activity"] = time.time()
    live(name)["label"] = label


def ev(name, *events):
    SESSIONS[name]["events"].extend(events)


N = {"n": 0}
def lid(p):
    N["n"] += 1
    return f"live-{p}-{N['n']}"


def tool(name, tname, inp):
    tid = lid("t")
    ev(name, call(tid, tid, tname, inp))
    return tid


def done(name, tid, text):
    ev(name, result(lid("r"), tid, text))


def stream(name, text, secs):
    steps = max(1, int(secs / 0.06))
    out = []
    for i in range(1, steps + 1):
        out.append((i * secs / steps, lambda i=i: live(name).update(preview=text[: int(len(text) * i / steps)])))
    return out


def commit(name, text):
    live(name)["preview"] = ""
    ev(name, msg("assistant_msg", lid("a"), text))
    SESSIONS[name]["info"]["last_reply"] = text.split("\n")[0][:80]


# ---------- scenes ----------
W = "/home/dev/projects"
# Texto das cenas por língua; o do fixture.py (FX["SCENE_TEXT"]) entra junto. Código, comandos, saída de
# ferramenta, nomes de arquivo e de sessão ficam iguais nas duas.
SCENE_TEXT = {
    "en": {
        "pages": "Translating {n}/42 pages",
        "suite": "Running the suite 3×",
        "on_it": "Working on it.",
        "thinking": "Thinking…", "searching": "Searching…", "editing": "Editing…", "testing": "Running tests…",
        "writing": "Writing…", "reviewing_diff": "Reviewing diff…",
        "a_prompt": "Verify Stripe webhook signatures and reject replays older than 5 minutes. Add tests.",
        "a_review0": "Reviewing the checkout diff.",
        "a_answer": ("Webhooks are now **verified before any handler runs**.\n\n"
                     "- `verifySignature()` checks the `Stripe-Signature` HMAC with a constant-time compare\n"
                     "- events older than **5 min** are rejected as replays (`409`)\n"
                     "- the event id is stored, so the same event is processed once\n\n"
                     "```ts\nif (!verifySignature(raw, header, secret)) return res.status(400).end();\n"
                     "if (isReplay(event, 300)) return res.status(409).end();\n```\n\n"
                     "All **9 webhook tests pass**."),
        "a_think": "Find where webhooks are routed, then verify before parsing the body.",
        "a_tests_desc": "Run webhook tests",
        "a_flaky_reply": "Timeout test is stable after 3 runs.",
        "k_prompt": "Invoice totals are off by a cent on some carts. Find out why and fix it.",
        "k_found": "Found it: `money.ts:41` rounds **each line item** before summing, so a 3-item cart can drift by one cent.",
        "k_answer": ("Fixed. Line items keep full precision and the total is **rounded once**.\n\n"
                     "| Cart | Before | After |\n|---|---|---|\n| 3 × $3.335 | $10.02 | $10.01 |\n| 7 × $0.145 | $1.05 | $1.02 |\n\n"
                     "Added a property test over 10k random carts — **12 tests pass**."),
        "k_q": "Round per line item or once on the total?",
        "k_opts": ["Once on the total (recommended)", "Per line item, like today", "Ask finance first"],
        "k_header": "Rounding",
        "k_opt_desc": ["Line items keep full precision; only the total is rounded.", "Each line item is rounded before the sum.", "No change until finance sets the rule."],
        "k_tests_desc": "Run billing tests",
        "p_prompt": "Redesign the checkout summary card and get it reviewed before pushing.",
        "p_found": "Summary card is split into line items and totals. Sent the diff to **checkout-review** for a second look.",
        "p_peer": ("[de: checkout-review] Blocking: `money.ts:41` rounds each line item before summing, so a 3-item cart is off by one cent. "
                   "Minor: the discount row renders when the discount is 0."),
        "p_answer": "Both fixed: the total is rounded once and the discount row hides at `0`. Sent the new commit back to **checkout-review**.",
        "p_reply": "Two findings, one blocking.",
        "p_think": "Fix the rounding in money.ts first, then the discount row.",
        "p_tests_desc": "Run checkout tests",
        "p_send": 'hangar-send checkout-review "fixed in 3e1a9c0, tests green"',
        "p_send_desc": "Tell the reviewer",
        "p_rereview": "Re-reviewing 3e1a9c0…",
        "o_rev2": "Round 2 approved. Rounding now happens once on the total; tests cover 3-item carts.",
        "o_start4": "Starting T4.", "o_reading": "Reading plan…", "o_wait4": "Waiting for T4.",
        "o_review3": "Reviewing T3…", "o_edit_bp": "Editing Breakpoints.css…",
    },
    "pt": {
        "pages": "Traduzindo {n}/42 páginas",
        "suite": "Rodando a suíte 3×",
        "on_it": "Trabalhando nisso.",
        "thinking": "Pensando…", "searching": "Buscando…", "editing": "Editando…", "testing": "Rodando os testes…",
        "writing": "Escrevendo…", "reviewing_diff": "Revisando o diff…",
        "a_prompt": "Verifique a assinatura dos webhooks do Stripe e recuse reenvios com mais de 5 minutos. Inclua testes.",
        "a_review0": "Revisando o diff do checkout.",
        "a_answer": ("Os webhooks agora são **verificados antes de qualquer handler rodar**.\n\n"
                     "- `verifySignature()` confere o HMAC do `Stripe-Signature` com comparação em tempo constante\n"
                     "- eventos com mais de **5 min** são recusados como reenvio (`409`)\n"
                     "- o id do evento fica guardado, então o mesmo evento só é processado uma vez\n\n"
                     "```ts\nif (!verifySignature(raw, header, secret)) return res.status(400).end();\n"
                     "if (isReplay(event, 300)) return res.status(409).end();\n```\n\n"
                     "Os **9 testes de webhook passam**."),
        "a_think": "Achar onde os webhooks são roteados e conferir a assinatura antes de ler o corpo.",
        "a_tests_desc": "Rodar os testes de webhook",
        "a_flaky_reply": "O teste de timeout ficou estável em 3 rodadas.",
        "k_prompt": "O total da fatura sai com um centavo de diferença em alguns carrinhos. Descubra por quê e corrija.",
        "k_found": "Achei: `money.ts:41` arredonda **cada item** antes de somar, então um carrinho com 3 itens erra por um centavo.",
        "k_answer": ("Corrigido. Cada item mantém a precisão completa e o total é **arredondado uma vez só**.\n\n"
                     "| Carrinho | Antes | Depois |\n|---|---|---|\n| 3 × R$ 3,335 | R$ 10,02 | R$ 10,01 |\n| 7 × R$ 0,145 | R$ 1,05 | R$ 1,02 |\n\n"
                     "Adicionei um teste de propriedade com 10 mil carrinhos aleatórios — **12 testes passam**."),
        "k_q": "Arredondo por item ou uma vez só no total?",
        "k_opts": ["Uma vez no total (recomendado)", "Por item, como hoje", "Perguntar ao financeiro antes"],
        "k_header": "Arredondamento",
        "k_opt_desc": ["Os itens ficam com a precisão inteira e só o total é arredondado.", "Cada item é arredondado antes da soma.", "Não mexo até o financeiro definir a regra."],
        "k_tests_desc": "Rodar os testes de cobrança",
        "p_prompt": "Refaça o card de resumo do checkout e peça revisão antes do push.",
        "p_found": "O card de resumo agora separa itens e totais. Mandei o diff para o **checkout-review** dar uma segunda olhada.",
        "p_peer": ("[de: checkout-review] Bloqueante: `money.ts:41` arredonda cada item antes de somar, então um carrinho com 3 itens erra por um centavo. "
                   "Menor: a linha de desconto aparece quando o desconto é 0."),
        "p_answer": "Corrigi os dois: o total é arredondado uma vez só e a linha de desconto some quando é `0`. Mandei o commit novo de volta para o **checkout-review**.",
        "p_reply": "Dois achados, um bloqueante.",
        "p_think": "Corrigir primeiro o arredondamento em money.ts, depois a linha de desconto.",
        "p_tests_desc": "Rodar os testes do checkout",
        "p_send": 'hangar-send checkout-review "corrigido em 3e1a9c0, testes passando"',
        "p_send_desc": "Avisar o revisor",
        "p_rereview": "Revisando 3e1a9c0 de novo…",
        "o_rev2": "Rodada 2 aprovada. O arredondamento agora acontece uma vez só, no total; os testes cobrem carrinhos com 3 itens.",
        "o_start4": "Começando a T4.", "o_reading": "Lendo o plano…", "o_wait4": "Esperando a T4.",
        "o_review3": "Revisando a T3…", "o_edit_bp": "Editando Breakpoints.css…",
    },
}


def base_side(t, skip=()):
    rows = [
        ("docs-i18n", "pi", "working", f"{W}/acme-docs", t["pages"].format(n=31), 60),
        ("flaky-tests", "kimi", "working", f"{W}/acme-api", t["suite"], 90),
        ("mobile-release", "claude", "idle", f"{W}/acme-mobile", None, 3600),
    ]
    for name, prov, s, cwd, label, when in rows:
        if name in skip: continue
        extra = {"label": label} if label else {"last_reply": t["release"]}
        add(name, prov, s, cwd, [msg("assistant_msg", f"{name}-0", t["on_it"])], when, branch="main", **extra)


def pages(t, start):
    out = []
    for k in range(12):
        out.append((1.3 * k, lambda k=k: (st("docs-i18n", "working", t["pages"].format(n=min(42, start + k))))))
    return out


def scene_agents(t):
    SESSIONS.clear()
    T = "api-webhooks"
    add(T, "claude", "working", f"{W}/acme-api",
        [msg("user_msg", "u1", t["a_prompt"])], 1,
        branch="feat/webhooks", git_added=0, git_removed=0, label=t["thinking"])
    add("checkout-review", "codex", "working", f"{W}/acme-web",
        [msg("assistant_msg", "cr0", t["a_review0"])], 20, branch="feat/checkout", label=t["reviewing_diff"])
    base_side(t)
    ANSWER = t["a_answer"]
    S = [(0.3, lambda: live(T).update(thinking=t["a_think"])),
         (1.8, lambda: (live(T).update(thinking=""), st(T, "working", t["searching"]))),
         (1.9, lambda: SCR.__setitem__("g", tool(T, "Grep", {"pattern": "webhook", "path": "src"}))),
         (2.6, lambda: done(T, SCR["g"], "src/routes/webhooks.ts:12\nsrc/server.ts:48")),
         (3.0, lambda: SCR.__setitem__("r", tool(T, "Read", {"file_path": f"{W}/acme-api/src/routes/webhooks.ts"}))),
         (3.6, lambda: done(T, SCR["r"], "\n".join(f"line {i}" for i in range(64)))),
         (4.0, lambda: (st(T, "working", t["editing"]), SCR.__setitem__("e", tool(T, "Edit", {"file_path": f"{W}/acme-api/src/routes/webhooks.ts",
              "old_string": "const event = JSON.parse(raw);", "new_string": "if (!verifySignature(raw, header, secret)) return res.status(400).end();\nconst event = JSON.parse(raw);\nif (isReplay(event, 300)) return res.status(409).end();"})))),
         (4.8, lambda: (done(T, SCR["e"], "The file has been updated."), SESSIONS[T]["info"].update(git_added=38, git_removed=4, git_dirty=2))),
         (5.3, lambda: (st(T, "working", t["testing"]), SCR.__setitem__("b", tool(T, "Bash", {"command": "pnpm test webhooks", "description": t["a_tests_desc"]})))),
         (6.0, lambda: (st("flaky-tests", "idle"), SESSIONS["flaky-tests"]["info"].update(last_reply=t["a_flaky_reply"]))),
         (7.6, lambda: done(T, SCR["b"], " ✓ webhooks.test.ts (9 tests) 412ms\n Test Files  1 passed\n Tests  9 passed")),
         (8.0, lambda: st(T, "working", t["writing"])),
         (8.6, lambda: st("checkout-review", "awaiting_input", question=t["rounding_q"], options=list(t["rounding_opts"]))),
         ]
    S += [(8.1 + at, f) for at, f in stream(T, ANSWER, 3.6)]
    S += [(12.0, lambda: (commit(T, ANSWER), st(T, "idle")))]
    S += pages(t, 31)
    return T, S


def scene_ask(t):
    SESSIONS.clear()
    T = "billing-totals"
    ev0 = [msg("user_msg", "u1", t["k_prompt"]),
           call("t1", "t1", "Grep", {"pattern": "round", "path": "src/billing"}), result("t1r", "t1", "src/billing/money.ts:41"),
           call("t2", "t2", "Read", {"file_path": f"{W}/acme-api/src/billing/money.ts"}), result("t2r", "t2", "\n".join(f"line {i}" for i in range(80))),
           msg("assistant_msg", "a1", t["k_found"])]
    add(T, "claude", "working", f"{W}/acme-api", ev0, 1, branch="fix/rounding", label=t["thinking"])
    add("checkout-review", "codex", "working", f"{W}/acme-web", [msg("assistant_msg", "cr0", t["reviewing"])], 30, branch="feat/checkout", label=t["reviewing_diff"])
    base_side(t)
    ANSWER = t["k_answer"]
    Q = t["k_q"]
    OPTS = list(t["k_opts"])
    S = [(1.0, lambda: st(T, "awaiting_input", question=Q, options=OPTS)),
         (6.5, lambda: (ev(T, msg("user_msg", lid("u"), OPTS[0])), st(T, "working", t["editing"]))),
         (7.2, lambda: SCR.__setitem__("e", tool(T, "Edit", {"file_path": f"{W}/acme-api/src/billing/money.ts",
              "old_string": "sum + round(line.total)", "new_string": "sum + line.total"}))),
         (7.8, lambda: done(T, SCR["e"], "The file has been updated.")),
         (8.2, lambda: (st(T, "working", t["testing"]), SCR.__setitem__("b", tool(T, "Bash", {"command": "pnpm test billing", "description": t["k_tests_desc"]})))),
         (10.0, lambda: done(T, SCR["b"], " Tests  12 passed")),
         ]
    S += [(10.3 + at, f) for at, f in stream(T, ANSWER, 2.8)]
    S += [(13.4, lambda: (commit(T, ANSWER), st(T, "idle")))]
    S += pages(t, 33)
    return T, S


def scene_pair(t):
    SESSIONS.clear()
    T = "checkout-redesign"
    G = dict(pair_peers=["checkout-review"], pair_gid="g-checkout", pair_task=t["group"])
    ev0 = [msg("user_msg", "u1", t["p_prompt"]),
           call("t1", "t1", "Edit", {"file_path": f"{W}/acme-web/src/CheckoutSummary.tsx", "old_string": "<Total />", "new_string": "<LineItems />\n<Total />"}),
           result("t1r", "t1", "The file has been updated."),
           msg("assistant_msg", "a1", t["p_found"])]
    add(T, "claude", "idle", f"{W}/acme-web", ev0, 1, branch="feat/checkout", git_added=112, git_removed=40, git_dirty=5, **G)
    add("checkout-review", "codex", "working", f"{W}/acme-web", [msg("assistant_msg", "cr0", t["reviewing"])], 5, branch="feat/checkout",
        label=t["reviewing_diff"], pair_peers=[T], pair_gid="g-checkout", pair_task=t["group"])
    base_side(t, skip=("mobile-release",))
    PEER = t["p_peer"]
    ANSWER = t["p_answer"]
    S = [(1.2, lambda: (st("checkout-review", "idle"), SESSIONS["checkout-review"]["info"].update(last_reply=t["p_reply"]))),
         (1.6, lambda: (ev(T, msg("user_msg", lid("p"), PEER)), st(T, "working", t["thinking"]))),
         (2.2, lambda: live(T).update(thinking=t["p_think"])),
         (3.6, lambda: (live(T).update(thinking=""), st(T, "working", t["editing"]), SCR.__setitem__("e", tool(T, "Edit", {"file_path": f"{W}/acme-web/src/money.ts",
              "old_string": "sum + round(item.total)", "new_string": "sum + item.total"})))),
         (4.3, lambda: done(T, SCR["e"], "The file has been updated.")),
         (4.7, lambda: SCR.__setitem__("e2", tool(T, "Edit", {"file_path": f"{W}/acme-web/src/CheckoutSummary.tsx",
              "old_string": "<DiscountRow value={d} />", "new_string": "{d > 0 && <DiscountRow value={d} />}"}))),
         (5.3, lambda: done(T, SCR["e2"], "The file has been updated.")),
         (5.7, lambda: (st(T, "working", t["testing"]), SCR.__setitem__("b", tool(T, "Bash", {"command": "pnpm test checkout", "description": t["p_tests_desc"]})))),
         (7.4, lambda: done(T, SCR["b"], " Tests  18 passed")),
         (7.8, lambda: SCR.__setitem__("s", tool(T, "Bash", {"command": t["p_send"], "description": t["p_send_desc"]}))),
         (8.4, lambda: done(T, SCR["s"], "delivered")),
         (8.8, lambda: st("checkout-review", "working", t["p_rereview"])),
         ]
    S += [(8.9 + at, f) for at, f in stream(T, ANSWER, 2.0)]
    S += [(11.2, lambda: (commit(T, ANSWER), st(T, "idle")))]
    S += pages(t, 35)
    return T, S


def scene_orq(t):
    # sessões de orquestração do fixture.py, refeitas na língua pedida (o painel o setup já refez)
    SESSIONS.clear()
    FX["orq_sessions"](t)
    base_side(t, skip=("mobile-release",))
    SESSIONS["checkout-arbiter"]["state"] = state("working"); SESSIONS["checkout-arbiter"]["info"]["state"] = "working"
    SESSIONS["checkout-arbiter"]["info"]["question"] = None
    T = "orq-checkout"
    ov, iso, P = FX["ov"], FX["iso"], FX["PANEL"]
    rows = P["tasks"]["rows"]
    now = 10700

    def notice(eid, text, **o):
        e = ov(lid(eid), now + N["n"] * 30, text, **o)
        ev(T, e)

    def task(n, s, rnd=None):
        rows[n - 1]["state"] = s
        if rnd: rows[n - 1]["round"] = rnd
        P["tasks"]["integrated"] = sum(1 for r in rows if r["state"] == "integrated")
    S = [(1.0, lambda: (notice("o", "rev", kind="dropped", task=2, sender="rev-t2",
                               body=t["o_rev2"],
                               decided_by={"source": "jev", "jev": {"mode": "on", "choice": "nothing", "p": 0.93, "probs": {"nothing": 0.93}}}),
                        task(2, "approved"))),
         (3.0, lambda: (notice("o", "T2 integrated", kind="advance", task=2, line={"code": "integrated", "merge": True}), task(2, "integrated"),
                        st("rev-t2", "idle"), st("exec-t2", "idle"))),
         (4.6, lambda: (notice("o", "T4 opened", kind="advance", task=4, line={"code": "opened", "sessions": [
             {"name": "exec-t4", "provider": "kimi", "model": "k3"}, {"name": "rev-t4", "provider": "codex", "model": "gpt-6"}]}), task(4, "executing", 1),
             add("exec-t4", "kimi", "working", f"{W}/acme-web", [msg("assistant_msg", "x4", t["o_start4"])], 0, label=t["o_reading"],
                 pair_gid="g-orq", pair_task=t["orq_group"]),
             add("rev-t4", "codex", "idle", f"{W}/acme-web", [msg("assistant_msg", "r4", t["o_wait4"])], 0,
                 pair_gid="g-orq", pair_task=t["orq_group"]))),
         (7.0, lambda: (notice("o", "T3 delivered", kind="advance", task=3, line={"code": "delivered", "round": 1, "commit": "c7e21b4"}), task(3, "in_review"),
                        st("exec-t3", "idle"), st("rev-t2", "working", t["o_review3"]))),
         (9.5, lambda: st("exec-t4", "working", t["o_edit_bp"])),
         ]
    S += pages(t, 37)
    return T, S


def scene_group(t):
    SESSIONS.clear()
    for name, prov, st_, cwd, events, extra, sextra in FX["rows"](t):
        data = info(name, prov, state=st_, **{k: v for k, v in extra.items() if k != "last_activity"})
        data["cwd"] = cwd; data["last_activity"] = extra.get("last_activity", time.time())
        SESSIONS[name] = {"info": data, "state": state(st_, **sextra), "events": [dict(e) for e in events], "stats": None, "modes": [],
                          "model": {"claude": "Opus 5.5", "codex": "gpt-6", "pi": "kimi-k3", "kimi": "K3"}[prov]}
    ev0 = SESSIONS["checkout-review"]["events"][0]
    ev0["text"] = ev0["text"].replace("[from: checkout-redesign]", "[de: checkout-redesign]")
    return "checkout-review", []


SCR = {}
def scene_phone(t):
    # mesma conversa da cena ask, com a pergunta na folha do AskUserQuestion, como o app do celular mostra
    T, S = scene_ask(t)
    Q, OPTS = t["k_q"], list(t["k_opts"])
    ASK = {"interaction_id": "phone-ask-1", "tool_use_id": "toolu_phone_ask", "questions": [
        {"header": t["k_header"], "question": Q, "multiSelect": False,
         "options": [{"label": o, "description": d} for o, d in zip(OPTS, t["k_opt_desc"])]}]}
    rest = [(at + 3.0, f) for at, f in S[1:]]
    return T, [(1.0, lambda: (st(T, "awaiting_input", question=Q), live(T).update(ask=ASK))),
               (9.0, lambda: live(T).update(ask=None))] + rest


SCENES = {"group": scene_group, "agents": scene_agents, "ask": scene_ask, "pair": scene_pair, "orq": scene_orq, "phone": scene_phone}
CUR = {"script": []}


RUN = {"running": False, "error": None, "done": 0, "total": 0}


def setup(name, lang="en"):
    t = {**FX["SCENE_TEXT"][lang], **SCENE_TEXT[lang]}
    with LOCK:
        LIVE.clear(); SCR.clear()
        FX["SHORTCUTS"] = FX["shortcuts"](t)
        # painel refeito a cada cena: nem língua nem avanço da cena anterior vazam
        FX["PANEL"].clear(); FX["PANEL"].update(FX["panel"](t))
        target, script = SCENES[name](t)
        CUR["script"] = sorted(script, key=lambda x: x[0])
        CUR["target"] = target
        EPOCH["n"] += 1
        RUN.update(running=False, error=None, done=0, total=len(CUR["script"]))
        bump()
    return target


def go():
    """Começa a timeline da cena preparada; devolve o motivo da recusa ou None."""
    with LOCK:
        if "target" not in CUR:
            return "nenhuma cena preparada; chame /control/setup antes"
        if RUN["running"]:
            return "a timeline desta cena já está rodando"
        RUN.update(running=True, error=None, done=0)
        epoch, script = EPOCH["n"], CUR["script"]

    def run():
        t0 = time.time()
        try:
            for at, fn in script:
                d = t0 + at - time.time()
                if d > 0: time.sleep(d)
                with LOCK:
                    # setup novo no meio: os passos que faltam são da cena velha
                    if EPOCH["n"] != epoch: return
                    try:
                        fn()
                    except Exception as e:
                        # o vídeo sairia com a cena parada; o record.sh lê isto no /control/status
                        traceback.print_exc()
                        RUN["error"] = RUN["error"] or f"passo em {at}s: {e!r}"
                    else:
                        RUN["done"] += 1
                    bump()
        finally:
            with LOCK:
                if EPOCH["n"] == epoch: RUN["running"] = False
    threading.Thread(target=run, daemon=True).start()
    return None


VOICE_MODEL = {"model": None, "effort": "low", "tier": None}


class L(FX["H"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/setup":
            q = parse_qs(url.query)
            scene = q.get("scene", [""])[0]
            if scene not in SCENES:
                self.send_json({"detail": f"cena desconhecida: {scene!r}; use {', '.join(SCENES)}"}, 400); return
            lang = q.get("lang", [""])[0] or "en"
            if lang not in SCENE_TEXT:
                self.send_json({"detail": f"língua desconhecida: {lang!r}; use {', '.join(SCENE_TEXT)}"}, 400); return
            self.send_json({"target": setup(scene, lang)}); return
        if url.path == "/control/go":
            refused = go()
            if refused:
                self.send_json({"detail": refused}, 409); return
            self.send_json({"ok": True}); return
        if url.path == "/control/status":
            with LOCK:
                body = {"running": RUN["running"], "error": RUN["error"], "steps_done": RUN["done"], "steps_total": RUN["total"]}
            self.send_json(body); return
        if url.path == "/api/voice/settings":
            # mesmo retrato da demonstração do app: sem Codex o botão de voz some
            self.send_json({"enabled": False, "codex": False, "jev": False, "voices": [], "call": {"active": False, "client": None},
                            "settings": {"voice": None, "codex_account": "demo", "organizer": {"direct": VOICE_MODEL, "plan": VOICE_MODEL}}}); return
        super().do_GET()

    def authorized(self):
        # o app do celular manda o token na query do SSE
        if parse_qs(urlparse(self.path).query).get("token", [""])[0] == "sintetica-fixture-sessao-0000":
            return True
        return super().authorized()

    def do_POST(self):
        path = urlparse(self.path).path
        parts = path.split("/")
        # resposta à folha da cena phone só confirma: quem segue a conversa é a linha do tempo da cena
        if path.startswith("/api/sessions/") and parts[-1] == "answer" and len(parts) > 3 and live(parts[3])["ask"] is not None:
            self.send_json({"ok": True}); return
        # o app do celular assina push ao entrar; o fixture não manda push
        if path.startswith("/api/push/"):
            self.send_json({"ok": True}); return
        super().do_POST()

    def stream_session(self, name):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        sent, version, epoch = set(), -1, EPOCH["n"]
        shown = {"thinking": None, "tool": None, "preview": None, "ask": None}
        try:
            while True:
                frames = []
                with LOCK:
                    s = SESSIONS.get(name)
                    if EPOCH["n"] != epoch:
                        epoch = EPOCH["n"]
                        frames.append(("reset", {}, None))
                        sent = {e["id"] for e in (s or {"events": []})["events"]}
                        version = -1
                    if s is not None:
                        lv = live(name)
                        if BASE["VERSION"]["n"] != version:
                            version = BASE["VERSION"]["n"]
                            frames.append(("state", {"session": name, **s["state"], "label": lv["label"]}, None))
                        for event in s["events"]:
                            if event["id"].startswith("live-") and event["id"] not in sent:
                                sent.add(event["id"])
                                frames.append(("message", event, f"fixture:{event['id']}"))
                        for slot, kind in (("thinking", "pensamento"), ("tool", "ferramenta")):
                            if shown[slot] != lv[slot]:
                                shown[slot] = lv[slot]
                                frames.append((kind, {"text": lv[slot]}, None))
                        if shown["ask"] != lv["ask"]:
                            shown["ask"] = lv["ask"]
                            frames.append(("ask_question", lv["ask"], None))
                        if shown["preview"] != lv["preview"]:
                            shown["preview"] = lv["preview"]
                            frames.append(("preview", {"text": lv["preview"], "md": True, "full": True, "vivo": True}, None))
                for kind, body, eid in frames or [("ping", {}, None)]:
                    self.frame(kind, body, eid)
                time.sleep(0.06)
        except (BrokenPipeError, ConnectionResetError):
            pass


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 47123), L)
print("Fixture URL: http://127.0.0.1:47123", flush=True)
server.serve_forever()
