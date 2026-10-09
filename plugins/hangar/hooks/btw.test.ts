import { expect, mock, test, tier } from "claude-code/testing";
import type { Engine } from "claude-code/testing";
import type { On } from "claude-code";
import { BTW_MAX, BTW_PANE, BTW_TIMEOUT_MS, btwSupported, pushEntry, recentEntries, type BtwEntry } from "./btw";
import { agentDone, forgetAgent, waitAgent } from "./bridge";

// Como na sessão do Hangar: o plugin entra por `--plugin-dir` e fica por fora dos outros mods.
tier("prepend");

type Fork = { isAnswered: true; text: string; usage: Record<string, number> } | { isAnswered: false; reason: string; [k: string]: unknown };
const usage = { input_tokens: 1, output_tokens: 1, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 };
const answered = (text: string): Fork => ({ isAnswered: true, text, usage });

// Sessão com terminal sem ponte do Hangar (env vazio): só o `/btw` e o painel entram na conta.
function sessao(on: On, forks: (prompt: string) => Fork | Promise<Fork>, base = "2.1.294") {
  const pedidos: string[] = [];
  const abertos: string[] = [];
  const copias: string[] = [];
  on("env.get", () => ({ value: undefined }));
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.version", () => ({ value: { version: base, base } }) as never);
  on("model.fork", async ($, e) => {
    pedidos.push(e.prompt);
    return { value: await forks(e.prompt) } as never;
  });
  on("ui.open", ($, e) => {
    abertos.push(e.id);
    return { value: { isPlaced: true } } as never;
  });
  on("ui.copy", ($, e) => {
    copias.push(e.text);
    return { value: { isCopied: true } } as never;
  });
  on("command.run", () => ({ text: "overlay do Claude Code" }));
  return { pedidos, abertos, copias };
}

// O `/btw` como o Enter do campo o roda.
const btw = ($: Engine, args: string) => $.command.run({
  command: "btw", args, origin: { kind: "composer" }, presentation: { isFullscreen: true, columns: 200 },
});

const painel = ($: Engine) => $.ui.mount({
  plugin: "hangar", surface: "terminal", component: "Pane", requestId: BTW_PANE,
  props: { title: "Pergunta lateral", isFocused: false, bodyColumns: 60, placement: "dock", scroll: { offset: 0, bodyRows: 40 }, view: {} } as never,
});

const textos = async (ui: Awaited<ReturnType<typeof painel>>) => (await ui.findAll({ type: "Text" })).map((t) => t.text);
const botoes = async (ui: Awaited<ReturnType<typeof painel>>) => (await ui.findAll({ type: "Button" })).map((b) => b.key);

test("/btw abre o painel na hora, responde pelo fork e não deixa nada para o transcript", {}, async ($, on) => {
  const relogio = mock.clock(on, { now: 1_000_000 });
  let solta: (f: Fork) => void = () => {};
  const s = sessao(on, () => new Promise<Fork>((r) => { solta = r; }));
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });

  const r = await btw($, "  qual é a palavra-código?  ");
  expect(r).toEqual({});
  expect(s.abertos).toEqual([BTW_PANE]);
  expect(s.pedidos).toHaveLength(1);
  expect(s.pedidos[0]).toContain("qual é a palavra-código?");
  expect(s.pedidos[0]).toContain("do not call tools");

  const ui = await painel($);
  expect(await textos(ui)).toContain("Respondendo…");
  expect(await botoes(ui)).not.toContain("copiar");

  solta(answered("A palavra-código é **ABACAXI-42**."));
  await relogio.settle();
  const md = await ui.find({ type: "Markdown" });
  expect(md?.text).toBe("A palavra-código é **ABACAXI-42**.");

  await ui.press({ key: "copiar" });
  expect(s.copias).toEqual(["A palavra-código é **ABACAXI-42**."]);
  await ui.unmount();
});

