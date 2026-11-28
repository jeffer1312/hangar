// Roda com `npm test`: a conta de quantas versões cada máquina está atrás.
import { test } from "node:test";
import assert from "node:assert/strict";
import { compareVersions, standing, standingText, versionView } from "../src/lib/versions.ts";
import type { MachineLast, Published } from "../src/lib/types.ts";

const rel = (version: string, os = "linux") => ({ version, os, at: "2026-11-28T19:16:19Z" });
// Linux e Windows com 5 versões guardadas cada, como nos releases reais.
const published: Published = { error: null, channels: {
  latest: ["12251", "12256", "12266", "12296", "12300"].map(v => rel("0.1.0." + v))
    .concat(["11980", "11989", "12029", "12238", "12251"].map(v => rel("0.1.0." + v, "windows"))),
  beta: [rel("0.1.0.12244")],
} };
const m = (version: string, os = "linux", channel = "latest", label = "m", install: string | null = "package"): MachineLast =>
  ({ label, os, version, channel, install, last: "2026-11-28T15:00:00Z" });

test("versão compara por número, não por texto", () => {
  assert.equal(compareVersions("0.1.0.9", "0.1.0.10"), -1);
  assert.equal(compareVersions("0.1.0.12300", "0.1.0.12300"), 0);
  assert.equal(compareVersions("0.2", "0.1.9.9"), 1);
});

test("atrás conta as publicadas mais novas no canal e no sistema dela", () => {
  assert.deepEqual(standing(m("0.1.0.12300"), published), { kind: "current" });
  assert.equal(standingText(standing(m("0.1.0.12266"), published)), "2 versões atrás");
  assert.equal(standingText(standing(m("0.1.0.12296"), published)), "1 versão atrás");
  assert.equal(standingText(standing(m("0.1.0.12029", "windows"), published)), "2 versões atrás");
  // Entre duas publicadas: só as mais novas que ela contam.
  assert.equal(standingText(standing(m("0.1.0.12260"), published)), "3 versões atrás");
});

test("mais velha que todas as guardadas é N ou mais", () => {
  assert.deepEqual(standing(m("0.1.0.12238"), published), { kind: "behind", behind: 5, orMore: true });
  assert.equal(standingText(standing(m("0.1.0.12238"), published)), "5 ou mais versões atrás");
  assert.equal(standingText(standing(m("0.1.0.11000", "linux", "beta"), published)), "1 ou mais versões atrás");
});

test("checkout vem do install, não do canal; outro canal e sistema sem publicação ficam fora", () => {
  assert.deepEqual(standing(m("0.1.0.12238", "linux", "latest", "m", "checkout"), published), { kind: "checkout" });
  assert.deepEqual(standing(m("0.1.0.1", "linux", "other"), published), { kind: "other" });
  assert.equal(standingText(standing(m("0.1.0.1", "linux", "other"), published)), "outro canal");
  assert.deepEqual(standing(m("0.1.0.1", "macos"), published), { kind: "unknown" });
});

test("canal que o receptor ainda não leu não vira sem versão publicada", () => {
  const onlyLatest: Published = { channels: { latest: published.channels.latest }, error: "beta: http status: 403" };
  assert.deepEqual(standing(m("0.1.0.12244", "linux", "beta"), onlyLatest), { kind: "unread" });
  assert.equal(standingText({ kind: "unread" }), "canal não lido");
});

test("as mais atrasadas primeiro e a contagem por faixa", () => {
  const v = versionView([m("0.1.0.12300", "linux", "latest", "a"), m("0.1.0.12266", "linux", "latest", "b"),
    m("0.1.0.12238", "linux", "latest", "c"), m("0.1.0.11980", "windows", "latest", "d"), m("0.1.0.1", "linux", "latest", "e", "checkout")], published);
  assert.deepEqual(v.rows.map(r => r.machine.label), ["c", "d", "b", "a", "e"]);
  assert.deepEqual([v.ok, v.near, v.far, v.outside], [1, 1, 2, 1]);
  assert.deepEqual(v.latest.map(l => [l.channel, l.os, l.version]), [["latest", "linux", "0.1.0.12300"], ["latest", "windows", "0.1.0.12251"]]);
});
