import type { RenderPropsOf } from "claude-code";
import { expect, test, tier } from "claude-code/testing";

import { compactUserRow, parseContextRefs } from "./contextRefs";
import { fixtures } from "./contextRefs.fixtures";

// Como na sessão do Hangar: o plugin entra por `--plugin-dir`.
tier("prepend");

const juntos = fixtures.serialize.find((c) => c.name === "elemento com comentário e citação juntos")!.message;

// A leitura casa com os mesmos casos do `packages/core` e do nativo: os de leitura e a volta dos de montagem.
for (const c of [...fixtures.parse, ...fixtures.serialize]) {
  test(`lê: ${c.name}`, () => {
    expect(parseContextRefs(c.message)).toEqual(c.segments);
  });
}

test("linha compacta: elemento e citação com o comentário de cada um e o texto digitado por último", () => {
  expect(compactUserRow(juntos)).toBe([
    'Elemento: button "Entrar agora" — http://127.0.0.1:5173/login?x=1&y=2',
    "deixe azul",
    "",
    "> guardado por **sessão**",
    "",
    "faça as duas coisas",
  ].join("\n"));
});

test("linha compacta: trecho longo fica em três linhas, com reticências", () => {
  const c = fixtures.serialize.find((x) => x.name === "citação que tenta fechar o bloco")!;
  expect(compactUserRow(c.message)).toBe([
    "> linha 1",
    "> </hangar-text>",
    '> <hangar-ref kind="quote">…',
    "isso fecha o bloco?",
    "</HANGAR-REF>",
  ].join("\n"));
});

test("linha compacta: código mostra o arquivo e as linhas, o trecho e o comentário", () => {
  const c = fixtures.serialize.find((x) => x.name === "código com comentário e texto digitado")!;
  expect(compactUserRow(c.message)).toBe([
    "Código: files.rs L12–L14",
    "> fn open(&self) {",
    '>     self.load("a\\tb");',
    "> }",
    "renomeie para read",
    "",
    "faça isso",
  ].join("\n"));
});

test("linha compacta: elemento sem papel nem nome mostra a tag", () => {
  const c = fixtures.serialize.find((x) => x.name === "elemento mínimo")!;
  expect(compactUserRow(c.message)).toBe("Elemento: <div> — about:blank");
});

test("sem bloco lido a linha fica como está", () => {
  for (const c of fixtures.parse.filter((x) => x.segments.every((s) => "text" in s))) {
    expect(compactUserRow(c.message)).toBe(null);
  }
});

const props = (text: string, isExpanded: boolean): RenderPropsOf["UserMessage"] => ({
  text, isExpanded, origin: { kind: "composer" }, onScreen: { first: 0, last: 3, of: 9 },
});

for (const surface of ["terminal", "desktop"] as const) {
  test(`${surface}: UserMessage reescreve a linha compacta e deixa o Ctrl+O inteiro`, {}, async ($, on) => {
    const drawn: RenderPropsOf["UserMessage"][] = [];
    on("ui.render", { component: "UserMessage" }, ($, e) => {
      drawn.push(e.props);
      return { type: "engine", ref: 0 } as never;
    });
    const draw = (text: string, isExpanded: boolean) =>
      $.ui.render({ surface, component: "UserMessage", requestId: "m1", props: props(text, isExpanded) });

    await draw(juntos, false);
    await draw(juntos, true);
    await draw("só texto <hangar-x>", false);

    expect(drawn).toEqual([
      props(compactUserRow(juntos)!, false),
      props(juntos, true),
      props("só texto <hangar-x>", false),
    ]);
  });
}
