// Só no `vite dev`: responde as rotas do receptor com dados de exemplo. `/panel/__mock?state=` troca o cenário.
import type { Plugin } from "vite";
import type { ServerResponse } from "node:http";
import { bump, summary, trail } from "./sample";

type State = "normal" | "empty" | "error" | "slow";

export const mock = (): Plugin => ({
  name: "telemetry-mock",
  configureServer(server) {
    let logged = true, state: State = "normal";
    const streams = new Set<ServerResponse>();
    setInterval(() => {
      bump();
      for (const s of streams) s.write("event: batch\ndata: {}\n\n");
    }, 20000);
    const json = (res: ServerResponse, status: number, body?: unknown) => {
      res.statusCode = status;
      res.setHeader("content-type", "application/json");
      res.end(body === undefined ? "" : JSON.stringify(body));
    };
    server.middlewares.use(async (req, res, next) => {
      const url = new URL(req.url ?? "/", "http://x");
      const p = url.pathname;
      if (p === "/panel/__mock") {
        const s = url.searchParams.get("state");
        if (s) state = s as State;
        if (url.searchParams.has("out")) logged = false;
        return json(res, 200, { state, logged });
      }
      if (!p.startsWith("/panel/data") && !p.startsWith("/panel/trail") && !p.startsWith("/panel/api/") && p !== "/panel/events") return next();
      if (p === "/panel/api/login" || p === "/panel/api/join") {
        await new Promise(r => setTimeout(r, 500));
        let body = "";
        for await (const c of req) body += c;
        const { password } = JSON.parse(body || "{}");
        if (p === "/panel/api/login" && password !== "senha-de-exemplo") return json(res, 401, { error: "login" });
        logged = true;
        return json(res, 204);
      }
      if (p === "/panel/api/logout") { logged = false; return json(res, 204); }
      if (!logged) return json(res, 401, { error: "session" });
      if (p === "/panel/api/me") return json(res, 200, { email: "dono@hangar.dev.br" });
      if (p === "/panel/api/invites") return json(res, 200, { path: "/join#" + "ab12".repeat(16), days: 7 });
      if (p === "/panel/events") {
        res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
        res.write(": ok\n\n");
        streams.add(res);
        req.on("close", () => streams.delete(res));
        return;
      }
      if (state === "slow") await new Promise(r => setTimeout(r, 2500));
      if (state === "error") return json(res, 503, { error: "store" });
      await new Promise(r => setTimeout(r, 250));
      if (p === "/panel/trail") return json(res, 200, trail(url.searchParams.get("machine") ?? "", url.searchParams.get("day") ?? ""));
      const days = Number(url.searchParams.get("days")) || 30;
      if (state === "empty") {
        const s = summary(days);
        return json(res, 200, { ...s, records: [], records_total: 0, transitions: [], transitions_total: 0, trail_days: [], trail_days_total: 0, refusals: { declined: 0, revoked: 0 },
          period: { batches: 0, machines: 0, partial_batches: 0 }, daily: [], env: [], per_owner: [], by_dims: [], by_bucket: [], speed: [], logs: [], trend: [], counters: [] });
      }
      return json(res, 200, summary(days));
    });
  },
});