test("erro claro e Repetir pergunta de novo", {}, async ($, on) => {
  mock.clock(on, { now: 1_000_000 });
  const respostas: Fork[] = [{ isAnswered: false, reason: "nothing-to-fork" }, answered("agora sim")];
  const s = sessao(on, () => respostas.shift()!);
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  await btw($, "oi");
  const ui = await painel($);
  expect((await textos(ui)).join("\n")).toContain("Mande uma mensagem, espere a resposta e repita");
  await ui.press({ key: "repetir" });
  expect(s.pedidos).toHaveLength(2);
  expect((await ui.find({ type: "Markdown" }))?.text).toBe("agora sim");
  await ui.unmount();
});

test("API recusou: o status e o tipo do erro aparecem", {}, async ($, on) => {
  mock.clock(on, { now: 1_000_000 });
  sessao(on, () => ({ isAnswered: false, reason: "api-error", status: 529, error: "overloaded", usage }));
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  await btw($, "oi");
  const ui = await painel($);
  expect((await textos(ui)).join("\n")).toContain("HTTP 529");
  expect((await textos(ui)).join("\n")).toContain("overloaded");
  await ui.unmount();
});

test("sem resposta no prazo vira erro, e a resposta atrasada é descartada", {}, async ($, on) => {
  const relogio = mock.clock(on, { now: 1_000_000 });
  let solta: (f: Fork) => void = () => {};
  sessao(on, () => new Promise<Fork>((r) => { solta = r; }));
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  await btw($, "demora?");
  await relogio.advance(BTW_TIMEOUT_MS);
  const ui = await painel($);
  expect((await textos(ui)).join("\n")).toContain("2 minutos");
  solta(answered("tarde demais"));
  await relogio.advance(0);
  expect(await ui.find({ type: "Markdown" })).toBeUndefined();
  expect(await botoes(ui)).toContain("repetir");
  await ui.unmount();
});

test("/btw vazio reabre a última; anterior, próxima, recentes e limpar", {}, async ($, on) => {
  mock.clock(on, { now: 1_000_000 });
  const s = sessao(on, (prompt) => answered(`resposta de ${prompt.split("\n").at(-1)}`));
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });

  // Sem histórico: o painel abre vazio, sem perguntar nada ao modelo.
  expect(await btw($, "")).toEqual({});
  let ui = await painel($);
  expect((await textos(ui)).join("\n")).toContain("Nenhuma pergunta ainda");
  await ui.unmount();

  for (const q of ["um", "dois", "tres"]) await btw($, q);
  ui = await painel($);
  expect((await ui.find({ type: "Markdown" }))?.text).toBe("resposta de tres");
  expect(await botoes(ui)).toEqual(expect.arrayContaining(["anterior", "limpar", "fechar", "recente-1", "recente-2", "recente-3"]));
  expect(await botoes(ui)).not.toContain("proxima");

  await ui.press({ key: "anterior" });
  expect((await ui.find({ type: "Markdown" }))?.text).toBe("resposta de dois");
  await ui.press({ key: "recente-1" });
  expect((await ui.find({ type: "Markdown" }))?.text).toBe("resposta de um");
  await ui.press({ key: "proxima" });
  expect((await ui.find({ type: "Markdown" }))?.text).toBe("resposta de dois");
  await ui.unmount();

  // `/btw` vazio volta para a última, sem nova pergunta.
  await btw($, "");
  expect(s.pedidos).toHaveLength(3);
  ui = await painel($);
  expect((await ui.find({ type: "Markdown" }))?.text).toBe("resposta de tres");
  await ui.press({ key: "limpar" });
  expect((await textos(ui)).join("\n")).toContain("Nenhuma pergunta ainda");
  await ui.unmount();
});

