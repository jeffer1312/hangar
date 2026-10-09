import { expect, test } from "claude-code/testing";
import { bandBody, scrollOffset, shownAfterClose, UNDRAWN, type PaneEntry } from "./uiPayload";

const pane = (id: string, size = 10): PaneEntry => ({
  id, title: id, placement: "dock", columns: 72, tree: { type: "Text", children: ["x".repeat(size)] },
});

test("faixa, bodyColumns, painéis e o painel na frente vão juntos", async () => {
  const body = JSON.parse(`{${bandBody({ type: "Box" }, 82, [pane("review-mr")], "review-mr", 1000)}}`);
  expect(body.above).toEqual({ type: "Box" });
  // `columns` é o de hoje (a coluna da conversa, com o `[-]`), que o Python lê; `bodyColumns` é o do evento.
  expect(body.columns).toBe(87);
  expect(body.bodyColumns).toBe(82);
  expect(body.panes.map((p: PaneEntry) => p.id)).toEqual(["review-mr"]);
  expect(body.shown).toBe("review-mr");
  expect(body.caps).toEqual([]);
});

test("o que o plugin atende vai junto, mesmo quando os painéis saem pelo teto", async () => {
  expect(JSON.parse(`{${bandBody(null, 82, [], null, 1000, ["btw"])}}`).caps).toEqual(["btw"]);
  expect(JSON.parse(`{${bandBody({ type: "Box" }, 82, [pane("grande", 5000)], "grande", 300, ["btw"])}}`).caps).toEqual(["btw"]);
});

test("shown de painel que não está na lista vai como null", async () => {
  expect(JSON.parse(`{${bandBody(null, 82, [pane("a")], "sumiu", 1000)}}`).shown).toBeNull();
});

test("marcador do engine é faixa vazia", async () => {
  expect(JSON.parse(`{${bandBody({ type: "engine", ref: 1 }, 82, [], null, 1000)}}`).above).toBeNull();
});

test("acima do teto os painéis saem antes da faixa", async () => {
  const body = JSON.parse(`{${bandBody({ type: "Box" }, 82, [pane("grande", 5000)], "grande", 300)}}`);
  expect(body.panes).toEqual([]);
  expect(body.above).toEqual({ type: "Box" });
  expect(body.shown).toBeNull();
});

test("painel ainda não desenhado vai com o nó vazio do engine", async () => {
  expect(UNDRAWN).toEqual({ type: "engine", ref: 0 });
});

test("fechado o painel da frente, aparece o vizinho anterior", async () => {
  expect(shownAfterClose(["a", "b", "c"], "c", "c")).toBe("b");
  expect(shownAfterClose(["a", "b", "c"], "a", "a")).toBe("b");
  expect(shownAfterClose(["a", "b", "c"], "b", "c")).toBe("b");
  expect(shownAfterClose(["a"], "a", "a")).toBeNull();
});

test("o offset vai preso ao conteúdo", async () => {
  expect(scrollOffset({ offset: 999, bodyRows: 38, contentRows: 205 })).toBe(167);
  expect(scrollOffset({ offset: -3, bodyRows: 38, contentRows: 205 })).toBe(0);
  expect(scrollOffset({ offset: 4, bodyRows: 38, contentRows: 10 })).toBe(0);
});
