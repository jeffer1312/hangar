import { expect, mock, test, tier } from "claude-code/testing";
import { returnFocus } from "./input";

// Como na sessão do Hangar: o plugin entra por `--plugin-dir`.
tier("prepend");

for (const { name, result, ok, dropped } of [
  { name: "recusa do prompt pelo engine não confirma a entrega ao Hangar", result: { drop: "the session was stopped and nobody has sent input since" }, ok: false, dropped: true },
  { name: "descarte do prompt por outro hook não confirma a entrega ao Hangar", result: { drop: "a hook answered without passing the prompt on" }, ok: false, dropped: true },
  { name: "prompt aceito pelo engine confirma a entrega ao Hangar", result: { text: "pode continuar" }, ok: true, dropped: false },
  { name: "exceção no envio não autoriza reenvio como descarte explícito", result: null, ok: false, dropped: false },
]) test(name, {}, async ($, on) => {
  const clock = mock.clock(on, { now: 1_000_000 });
  const env: Record<string, string> = {
    HANGAR_PLUGIN_URL: "http://127.0.0.1:1/api/plugin", HANGAR_PLUGIN_TOKEN: "tok", CP_SESSION_NAME: "sessao-h",
  };
  const receipts: Record<string, unknown>[] = [];
  let published = false;
  on("env.get", ($, e) => ({ value: env[e.name] }));
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.id", () => ({ value: "sid" }) as never);
  on("command.register", () => ({ value: undefined }) as never);
  on("prompt.submit", () => {
    if (result === null) throw new Error("envio incerto");
    return result;
  });
  on("http.fetch", ($, e) => {
    const path = e.url.slice(e.url.lastIndexOf("/") + 1);
    const body = JSON.parse(e.init?.body as string) as Record<string, unknown>;
    if (path === "submitted") receipts.push(body);
    const text = path === "pull" && !published
      ? (published = true, JSON.stringify({ text: "pode continuar", modo: "user", publication_id: "p1", generation: 7, session_id: "sid" }))
      : '{"text":null}';
    return { value: { status: 200, ok: true, headers: {}, text } } as never;
  });
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  await clock.advance(60);
  expect(receipts).toEqual([
    { sessao: "sessao-h", token: "tok", ok, dropped, publication_id: "p1", generation: 7, session_id: "sid" },
  ]);
});

test("sem terminal: o `/pull` só declara agent_message, entrega ao subagente e ignora prompt", {}, async ($, on) => {
  const relogio = mock.clock(on, { now: 1_000_000 });
  const env: Record<string, string> = {
    HANGAR_PLUGIN_URL: "http://127.0.0.1:1/api/plugin", HANGAR_PLUGIN_TOKEN: "tok", CP_SESSION_NAME: "sessao-h",
  };
  const posts: { path: string; body: Record<string, unknown> }[] = [];
  const enviados: { to: string; text: string }[] = [];
  const prompts: string[] = [];
  const respostas = [
    '{"text":"oi","modo":"user","publication_id":"p","generation":1}',
    '{"text":null,"agent_message":{"id":"m1","to":"a1","text":"olá"}}',
  ];
  on("env.get", ($, e) => ({ value: env[e.name] }));
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.id", () => ({ value: "sid" }) as never);
  on("command.register", () => ({ value: undefined }) as never);
  on("session.send", ($, e) => {
    enviados.push({ to: e.to, text: e.text });
    return { isDelivered: true } as never;
  });
  on("prompt.submit", ($, e) => {
    prompts.push(e.text);
    return { text: e.text };
  });
  on("http.fetch", ($, e) => {
    const path = e.url.slice(e.url.lastIndexOf("/") + 1);
    posts.push({ path, body: JSON.parse(e.init?.body as string) });
    const text = path === "pull" ? respostas.shift() : '{"ok":true}';
    const status = text === undefined ? 500 : 200;
    return { value: { status, ok: status === 200, headers: {}, text: text ?? "" } } as never;
  });
  await $.session.start({ cwd: "/tmp", surface: null, isInteractive: false });
  await relogio.advance(60);
  await relogio.advance(60);

  const pulls = posts.filter((p) => p.path === "pull").map((p) => p.body);
  expect(pulls.length).toBeGreaterThanOrEqual(2);
  for (const pull of pulls) expect(pull).toMatchObject({ sessao: "sessao-h", token: "tok", modos: ["agent_message"], session_id: "sid" });
  expect(pulls[0]!.estado).toBeUndefined();
  expect(enviados).toEqual([{ to: "a1", text: "olá" }]);
  expect(posts.filter((p) => p.path === "agent-sent").map((p) => p.body)).toEqual([
    { sessao: "sessao-h", token: "tok", id: "m1", isDelivered: true },
  ]);
  // A entrada da sessão sem terminal é do stdin do runtime: nada de prompt nem recibo por aqui.
  expect(prompts).toEqual([]);
  expect(posts.map((p) => p.path).filter((p) => p !== "pull" && p !== "agent-sent" && p !== "ui")).toEqual([]);
});

