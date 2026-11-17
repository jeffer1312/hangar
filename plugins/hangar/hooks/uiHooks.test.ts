import { expect, mock, test, tier } from "claude-code/testing";
import type { Engine, MockClock } from "claude-code/testing";
import type { On } from "claude-code";
import { registerCascata } from "./uiHooksMod";

// Como na sessão do Hangar: o plugin entra por `--plugin-dir` e fica por fora dos outros mods.
tier("prepend");

type Post = { path: string; body: Record<string, unknown> };
type Answer = (path: string, body: Record<string, unknown>) => { status: number; text: string };

const OK: Answer = () => ({ status: 200, text: '{"ok":true}' });
const PANE = { title: "painel", isFocused: false, bodyColumns: 58, placement: "dock", scroll: { offset: 0, bodyRows: 40 }, view: {} } as never;
const PLUGINS = { plugins: [{ name: "paineis", register: registerCascata }] };

/** Sessão com terminal e a ponte do Hangar; o backend de mentira responde cada rota pelo `answer`. */
async function start($: Engine, on: On, answer: Answer = OK): Promise<{ clock: MockClock; posts: Post[] }> {
  // O relógio só anda quando o teste manda: o envio da faixa (500 ms) sai no `advance`.
  const clock = mock.clock(on, { now: 1_000_000 });
  const env: Record<string, string> = {
    HANGAR_PLUGIN_URL: "http://127.0.0.1:1/api/plugin", HANGAR_PLUGIN_TOKEN: "tok", CP_SESSION_NAME: "sessao-a",
  };
  const posts: Post[] = [];
  on("env.get", ($, e) => ({ value: env[e.name] }));
  on("session.start", ($, e) => ({ cwd: e.cwd }));
  on("session.cwd", () => ({ value: "/tmp" }));
  on("session.model", () => ({ value: "modelo" }));
  on("session.id", () => ({ value: "sid" }) as never);
  on("command.register", () => ({ value: undefined }) as never);
  on("http.fetch", ($, e) => {
    const path = e.url.slice(e.url.lastIndexOf("/") + 1);
    const body = JSON.parse(e.init?.body as string) as Record<string, unknown>;
    posts.push({ path, body });
    // O `/pull` falha: o laço de entrega espera 2 s entre as voltas e fica fora do caminho.
    const r = path === "pull" ? { status: 500, text: "" } : answer(path, body);
    return { value: { ...r, ok: r.status === 200, headers: {} } } as never;
  });
  on("ui.open", ($, e) => ({ value: e.id === "fora" ? { isPlaced: false, reason: "estreito" } : { isPlaced: true } }) as never);
  on("ui.close", () => ({ value: undefined }) as never);
  on("ui.scroll", () => ({}));
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  return { clock, posts };
}

const bodies = (posts: Post[], path: string) => posts.filter((p) => p.path === path).map((p) => p.body);
/** O último `/ui` enviado: os ids dos painéis, na ordem, e o da frente. */
function lastUi(posts: Post[]): { ids: string[]; shown: unknown; trees: unknown[] } {
  const ui = bodies(posts, "ui").at(-1) as { panes: { id: string; tree: unknown }[]; shown: unknown };
  return { ids: ui.panes.map((p) => p.id), shown: ui.shown, trees: ui.panes.map((p) => p.tree) };
}
const mount = ($: Engine, requestId: string) => $.ui.mount({ plugin: "paineis", surface: "terminal", component: "Pane", requestId, props: PANE });
/** O painel dos botões do mod, fora do terminal: abre e fecha os painéis pelo `$` do próprio mod. */
const control = ($: Engine) => $.ui.mount({ plugin: "paineis", surface: "desktop", component: "Pane", requestId: "controle", props: PANE });

test("o painel colocado entra no ui.open; fechado o da frente, aparece o vizinho anterior", PLUGINS, async ($, on) => {
  on("ui.focus", { plugin: "*" }, () => ({}));
  const { clock, posts } = await start($, on);
  const botoes = await control($);
  await botoes.press({ key: "abrir" });
  await clock.advance(600);
  // Colocado e ainda sem desenho vai com a árvore vazia do engine; o não colocado fica de fora.
  expect(lastUi(posts)).toEqual({ ids: ["pm-a", "pm-b", "pm-c"], shown: null, trees: [0, 1, 2].map(() => ({ type: "engine", ref: 0 })) });
  await mount($, "pm-c");
  await botoes.press({ key: "fechar-c" });
  await clock.advance(600);
  expect(lastUi(posts).ids).toEqual(["pm-a", "pm-b"]);
  expect(lastUi(posts).shown).toBe("pm-b");
});

test("o desenho que cruza o fechamento dos três não devolve o painel, e o reaberto vai para o fim", PLUGINS, async ($, on) => {
  on("ui.focus", { plugin: "*" }, () => ({}));
  const { clock, posts } = await start($, on);
  const botoes = await control($);
  await botoes.press({ key: "abrir" });
  await mount($, "pm-a");
  await mount($, "pm-c");
  // O desenho do painel do meio fica no relógio enquanto o mod fecha os três de uma vez.
  const desenho = mount($, "pm-b");
  await clock.settle();
  await botoes.press({ key: "fechar-tres" });
  await clock.advance(1000);
  await desenho;
  await clock.advance(600);
  expect(lastUi(posts).ids).toEqual([]);
  // Reabertos, seguem a ordem de abertura, como as abas do terminal. O reabrir pede desenho de novo, e o
  // painel do meio, ainda montado e fechado, é redesenhado sem entrar na lista.
  await botoes.press({ key: "reabrir" });
  await clock.advance(600);
  expect(lastUi(posts).ids).toEqual(["pm-c", "pm-a"]);
});

