import { expect, test } from "claude-code/testing";
import { setBridge, clearBridge } from "./bridge";
import { registerState } from "./state";

test("publica o uso e a janela ao terminar o primeiro turno, com a identidade da conversa", async () => {
    const handlers = new Map<string, (...args: any[]) => any>();
    registerState(((event: string, ...args: any[]) => handlers.set(event, args.at(-1))) as never);
    const posts: unknown[] = [];
    const engine = {
      session: {
        id: async () => "conversa",
        usage: async () => ({ context: { tokens: 76_604, window: 1_000_000, percent: 7.6604 } }),
      },
      http: { fetch: async (_url: string, init: { body: string }) => { posts.push(JSON.parse(init.body)); } },
      clock: { sleep: () => new Promise<void>(() => {}) },
    };
    setBridge({ url: "http://127.0.0.1:1/api/plugin", token: "teste", sessao: "nova" });
    try {
      await handlers.get("turn.complete")!(engine, { reason: "completed" }, async () => "continuou");
      expect(posts).toEqual([{
        sessao: "nova", token: "teste", estado: "idle", motivo: "completed", session_id: "conversa",
        context: { used: 76_604, window: 1_000_000 },
      }]);
    } finally {
      clearBridge();
    }
});

test("medida que não responde no prazo não segura o idle", async () => {
    const handlers = new Map<string, (...args: any[]) => any>();
    registerState(((event: string, ...args: any[]) => handlers.set(event, args.at(-1))) as never);
    const posts: unknown[] = [];
    const engine = {
      session: { id: async () => "conversa", usage: () => new Promise(() => {}) },
      http: { fetch: async (_url: string, init: { body: string }) => { posts.push(JSON.parse(init.body)); } },
      clock: { sleep: async () => {} },
    };
    setBridge({ url: "http://127.0.0.1:1/api/plugin", token: "teste", sessao: "nova" });
    try {
      await handlers.get("turn.complete")!(engine, { reason: "completed" }, async () => "continuou");
      expect(posts).toEqual([{ sessao: "nova", token: "teste", estado: "idle", motivo: "completed" }]);
    } finally {
      clearBridge();
    }
});
