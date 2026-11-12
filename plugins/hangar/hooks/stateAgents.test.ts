import { expect, test } from "claude-code/testing";
import { parseNotifications } from "./verificarParse";
import { clearBridge, setBridge } from "./bridge";
import { agentEnd, contentText, registerState } from "./state";
import { deliverAgentMessage } from "./input";

test("a task-notification da linha vira o fim do subagente", () => {
  const content = [{ type: "text", text: "<task-notification>\n<task-id>af289ebc016b47e48</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n</task-notification>" }];
  const done = parseNotifications(contentText(content));
  expect(done).toEqual([{ id: "af289ebc016b47e48", status: "completed" }]);
  expect(done.map((d) => agentEnd(d.status))).toEqual(["finished"]);
});

test("só completed é fim normal", () => {
  expect(["failed", "killed"].map(agentEnd)).toEqual(["failed", "failed"]);
  expect(contentText("texto")).toBe("texto");
  expect(contentText(undefined)).toBe("");
});

type Handler = (...args: any[]) => any;

/** Os hooks do state.ts com um engine de mentira; `status` responde cada POST /agent. */
function harness(status: () => number | "hang" = () => 200, sleep: () => Promise<void> = () => new Promise(() => {})) {
  const handlers = new Map<string, Handler>();
  registerState(((event: string, ...args: any[]) => handlers.set(event, args.at(-1))) as never);
  const agents: string[] = [];
  const logs: string[] = [];
  const $ = {
    http: { fetch: async (url: string, init: { body: string }) => {
      if (!url.endsWith("/agent")) return { status: 200, text: "" };
      const b = JSON.parse(init.body) as { agentId: string; estado: string };
      agents.push(`${b.agentId}:${b.estado}`);
      const s = status();
      if (s === "hang") return new Promise(() => {});
      return { status: s, text: "" };
    } },
    ui: { log: (m: string) => { logs.push(m); } },
    clock: { sleep },
    session: { id: async () => "sid", usage: () => new Promise(() => {}) },
  };
  const fire = (event: string, e: unknown, answer: unknown = {}) => handlers.get(event)!($, e, async () => answer);
  return {
    agents, logs,
    spawn: (agentId: string, background = false) => fire("agent.spawn", { background }, { model: "m", agentId }),
    done: (agentId: string, isAborted = false) => fire("turn.complete", { agentId, answer: "", isAborted }),
    sendMessage: (to: string, answer: unknown = { result: "ok" }) => fire("tool.call", { tool: "SendMessage", to }, answer),
    notify: (id: string) => fire("session.append", { origin: { kind: "task-notification" },
      message: { content: `<task-notification><task-id>${id}</task-id><status>completed</status></task-notification>` } }),
    fireSpawnWith: (inner: () => Promise<unknown>) => handlers.get("agent.spawn")!($, { background: false }, inner),
  };
}

async function withBridge(run: () => Promise<void>) {
  setBridge({ url: "http://127.0.0.1:1/api/plugin", token: "t", sessao: "s" });
  try { await run(); } finally { clearBridge(); }
}

test("SendMessage só retoma subagente conhecido, depois de entregue", () => withBridge(async () => {
  const h = harness();
  await h.spawn("fg1");
  await h.done("fg1");
  await h.sendMessage("fg1", { deny: "não" });
  await h.sendMessage("fg1", { result: "x", isError: true });
  await h.sendMessage("nome-do-colega");
  await h.sendMessage("*");
  await h.sendMessage("fg1");
  // Primeiro plano retomado acaba no turn.complete, sem virar segundo plano.
  await h.done("fg1");
  expect(h.agents).toEqual(["fg1:running", "fg1:finished", "fg1:running", "fg1:finished"]);
}));

test("o running do spawn não cobre um fim que já saiu", () => withBridge(async () => {
  const h = harness();
  await h.fireSpawnWith(async () => {
    await h.done("rapido");
    return { model: "m", agentId: "rapido" };
  });
  expect(h.agents).toEqual(["rapido:finished"]);
}));

test("segundo plano interrompido: o turn.complete abortado encerra, porque nenhuma notificação vem", () => withBridge(async () => {
  const h = harness();
  await h.spawn("bg2", true);
  await h.done("bg2", true);
  expect(h.agents).toEqual(["bg2:running", "bg2:failed"]);
}));

test("segundo plano: a notificação encerra, e depois da retomada o turn.complete encerra", () => withBridge(async () => {
  const h = harness();
  await h.spawn("bg1", true);
  await h.done("bg1");
  await h.notify("bg1");
  await h.sendMessage("bg1");
  await h.done("bg1");
  expect(h.agents).toEqual(["bg1:running", "bg1:finished", "bg1:running", "bg1:finished"]);
}));

test("aviso recusado aparece no log", () => withBridge(async () => {
  const h = harness(() => 500);
  await h.spawn("e1");
  expect(h.logs.some((l) => l.includes("e1") && l.includes("500"))).toBe(true);
}));

test("backend pendurado não segura o hook nem os avisos seguintes", () => withBridge(async () => {
  let s: number | "hang" = "hang";
  const h = harness(() => s, async () => {});
  await h.spawn("h1");
  s = 200;
  await h.done("h1");
  await new Promise((r) => setTimeout(r, 0));
  expect(h.agents).toEqual(["h1:running", "h1:finished"]);
}));

const PONTE = { url: "http://127.0.0.1:1/api/plugin", token: "t", sessao: "s" };

/** Engine de mentira do `/agent-sent`: `acks` responde cada tentativa (status ou erro de rede). */
function delivery(acks: (number | "erro")[]) {
  const sends: unknown[] = [];
  const posts: unknown[] = [];
  const logs: string[] = [];
  const $ = {
    session: { send: async (a: unknown) => { sends.push(a); return { isDelivered: true }; } },
    http: { fetch: async (_url: string, init: { body: string }) => {
      posts.push(JSON.parse(init.body));
      const a = acks.shift() ?? 200;
      if (a === "erro") throw new Error("rede");
      return { status: a, text: "" };
    } },
    ui: { log: (m: string) => { logs.push(m); } },
  } as never;
  return { $, sends, posts, logs };
}

test("confirmação que falha uma vez é repetida, sem segunda entrega", async () => {
  const d = delivery(["erro", 200]);
  await deliverAgentMessage(d.$, PONTE, { id: "m-1", to: "a1", text: "oi" });
  expect(d.sends).toEqual([{ to: { agentId: "a1" }, text: "oi" }]);
  expect(d.posts).toEqual([1, 2].map(() => ({ sessao: "s", token: "t", id: "m-1", isDelivered: true })));
  expect(d.logs).toEqual([]);
});

test("confirmação que não chega aparece no log", async () => {
  const d = delivery([500, "erro"]);
  await deliverAgentMessage(d.$, PONTE, { id: "m-2", to: "a1", text: "oi" });
  expect(d.posts).toHaveLength(2);
  expect(d.logs.some((l) => l.includes("m-2"))).toBe(true);
});

test("o mesmo id de novo só reconfirma", async () => {
  const d = delivery([]);
  await deliverAgentMessage(d.$, PONTE, { id: "m-3", to: "a1", text: "oi" });
  await deliverAgentMessage(d.$, PONTE, { id: "m-3", to: "a1", text: "oi" });
  expect(d.sends).toHaveLength(1);
  expect(d.posts).toHaveLength(2);
});
