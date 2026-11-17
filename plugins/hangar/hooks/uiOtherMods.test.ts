import { expect, mock, test, tier } from "claude-code/testing";
import type { Engine } from "claude-code/testing";
import type { On } from "claude-code";
import { registerOutro } from "./uiOtherModsMod";

// Como na sessão do Hangar: o plugin entra por `--plugin-dir`, no nível `user`, por fora dos mods instalados.
tier("user");

type Post = { path: string; body: Record<string, unknown> };
const PANE = { title: "Outro", isFocused: false, bodyColumns: 58, placement: "dock", scroll: { offset: 0, bodyRows: 40 }, view: {} } as never;
const PLUGINS = { plugins: [{ name: "outro", register: registerOutro }] };

async function start($: Engine, on: On) {
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
  on("fs.list", () => ({ value: [] }) as never);
  on("fs.exists", () => ({ value: false }) as never);
  on("http.fetch", ($, e) => {
    const path = e.url.slice(e.url.lastIndexOf("/") + 1);
    posts.push({ path, body: JSON.parse(e.init?.body as string) as Record<string, unknown> });
    // O `/pull` falha: o laço de entrega espera entre as voltas e fica fora do caminho.
    const r = path === "pull" ? { status: 500, text: "" } : { status: 200, text: '{"ok":true}' };
    return { value: { ...r, ok: r.status === 200, headers: {} } } as never;
  });
  on("ui.open", () => ({ value: { isPlaced: true } }) as never);
  await $.session.start({ cwd: "/tmp", surface: "terminal", isInteractive: true });
  return { clock, posts };
}

const lastUi = (posts: Post[]) => posts.filter((p) => p.path === "ui").at(-1)?.body as
  { above: unknown; panes: { id: string; tree: unknown }[] } | undefined;

test("o painel e a linha da faixa de outro mod vão ao app desenhados", PLUGINS, async ($, on) => {
  const { clock, posts } = await start($, on);
  const painel = await $.ui.mount({ plugin: "outro", surface: "terminal", component: "Pane", requestId: "outro-painel", props: PANE });
  await painel.drawn();
  const faixa = await $.ui.mount({ plugin: "outro", surface: "terminal", component: "AbovePrompt", props: { bodyColumns: 80, hasSurvey: false } as never });
  await faixa.drawn();
  await clock.advance(1000);
  const ui = lastUi(posts);
  const tree = JSON.stringify(ui?.panes.find((p) => p.id === "outro-painel")?.tree);
  expect(tree).toContain("corpo do outro");
  expect(tree).toContain('"plugin":"outro"');
  expect(JSON.stringify(ui?.above)).toContain("faixa do outro");
  await faixa.unmount();
  await painel.unmount();
});