test("rolagem e foco com alvo armado vão ao backend, e a key armada entra no evento", PLUGINS, async ($, on) => {
  const focos: (string | undefined)[] = [];
  on("ui.focus", { plugin: "*" }, ($, e) => {
    focos.push(e.element);
    return {};
  });
  const armado: Answer = (path) => path === "focus-target"
    ? { status: 200, text: JSON.stringify({ armed: true, attempt: "t-1", rewrite: "alvo" }) } : OK(path, {});
  const { clock, posts } = await start($, on, armado);
  await $.ui.scroll({ component: "Pane", requestId: "pm-a", offset: 500, by: 3, bodyRows: 10, contentRows: 100, origin: { kind: "person" } });
  await $.ui.focus({ component: "Pane", requestId: "pm-a", plugin: "paineis", element: "outro", origin: { kind: "person" } });
  await clock.settle();
  const ponte = { sessao: "sessao-a", token: "tok" };
  // O `offset` vai preso ao conteúdo: 100 linhas com 10 à vista param em 90.
  expect(bodies(posts, "scroll")).toEqual([{ ...ponte, requestId: "pm-a", offset: 90, bodyRows: 10, contentRows: 100 }]);
  expect(bodies(posts, "focus-target")).toEqual([{ ...ponte, requestId: "pm-a", plugin: "paineis", element: "outro" }]);
  expect(focos).toEqual(["alvo"]);
  expect(bodies(posts, "focused")).toEqual([{ ...ponte, attempt: "t-1", requestId: "pm-a", plugin: "paineis", element: "alvo", denied: false }]);
});

test("na janela do foco armado, o envio do composer só cai com o alvo confirmado como armado", PLUGINS, async ($, on) => {
  on("ui.focus", { plugin: "*" }, () => ({}));
  const enviados: string[] = [];
  on("prompt.submit", ($, e) => {
    enviados.push(e.text);
    return { text: e.text };
  });
  let estado: "armado" | "desarmado" | "fora" = "armado";
  const backend: Answer = (path) => {
    if (path !== "focus-target") return OK(path, {});
    if (estado === "fora") return { status: 503, text: "" };
    return { status: 200, text: JSON.stringify(estado === "armado" ? { armed: true, attempt: "t-1", rewrite: null } : { armed: false, attempt: null, rewrite: null }) };
  };
  const { clock, posts } = await start($, on, backend);
  const composer = (text: string) => $.prompt.submit({ text, wait: false, origin: { kind: "composer" } });
  await $.ui.focus({ component: "Pane", requestId: "pm-a", plugin: "paineis", element: "x", origin: { kind: "person" } });
  await composer("letra no meio do clique");
  expect(enviados).toEqual([]);
  // A ponte sem resposta não confirma o alvo: o envio passa, para a mensagem da fila não sumir como entregue.
  estado = "fora";
  await composer("backend sem resposta");
  expect(enviados).toEqual(["backend sem resposta"]);
  // A janela segue: com o alvo armado de novo, segura.
  estado = "armado";
  await composer("outra letra");
  expect(enviados).toEqual(["backend sem resposta"]);
  // Desarmado antes de a fila soltar: a mensagem dela passa, ainda dentro dos 3 s.
  estado = "desarmado";
  await composer("mensagem da fila");
  expect(enviados).toEqual(["backend sem resposta", "mensagem da fila"]);
  // A pergunta é só de leitura: sem mod nem elemento, com um `requestId` que não é de painel.
  expect(bodies(posts, "focus-target").at(-1)).toMatchObject({ requestId: "prompt", plugin: null, element: null });
  // A janela fechou com o desarme: o próximo envio nem pergunta.
  const perguntas = bodies(posts, "focus-target").length;
  await composer("depois");
  expect(enviados).toEqual(["backend sem resposta", "mensagem da fila", "depois"]);
  expect(bodies(posts, "focus-target")).toHaveLength(perguntas);
  void clock;
});

test("botão do próprio plugin num painel vai ao app com o dono, e o de outro mod mantém o dele", PLUGINS, async ($, on) => {
  on("fs.list", () => ({ value: [] }) as never);
  on("fs.exists", () => ({ value: false }) as never);
  on("tool.call", { tool: "Bash" }, () => ({ result: { stdout: "", stderr: "", interrupted: false, backgroundTaskId: "b1" } }) as never);
  const { clock, posts } = await start($, on);
  await $.tool.call({ tool: "Bash", command: "sleep 600", run_in_background: true } as never);
  await clock.advance(3000);
  const ui = await $.ui.mount({ plugin: "hangar", surface: "terminal", component: "Pane", requestId: "hangar-andamento", props: PANE });
  await ui.drawn();
  await clock.advance(1000);
  const outro = await $.ui.mount({ plugin: "paineis", surface: "terminal", component: "Pane", requestId: "controle", props: PANE });
  await outro.drawn();
  await clock.advance(1000);
  const owners = new Map<string, string>();
  const walk = (n: unknown): void => {
    if (Array.isArray(n)) return n.forEach(walk);
    if (!n || typeof n !== "object") return;
    const node = n as { props?: { key?: string }; press?: { plugin?: string }; children?: unknown };
    if (node.press && node.props?.key) owners.set(node.props.key, node.press.plugin ?? "");
    walk(node.children);
  };
  walk(lastUi(posts).trees);
  expect(lastUi(posts).ids).toEqual(expect.arrayContaining(["hangar-andamento", "controle"]));
  expect([...owners.values()].every(Boolean)).toBe(true);
  expect(owners.get("parar-b1")).toBe("hangar");
  expect(owners.get("abrir")).toBe("paineis");
  await outro.unmount();
  await ui.unmount();
});
