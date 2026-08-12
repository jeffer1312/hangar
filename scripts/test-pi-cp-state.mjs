// Contrato da trava de dono do pane em scripts/pi/cp-state.ts (node >= 22.18 roda o .ts direto).
//   node scripts/test-pi-cp-state.mjs
//
// O que ela protege: o Pi cria sessoes DENTRO DO MESMO PROCESSO que nao sao a conversa do usuario
// (a extensao pi-subagents forka a sessao pra dar contexto a cada subagente). Sem a trava, o
// session_start do fork reescrevia o bilhete pane->sessao e o catalogo de modelos, e o app mostrava
// o modelo do fork no lugar do da sessao (medido 12/08/2026 no pane %2612).
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const cfg = fs.mkdtempSync(path.join(os.tmpdir(), "cp-state-test-"));
process.env.CLAUDE_CONFIG_DIR = cfg;
process.env.TMUX_PANE = "%999";

const { default: registrar } = await import("./pi/cp-state.ts");

const handlers = new Map();
const pi = {
  on: (evento, fn) => handlers.set(evento, fn),
  registerCommand: () => {},
  getThinkingLevel: () => "high",
  setModel: async () => true,
  setThinkingLevel: () => {},
};
registrar(pi);

const disparar = (evento, ctx) => handlers.get(evento)(({}), ctx);

const sessao = (file, modelo) => ({
  sessionManager: { getSessionFile: () => file, getSessionId: () => path.basename(file) },
  model: { provider: "p", id: modelo, name: modelo },
  modelRegistry: { getAvailable: () => [{ provider: "p", id: modelo, name: modelo }] },
});

const A = "/tmp/sessoes/2026-08-12T12-40-06-359Z_aaaa.jsonl";   // a conversa do usuario
const B = "/tmp/sessoes/2026-08-12T12-58-03-419Z_bbbb.jsonl";   // fork feito pra um subagente
const C = "/tmp/sessoes/2026-08-12T13-10-00-000Z_cccc.jsonl";   // /fork ou /resume do usuario

const bilhete = () => JSON.parse(fs.readFileSync(path.join(cfg, ".claude-pocket-pi", "999.json"), "utf8")).file;
const temCatalogo = (f) => fs.existsSync(path.join(cfg, ".claude-pocket-pi", "models", `${path.basename(f, ".jsonl")}.json`));
const temEstado = (f) => fs.existsSync(path.join(cfg, ".claude-pocket-state", `${path.basename(f, ".jsonl")}.json`));

// 1. a primeira sessao do processo vira a dona do pane
await disparar("session_start", sessao(A, "gpt-5.6-sol"));
assert.equal(bilhete(), A, "a sessao do usuario tem que publicar o bilhete");
assert.ok(temCatalogo(A), "e o catalogo de modelos dela");

// 2. fork de subagente nao rouba o pane, nem publica catalogo proprio
await disparar("session_start", sessao(B, "sakana-namazu"));
await disparar("agent_start", sessao(B, "sakana-namazu"));
await disparar("message_update", { message: { role: "assistant", content: [{ type: "text", text: "oi" }] } });
assert.equal(bilhete(), A, "o fork do subagente NAO pode reescrever o bilhete");
assert.ok(!temCatalogo(B), "nem publicar o modelo dele como o da sessao");
assert.ok(!temEstado(B), "nem marcar estado por conta propria");

// 3. o fim do fork nao deixa o pane sem dono
await disparar("session_shutdown", sessao(B, "sakana-namazu"));
await disparar("session_start", sessao(C, "outro"));
assert.equal(bilhete(), A, "so a queda da DONA solta o pane");

// 4. troca de verdade (/new, /fork, /resume, /tree) derruba a dona antes -> o pane muda de mao
await disparar("session_shutdown", sessao(A, "gpt-5.6-sol"));
await disparar("session_start", sessao(C, "outro"));
assert.equal(bilhete(), C, "troca legitima de sessao tem que atualizar o bilhete");
assert.ok(temCatalogo(C), "e o catalogo da sessao nova");

fs.rmSync(cfg, { recursive: true, force: true });
console.log("ok: trava de dono do pane (cp-state.ts)");
process.exit(0);   // conectar() agendou retry do WebSocket; nada a esperar aqui