test("o painel do /btw vai ao app pelo /ui, com o `btw` entre o que o plugin atende", {}, async ($, on) => {
  const relogio = mock.clock(on, { now: 1_000_000 });
  const env: Record<string, string> = {
    HANGAR_PLUGIN_URL: "http://127.0.0.1:1/api/plugin", HANGAR_PLUGIN_TOKEN: "tok", CP_SESSION_NAME: "sessao-a",
  };
  const uis: { caps: string[]; panes: { id: string; tree: unknown }[] }[] = [];
  on("env.get", ($, e) => ({ value: env[e.name] }));
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.id", () => ({ value: "sid" }) as never);
  on("session.version", () => ({ value: { version: "2.1.294", base: "2.1.294" } }) as never);
  on("model.fork", () => ({ value: answered("resposta **forte**") }) as never);
  on("ui.open", () => ({ value: { isPlaced: true } }) as never);
  on("ui.close", () => ({ value: undefined }) as never);
  on("http.fetch", ($, e) => {
    const path = e.url.slice(e.url.lastIndexOf("/") + 1);
    if (path === "ui") uis.push(JSON.parse(e.init?.body as string));
    // O `/pull` falha: o laço de entrega espera entre as voltas e fica fora do caminho.
    const r = path === "pull" ? { status: 500, text: "" } : { status: 200, text: '{"ok":true}' };
    return { value: { ...r, ok: r.status === 200, headers: {} } } as never;
  });
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  await btw($, "oi");
  const ui = await painel($);
  await relogio.advance(600);
  const ultimo = uis.at(-1)!;
  expect(ultimo.caps).toEqual(["btw"]);
  expect(ultimo.panes.map((p) => p.id)).toEqual([BTW_PANE]);
  expect(JSON.stringify(ultimo.panes[0]!.tree)).toContain("resposta **forte**");
  // O app nativo desenha pelos dados, com o tema dele; a árvore fica para o terminal.
  expect((ultimo.panes[0] as { data?: unknown }).data).toMatchObject({
    current: 0, entries: [{ question: "oi", status: "done", answer: "resposta **forte**", fork: "" }],
  });
  // Sem dono o app não aciona o botão: a cópia leva o nome do plugin que o engine só poria na saída.
  const donos = new Set<string>();
  const varre = (n: unknown): void => {
    if (!n || typeof n !== "object") return;
    const no = n as { type?: string; press?: { plugin: string }; children?: unknown[] };
    if (no.type === "Button") donos.add(no.press?.plugin ?? "(sem press)");
    no.children?.forEach(varre);
  };
  varre(ultimo.panes[0]!.tree);
  expect([...donos]).toEqual(["hangar"]);
  // Fechar pelo botão do painel tira o painel do app também.
  await ui.press({ key: "fechar" });
  await relogio.advance(600);
  expect(uis.at(-1)!.panes.map((p) => p.id)).toEqual([]);
  await ui.unmount();
  // Reaberto pelo `/btw` vazio, volta ao app.
  await btw($, "");
  const de_novo = await painel($);
  await relogio.advance(600);
  expect(uis.at(-1)!.panes.map((p) => p.id)).toEqual([BTW_PANE]);
  await de_novo.unmount();
});

test("Bifurcar pede um subagente fork com a pergunta; recusa aparece no painel e o botão volta para tentar de novo", {}, async ($, on) => {
  mock.clock(on, { now: 1_000_000 });
  sessao(on, () => answered("resposta"));
  // O kit apaga o `agentId` de um spawn respondido pelo mock: o caminho até a conversa principal é provado
  // na sessão real (harnesses.md, pergunta lateral).
  const pedidos: string[] = [];
  on("agent.spawn", ($, e) => {
    pedidos.push(e.prompt);
    return { deny: "bloqueado por política" } as never;
  });
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  await btw($, "rode os testes");
  const ui = await painel($);
  await ui.press({ key: "bifurcar" });
  expect(pedidos).toEqual(["rode os testes"]);
  expect((await textos(ui)).join("\n")).toContain("Não deu para bifurcar: bloqueado por política");
  expect(await botoes(ui)).toContain("bifurcar");
  await ui.unmount();
});

