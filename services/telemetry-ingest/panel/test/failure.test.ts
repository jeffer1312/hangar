// Roda com `npm test`: as frases das falhas (o que aconteceu, estado da máquina, passos) a partir dos valores fechados.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { afterText, happened, skewText, stateRows, stepText } from "../src/lib/failure.ts";
import { CAUSES, CLIENT_CODE_LABEL } from "../src/lib/labels.ts";
import type { FailureRecord } from "../src/lib/types.ts";

const rec = (over: Partial<FailureRecord>): FailureRecord => ({ level: "warn", event: "health.state", codigo: null, etapa: null, erro_tipo: null,
  action: null, count: 1, machines: 1, versions: [], os: [], first: "", last: "", sample_at: null, fields: {}, trail: [], daily: [], ...over });

test("falha de saúde: a descrição da máquina e a causa em frase", () => {
  const h = happened(rec({ fields: { codigo: "terminal_observacao_falhou", causa: "capture_timeout", detalhe: "A tela do terminal da sessão não pôde ser lida." } }));
  assert.equal(h.summary, "A tela do terminal da sessão não pôde ser lida.");
  assert.equal(h.cause, "a captura da tela do terminal demorou demais");
  assert.match(h.technical, /health\.state · terminal_observacao_falhou · capture_timeout/);
});

test("causa desconhecida não vira frase, fica só o nome técnico", () => {
  const h = happened(rec({ fields: { codigo: "terminal_observacao_falhou", causa: "causa_nova" } }));
  assert.equal(h.cause, null);
  assert.match(h.technical, /causa_nova/);
  assert.equal(h.summary, "A tela do terminal da sessão não pôde ser lida.", "sem detalhe, a descrição vem do código");
});

test("ferramenta do agente e código do app", () => {
  const mcp = happened(rec({ event: "mcp.tool", action: "browser", fields: { codigo: "tool_failed", causa: "desktop_app_closed", action: "browser" } }));
  assert.equal(mcp.tool, "Navegador");
  assert.equal(mcp.cause, "o app do PC estava fechado");
  const app = happened(rec({ event: "api.sem_rede", fields: { codigo: "http_5xx" } }));
  assert.equal(app.cause, CLIENT_CODE_LABEL.http_5xx);
});

test("passos legíveis", () => {
  assert.deepEqual(stepText("tela.ver:chat"), { text: "Abriu a tela: conversa", known: true });
  assert.deepEqual(stepText("chat.opened"), { text: "Abriu o chat", known: true });
  assert.deepEqual(stepText("browser"), { text: "Chamou a ferramenta Navegador", known: true });
  assert.deepEqual(stepText("session.rename"), { text: "Pediu: renomear sessão", known: true });
  assert.deepEqual(stepText("sse.caiu:lista"), { text: "O canal ao vivo caiu (tela: lista de sessões)", known: true });
  assert.deepEqual(stepText("algo.novo"), { text: "algo.novo", known: false });
  assert.match(afterText("recovered")!, /^Recuperou sozinho/);
  assert.equal(afterText(null), null);
});

test("estado da máquina: faixas em frase e a mudança da primeira para a última vez", () => {
  const rows = stateRows(rec({ ctx_first: { mem_free: "ge40", provider: "claude", resumed: "lt5s" }, ctx_last: { mem_free: "lt5", provider: "claude" },
    batch: { version: "0.45.0", commit: "abc1234", install: "checkout", os: "linux", clock_skew_s: 130,
      machine: { os: "linux", os_name: "cachyos", os_version: "24.04", "cli.claude": "absent", ram_total: "16-32g", mux: "tmux", mux_version: "3.5a" } } }));
  const find = (label: string) => [...rows.build, ...rows.machine, ...rows.moment].find(r => r.label === label);
  assert.deepEqual([find("Memória livre")?.value, find("Memória livre")?.first], ["menos de 5% livre", "40% ou mais livre"]);
  assert.equal(find("Agente")?.first, undefined, "igual nas duas vezes não repete");
  assert.equal(find("Voltando da suspensão")?.value, "não informado", "só na primeira vez");
  assert.equal(find("Sistema")?.value, "CachyOS 24.04");
  assert.equal(find("Claude Code")?.value, "não instalado");
  assert.equal(find("Memória total")?.value, "16–32 GB");
  assert.equal(find("Terminal")?.value, "tmux 3.5a");
  assert.equal(find("Versão")?.value, "0.45.0 · commit abc1234");
  assert.equal(find("Instalação")?.value, "checkout de desenvolvimento");
  assert.equal(find("Relógio da máquina")?.value, "2 min atrasado");
  assert.equal(stateRows(rec({})).machine.length, 0, "pacote antigo sem retrato: nada inventado");
});

test("diferença do relógio", () => {
  assert.equal(skewText(20), "certo");
  assert.equal(skewText(-7200), "2 h adiantado");
  assert.match(skewText(null), /sem/);
});

// Toda causa e código fechado do servidor tem frase: a lista do painel acompanha events.rs.
test("toda causa de events.rs tem frase", () => {
  const src = readFileSync(new URL("../../../../crates/hangar-telemetry/src/events.rs", import.meta.url), "utf8");
  const quoted = (block: string) => [...block.matchAll(/"([a-z0-9_]+)"/g)].map(m => m[1]);
  const causes = [...src.matchAll(/causes: &\[([^\]]*)\]/g)].flatMap(m => quoted(m[1]));
  const list = (name: string) => quoted(src.match(new RegExp("pub const " + name + ": &\\[&str\\] = &\\[([^\\]]*)\\]"))![1]);
  const mcp = list("MCP_CAUSES"), client = list("CLIENT_CODES");
  assert.ok(causes.length > 100 && mcp.length > 50 && client.length > 10);
  assert.deepEqual([...causes, ...mcp].filter(c => !Object.hasOwn(CAUSES, c)), []);
  assert.deepEqual(client.filter(c => !Object.hasOwn(CLIENT_CODE_LABEL, c)), []);
});