test("ponte do `whoami` recusada pelo backend novo: pergunta de novo e segue com a chave nova", {}, async ($, on) => {
  const relogio = mock.clock(on, { now: 1_000_000 });
  const env: Record<string, string> = { HOME: "/home/x", TMUX_PANE: "%3" };
  const tokens = ["velha", "nova"];
  const whoamis: number[] = [];
  const pulls: string[] = [];
  on("env.get", ($, e) => ({ value: env[e.name] }));
  on("fs.read", () => ({ value: '{"url":"http://127.0.0.1:1/api/plugin","chave":"k"}' }) as never);
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.id", () => ({ value: "sid" }) as never);
  on("command.register", () => ({ value: undefined }) as never);
  on("http.fetch", ($, e) => {
    const path = e.url.slice(e.url.lastIndexOf("/") + 1);
    const resp = (status: number, text: string) => ({ value: { status, ok: status === 200, headers: {}, text } }) as never;
    if (path === "whoami") {
      whoamis.push(Date.now());
      return resp(200, JSON.stringify({ sessao: "s", token: tokens.shift() ?? "nova" }));
    }
    if (path !== "pull") return resp(200, '{"ok":true}');
    const token = (JSON.parse(e.init?.body as string) as { token: string }).token;
    pulls.push(token);
    // O Rust que subiu no lugar do Python não reconhece a chave que o `whoami` antigo deu.
    return token === "velha" ? resp(409, '{"error":"plugin_identity_missing"}') : resp(200, '{"text":null}');
  });
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  await relogio.advance(60);
  expect(pulls).toEqual(["velha"]);
  await relogio.advance(30_100);
  expect(whoamis.length).toBe(2);
  expect(pulls).toContain("nova");
});

test("ponte recusada e `whoami` falhando com o backend ainda subindo: o plugin não desiste", {}, async ($, on) => {
  const relogio = mock.clock(on, { now: 1_000_000 });
  const env: Record<string, string> = { HOME: "/home/x", TMUX_PANE: "%3" };
  const whoami = [200, 503, 200];
  const tokens = ["velha", "nova"];
  const pulls: string[] = [];
  on("env.get", ($, e) => ({ value: env[e.name] }));
  on("fs.read", () => ({ value: '{"url":"http://127.0.0.1:1/api/plugin","chave":"k"}' }) as never);
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.id", () => ({ value: "sid" }) as never);
  on("command.register", () => ({ value: undefined }) as never);
  on("http.fetch", ($, e) => {
    const path = e.url.slice(e.url.lastIndexOf("/") + 1);
    const resp = (status: number, text: string) => ({ value: { status, ok: status === 200, headers: {}, text } }) as never;
    if (path === "whoami") {
      const status = whoami.shift() ?? 200;
      return status === 200 ? resp(200, JSON.stringify({ sessao: "s", token: tokens.shift() ?? "nova" })) : resp(status, "");
    }
    if (path !== "pull") return resp(200, '{"ok":true}');
    const token = (JSON.parse(e.init?.body as string) as { token: string }).token;
    pulls.push(token);
    return token === "velha" ? resp(409, '{"error":"plugin_identity_missing"}') : resp(200, '{"text":null}');
  });
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  // 409 espera 30 s: descoberta que falha, nova tentativa na ponte velha, descoberta que dá certo.
  await relogio.advance(100_000);
  expect(pulls).toContain("nova");
});

