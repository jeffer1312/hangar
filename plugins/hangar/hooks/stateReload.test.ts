import { expect, test } from "claude-code/testing";
import { clearBridge, lastState, setBridge } from "./bridge";
import { registerState } from "./state";

function load() {
  const handlers = new Map<string, (...args: any[]) => any>();
  registerState(((event: string, ...args: any[]) => handlers.set(event, args.at(-1))) as never);
  return handlers;
}

test("recarregar o plugin no meio do turno não manda idle: repete o último estado da sessão", async () => {
  const kept = new Map<string, unknown>();
  const posts: { estado: string }[] = [];
  const $ = {
    session: { cwd: async () => "/repo", model: async () => "m" },
    http: { fetch: async (_url: string, init: { body: string }) => { posts.push(JSON.parse(init.body)); } },
    state: {
      get: async (ref: { key: string }) => ({ value: kept.get(ref.key), version: kept.has(ref.key) ? 1 : 0 }),
      set: async (ref: { key: string }, value: unknown) => { kept.set(ref.key, value); return { isSet: true, version: 1 }; },
    },
  };
  setBridge({ url: "http://127.0.0.1:1/api/plugin", token: "t", sessao: "s" });
  try {
    const first = load();
    await first.get("session.start")!($, {}, async () => ({}));
    await first.get("turn.start")!($, {}, async () => ({}));
    posts.length = 0;

    await load().get("session.start")!($, {}, async () => ({}));

    expect(posts.map((p) => p.estado)).toEqual(["working"]);
    expect(lastState()).toBe("working");
  } finally {
    clearBridge();
  }
});

test("depois do fim da sessão (o /clear) o session.start seguinte volta a idle", async () => {
  const kept = new Map<string, unknown>();
  const posts: { estado: string }[] = [];
  const $ = {
    session: { cwd: async () => "/repo", model: async () => "m" },
    http: { fetch: async (_url: string, init: { body: string }) => { posts.push(JSON.parse(init.body)); } },
    state: {
      get: async (ref: { key: string }) => ({ value: kept.get(ref.key), version: kept.has(ref.key) ? 1 : 0 }),
      set: async (ref: { key: string }, value: unknown) => { kept.set(ref.key, value); return { isSet: true, version: 1 }; },
    },
  };
  setBridge({ url: "http://127.0.0.1:1/api/plugin", token: "t", sessao: "s" });
  try {
    const hooks = load();
    await hooks.get("session.end")!($, { reason: "clear" }, async () => ({}));
    posts.length = 0;

    await hooks.get("session.start")!($, {}, async () => ({}));

    expect(posts.map((p) => p.estado)).toEqual(["idle"]);
  } finally {
    clearBridge();
  }
});
