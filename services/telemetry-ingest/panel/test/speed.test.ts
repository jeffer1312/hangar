// Roda com `npm test` (node --test, sem bundler): só a lógica pura do bloco Velocidade.
import { test } from "node:test";
import assert from "node:assert/strict";
import { speedCards, type SpeedRow } from "../src/lib/speed.ts";

const row = (event: string, bucket: string, count: number, os = "linux", dims: Record<string, string> = {}): SpeedRow =>
  ({ event, dims, bucket, os, count, machines: 1 });

test("normalmente é a faixa da mediana e 9 em 10 a do p90", () => {
  const rows = [row("chat_open", "lt300ms", 5, "linux", { client: "native" }), row("chat_open", "lt1s", 4, "linux", { client: "native" }),
    row("chat_open", "lt3s", 1, "linux", { client: "ios" })];
  const card = speedCards(rows, "all").find(c => c.event === "chat_open")!;
  assert.equal(card.total, 10);
  assert.equal(card.usually, "menos de 300 ms");
  assert.equal(card.nine, "menos de 1 s");
  assert.deepEqual(card.buckets, [0, 5, 4, 1, 0, 0, 0]);
  assert.equal(card.verdict, "ok");
  assert.deepEqual(card.rows.map(r => [r.key, r.usually]), [["native", "menos de 300 ms"], ["ios", "menos de 3 s"]]);
});

test("abrir conversa nova conta só o que a pessoa abriu", () => {
  const rows = [row("session_ready", "lt10s", 8, "windows", { opened_by: "person", provider: "claude" }),
    row("session_ready", "lt1s", 50, "linux", { opened_by: "agent", provider: "claude" })];
  const card = speedCards(rows, "all").find(c => c.event === "session_ready")!;
  assert.equal(card.total, 8);
  assert.equal(card.usually, "menos de 10 s");
  assert.equal(card.verdict, "warn");
  assert.deepEqual(card.rows.map(r => r.key), ["windows"]);
});

test("o filtro de sistema deixa só os pacotes dele", () => {
  const rows = [row("first_reply", "lt3s", 4, "linux", { provider: "codex" }), row("first_reply", "ge30s", 6, "windows", { provider: "codex" })];
  const windows = speedCards(rows, "windows").find(c => c.event === "first_reply")!;
  assert.equal(windows.total, 6);
  assert.equal(windows.usually, "30 s ou mais");
  assert.equal(windows.verdict, "bad");
});

test("sem medida o cartão fica vazio, e com poucas não julga", () => {
  const cards = speedCards([row("app_ready", "lt1s", 2, "linux", { client: "ios" })], "all");
  assert.equal(cards.length, 6, "os seis cartões sempre aparecem");
  assert.equal(cards.find(c => c.event === "server_startup")!.total, 0);
  assert.equal(cards.find(c => c.event === "app_ready")!.verdict, "few");
});