for (const { code, back } of [
  // A sessão reabrindo no Hangar (vínculo trocando): a ponte volta em segundos, não fica 30 s sem provar a conversa.
  { code: "erro_interacao_desatualizada", back: true },
  { code: "plugin_identity_missing", back: true },
  // Outra instância é dona da sessão: segue a espera longa.
  { code: "plugin_instance_conflict", back: false },
]) test(`ponte do lançamento recusada com ${code}: ${back ? "tenta de novo em segundos" : "espera a outra instância"}`, {}, async ($, on) => {
  const relogio = mock.clock(on, { now: 1_000_000 });
  const env: Record<string, string> = { HANGAR_PLUGIN_URL: "http://127.0.0.1:1/api/plugin", HANGAR_PLUGIN_TOKEN: "k.t", CP_SESSION_NAME: "s" };
  let pulls = 0;
  on("env.get", ($, e) => ({ value: env[e.name] }));
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.id", () => ({ value: "sid" }) as never);
  on("command.register", () => ({ value: undefined }) as never);
  on("http.fetch", ($, e) => {
    const resp = (status: number, text: string) => ({ value: { status, ok: status === 200, headers: {}, text } }) as never;
    if (!e.url.endsWith("/pull")) return resp(200, '{"ok":true}');
    pulls++;
    return pulls === 1 ? resp(409, JSON.stringify({ detail: { code } })) : resp(200, '{"text":null}');
  });
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  await relogio.advance(5_000);
  expect(pulls > 1).toBe(back);
});

/** Composer de mentira: `typed` é a tecla da pessoa que chega logo depois do espaço; `failBack` recusa a volta. */
function composer(draft: string, opts: { typed?: string; failBack?: boolean } = {}) {
  const box = { text: draft };
  const read = async () => ({ text: box.text });
  const fill = async (a: { text: string; mode: "append" | "replace" }) => {
    if (a.mode === "replace" && opts.failBack) return { isFilled: false };
    box.text = a.mode === "append" ? box.text + a.text + (opts.typed ?? "") : a.text;
    return { isFilled: true };
  };
  return { box, read, fill };
}

test("devolve o foco e deixa o rascunho como estava", async () => {
  const c = composer("ls -la");
  expect(await returnFocus(c.read, c.fill)).toEqual({ moved: true, restored: true });
  expect(c.box.text).toBe("ls -la");
});

test("uma tecla da pessoa no meio fica no rascunho", async () => {
  const c = composer("ls", { typed: "x" });
  expect(await returnFocus(c.read, c.fill)).toEqual({ moved: true, restored: true });
  expect(c.box.text).toBe("ls x");
});

test("a volta do rascunho que falha não desfaz a devolução do foco, e é avisada", async () => {
  const c = composer("ls", { failBack: true });
  expect(await returnFocus(c.read, c.fill)).toEqual({ moved: true, restored: false });
  expect(c.box.text).toBe("ls ");
});

test("o espaço que lança ainda tenta restaurar", async () => {
  const c = composer("ls");
  const fill = async (a: { text: string; mode: "append" | "replace" }) => {
    if (a.mode === "append") { c.box.text += " "; throw new Error("fill"); }
    return c.fill(a);
  };
  await expect(returnFocus(c.read, fill)).rejects.toThrow("fill");
  expect(c.box.text).toBe("ls");
});