test("sem terminal: /btw pelo SDK é descartado, abre o painel e o estado vai ao app pela ponte da superfície", {}, async ($, on) => {
  mock.clock(on, { now: 1_000_000 });
  const env: Record<string, string> = {
    HANGAR_PLUGIN_URL: "http://127.0.0.1:1/api/plugin", HANGAR_PLUGIN_TOKEN: "tok", CP_SESSION_NAME: "sessao-h",
  };
  const uis: { caps: string[]; panes: { id: string; data?: { entries: { question: string; status: string }[] } }[] }[] = [];
  const pedidos: string[] = [];
  const abaixo: string[] = [];
  on("env.get", ($, e) => ({ value: env[e.name] }));
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.version", () => ({ value: { version: "2.1.294", base: "2.1.294" } }) as never);
  on("model.fork", ($, e) => {
    pedidos.push(e.prompt);
    return { value: answered("KIWI-9") } as never;
  });
  on("ui.open", () => ({ value: { isPlaced: true } }) as never);
  on("prompt.submit", ($, e) => {
    abaixo.push(e.text);
    return { text: e.text };
  });
  on("http.fetch", ($, e) => {
    if (e.url.endsWith("/ui")) uis.push(JSON.parse(e.init?.body as string));
    return { value: { status: 200, ok: true, headers: {}, text: '{"ok":true}' } } as never;
  });
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: false });
  // Na largada o app já fica sabendo que o `/btw` é atendido.
  expect(uis[0]?.caps).toEqual(["btw"]);

  const r = await $.prompt.submit({ text: "/hangar-btw qual a palavra-código?", wait: false, origin: { kind: "sdk" } });
  expect(r).toMatchObject({ drop: "Pergunta lateral respondida no painel do Hangar." });
  expect(abaixo).toEqual([]);
  expect(pedidos.at(-1)).toContain("qual a palavra-código?");
  const ultimo = uis.at(-1)!;
  expect(ultimo.caps).toEqual(["btw"]);
  expect(ultimo.panes[0]?.data?.entries[0]).toMatchObject({ question: "qual a palavra-código?", status: "done" });

  // Outro texto segue normal.
  await $.prompt.submit({ text: "oi", wait: false, origin: { kind: "sdk" } });
  expect(abaixo).toEqual(["oi"]);
});

test("CLI anterior ao fork: o /btw segue para o Claude Code", {}, async ($, on) => {
  mock.clock(on, { now: 1_000_000 });
  const s = sessao(on, () => answered("x"), "2.1.288");
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  expect(await btw($, "oi")).toMatchObject({ text: "overlay do Claude Code" });
  expect(s.pedidos).toEqual([]);
  expect(s.abertos).toEqual([]);
});

test("histórico fica nas 20 mais novas e os recentes são as 5 últimas, da mais nova para trás", async () => {
  let list: BtwEntry[] = [];
  let index = -1;
  for (let id = 1; id <= BTW_MAX + 3; id++) {
    ({ list, index } = pushEntry(list, { id, question: `q${id}`, status: "done", answer: "", error: "", attempt: 1, fork: "", forkState: "", askedAt: 0 }));
  }
  expect(list).toHaveLength(BTW_MAX);
  expect(list[0]!.id).toBe(4);
  expect(index).toBe(BTW_MAX - 1);
  expect(recentEntries(list).map((r) => r.entry.id)).toEqual([23, 22, 21, 20, 19]);
  expect(recentEntries(list)[0]!.index).toBe(BTW_MAX - 1);
});

test("versão mínima do CLI", async () => {
  expect(btwSupported("2.1.289")).toBe(true);
  expect(btwSupported("2.1.294-dev")).toBe(true);
  expect(btwSupported("2.2.0")).toBe(true);
  expect(btwSupported("2.1.288")).toBe(false);
  expect(btwSupported(undefined)).toBe(false);
  expect(btwSupported("x")).toBe(false);
});

test("fim de subagente que chega antes da espera não se perde", async () => {
  agentDone("cedo", { answer: "14", isAborted: false });
  expect(await waitAgent("cedo")).toEqual({ answer: "14", isAborted: false });
  const depois = waitAgent("tarde");
  agentDone("tarde", { answer: "ok", isAborted: false });
  expect(await depois).toEqual({ answer: "ok", isAborted: false });
  agentDone("esquecido", { answer: "x", isAborted: false });
  forgetAgent("esquecido");
  let chegou = false;
  void waitAgent("esquecido").then(() => { chegou = true; });
  await Promise.resolve();
  expect(chegou).toBe(false);
  forgetAgent("esquecido");
});
