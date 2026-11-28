// Roda com `npm test`: a conta de quantas versões cada máquina está atrás.
import { test } from "node:test";
import assert from "node:assert/strict";
import { compareVersions, standing, standingText, versionView } from "../src/lib/versions.ts";
import type { MachineLast, Published } from "../src/lib/types.ts";

const rel = (version: string, os = "linux") => ({ version, os, at: "2026-11-28T19:16:19Z" });
// Releases reais de 28/11: Linux com 5 versões guardadas, Windows com 5.
const published: Published = { error: null, channels: {
  latest: ["12251", "12256", "12266", "12296", "12300"].map(v => rel("0.1.0." + v))
    .concat(["11980", "11989", "12029", "12238", "12251"].map(v => rel("0.1.0." + v, "windows"))),
  beta: [rel("0.1.0.12244")],
} };
const m = (version: string, os = "linux", channel = "latest", label = "m"): MachineLast => ({ label, os, version, channel, last: "2026-11-28T15:00:00Z" });

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

test("checkout fica fora da conta; sistema sem publicação também", () => {
  assert.deepEqual(standing(m("0.1.0.1", "linux", "other"), published), { kind: "checkout" });
  assert.deepEqual(standing(m("0.1.0.1", "macos"), published), { kind: "unknown" });
});

test("as mais atrasadas primeiro e a contagem por faixa", () => {
  const v = versionView([m("0.1.0.12300", "linux", "latest", "a"), m("0.1.0.12266", "linux", "latest", "b"),
    m("0.1.0.12238", "linux", "latest", "c"), m("0.1.0.11980", "windows", "latest", "d"), m("dev", "linux", "other", "e")], published);
  assert.deepEqual(v.rows.map(r => r.machine.label), ["c", "d", "b", "a", "e"]);
  assert.deepEqual([v.ok, v.near, v.far, v.outside], [1, 1, 2, 1]);
  assert.deepEqual(v.latest.map(l => [l.channel, l.os, l.version]), [["latest", "linux", "0.1.0.12300"], ["latest", "windows", "0.1.0.12251"]]);
});
