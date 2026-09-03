// Contrato: a copia da extensao que roda DENTRO de um subagente do pi-subagents nao publica NADA.
//   node scripts/test-pi-hangar-state.mjs
//
// O subagente e OUTRO PROCESSO com o MESMO TMUX_PANE herdado (execution.ts:463, `spawnEnv =
// {...process.env, ...}`) e o arquivo de sessao do fork via `--session` (pi-args.ts:519). O sinal
// que atravessa o spawn e PI_SUBAGENT_DEPTH: o pai nao tem a var (vale 0), o filho recebe "1"
// (getSubagentDepthEnv, execution.ts:463 / subagent-runner.ts:541). O fork em si (fork-context.ts)
// so escreve o arquivo no disco — nao emite session_start no pai —, entao pai e filho nunca
// dividem evento; a trava por sessao do 24f1b75 comparava sessoes DENTRO de um processo e nao via
// o filho. Aqui os dois processos sao simulados com duas instancias do modulo (import com query
// string): uma sem a var (o usuario), outra com PI_SUBAGENT_DEPTH=1 (o subagente).
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const cfg = fs.mkdtempSync(path.join(os.tmpdir(), "hangar-state-test-"));
process.env.CLAUDE_CONFIG_DIR = cfg;
process.env.TMUX_PANE = "%999";
// HERDADA do pane em que o teste roda, e no Windows ela SEMPRE existe: `paneKey()` prefere
// `PSMUX_SESSION` (o pane e `%1` em toda sessao do psmux), entao o bilhete saia com o nome da
// sessao de QUEM RODA o teste e o `999.json` nunca nascia — ENOENT na primeira assercao, antes de
// medir coisa alguma. Mesma familia do `b4d97790`: um caso que so rodava num dos dois sistemas.
// Aqui o teste ESCOLHE o pane, entao a variavel herdada tem de sair; o ramo do psmux tem caso
// proprio no fim do arquivo.
delete process.env.PSMUX_SESSION;

const fakePi = () => {
  const handlers = new Map();
  const commands = new Map();
  return {
    handlers,
    commands,
    on: (evento, fn) => handlers.set(evento, fn),
    registerCommand: (nome, def) => commands.set(nome, def),
    getThinkingLevel: () => "high",
    setModel: async () => true,
    setThinkingLevel: () => {},
  };
};

const disparar = (pi, evento, ctx) => pi.handlers.get(evento)?.({}, ctx) ?? Promise.resolve();
const dispararCom = (pi, evento, e, ctx) => pi.handlers.get(evento)?.(e, ctx) ?? Promise.resolve();

const sessao = (file, modelo) => ({
  sessionManager: { getSessionFile: () => file, getSessionId: () => path.basename(file) },
  model: { provider: "p", id: modelo, name: modelo },
  modelRegistry: { getAvailable: () => [{ provider: "p", id: modelo, name: modelo }] },
});

const A = path.join(cfg, "2026-08-12T12-40-06-359Z_aaaa.jsonl");   // a conversa do usuario
const C = path.join(cfg, "2026-08-12T13-10-00-000Z_cccc.jsonl");   // /tree ou /resume do usuario

const bilhete = () => JSON.parse(fs.readFileSync(path.join(cfg, ".hangar-pi", "999.json"), "utf8")).file;
const temCatalogo = (f) => fs.existsSync(path.join(cfg, ".hangar-pi", "models", `${path.basename(f, ".jsonl")}.json`));
const temEstado = (f) => fs.existsSync(path.join(cfg, ".hangar-state", `${path.basename(f, ".jsonl")}.json`));
const temPreview = (f) => fs.existsSync(path.join(cfg, ".hangar-preview", `${path.basename(f, ".jsonl")}.json`));

// ── caso real 1: o SUBAGENTE (PI_SUBAGENT_DEPTH=1) nao publica nada ─────────────────────────────
process.env.PI_SUBAGENT_DEPTH = "1";
const piFilho = fakePi();
(await import("./pi/hangar-state.ts?filho=1")).default(piFilho);

assert.equal(piFilho.handlers.size, 0, "subagente nao registra handler nenhum");
assert.equal(piFilho.commands.size, 0, "nem comandos");

// Dispara tudo o que a extensao do pai escuta: nada pode existir apos isto.
await disparar(piFilho, "session_start", sessao(A, "sakana-namazu"));
await disparar(piFilho, "session_shutdown", sessao(A, "sakana-namazu"));
await disparar(piFilho, "agent_start", sessao(A, "sakana-namazu"));
await disparar(piFilho, "agent_settled", sessao(A, "sakana-namazu"));
await disparar(piFilho, "message_update", { message: { role: "assistant", content: [{ type: "text", text: "oi" }] } });
await disparar(piFilho, "message_end", { message: { role: "assistant" } });
await new Promise((r) => setTimeout(r, 250));   // folga pro coalesce de previa (150ms), se existisse

for (const d of [".hangar-state", ".hangar-pi", ".hangar-preview"]) {
  assert.ok(!fs.existsSync(path.join(cfg, d)), `${d} nao pode existir: subagente nao escreve nada`);
}
delete process.env.PI_SUBAGENT_DEPTH;

// ── caso real 2: a sessao do usuario publica como sempre ───────────────────────────────────────
const piPai = fakePi();
(await import("./pi/hangar-state.ts?pai=1")).default(piPai);

await disparar(piPai, "session_start", sessao(A, "gpt-5.6-sol"));
assert.equal(bilhete(), A, "a sessao do usuario publica o bilhete pane->sessao");
assert.ok(temCatalogo(A), "e o catalogo de modelos dela");

await disparar(piPai, "agent_start", sessao(A, "gpt-5.6-sol"));
assert.ok(temEstado(A), "agent_start marca working");

// message_update/message_end levam o evento no 1o argumento (e.message) e o ctx de sessao no 2o.
const msg = (role, text) => ({ message: { role, content: [{ type: "text", text }] } });
await piPai.handlers.get("message_update")(msg("assistant", "oi"), sessao(A, "gpt-5.6-sol"));
await new Promise((r) => setTimeout(r, 200));   // coalesce de 150ms da previa
assert.ok(temPreview(A), "previa ao vivo e publicada");

// Troca legitima (/new, /fork, /resume, /tree): shutdown da atual, start da nova -> o bilhete
// acompanha a sessao do processo (teardownCurrent, agent-session-runtime.js:102-113).
await disparar(piPai, "session_shutdown", sessao(A, "gpt-5.6-sol"));
await disparar(piPai, "session_start", sessao(C, "outro"));
assert.equal(bilhete(), C, "troca legitima de sessao atualiza o bilhete");
assert.ok(temCatalogo(C), "e o catalogo da sessao nova");

// ── caso real 3: no psmux a chave do bilhete e o NOME DA SESSAO, nao o pane ────────────────────
// O pane so e unico no tmux. No psmux ele e numerado por SESSAO — quatro sessoes vivas, TODAS com
// `TMUX_PANE=%1` (medido 21/08/2026) —, e com o pane como chave a segunda sessao Pi sobrescrevia o
// bilhete da primeira: as duas passavam a apontar pro mesmo transcript e uma abria a conversa da
// outra. `paneKey()` prefere `PSMUX_SESSION` por isso, e sanitiza porque ali a chave vira NOME DE
// ARQUIVO (a chave da linha, que nao vira, vai crua — ver `chaveDaLinha`).
process.env.PSMUX_SESSION = "pi teste/2";
const piPsmux = fakePi();
(await import("./pi/hangar-state.ts?psmux=1")).default(piPsmux);
await disparar(piPsmux, "session_start", sessao(A, "gpt-5.6-sol"));
const noPsmux = path.join(cfg, ".hangar-pi", "pi-teste-2.json");
assert.ok(fs.existsSync(noPsmux), "no psmux o bilhete e do NOME da sessao, sanitizado pra arquivo");
assert.equal(JSON.parse(fs.readFileSync(noPsmux, "utf8")).file, A);
delete process.env.PSMUX_SESSION;

// ── caso real 3 (omp): subagente no MESMO processo, arquivo <stem>/<Nome>.jsonl, sem PI_SUBAGENT_DEPTH ──
{
  const piOmp = fakePi();
  (await import("./pi/hangar-state.ts?omp=1")).default(piOmp);
  const M = path.join(cfg, "2026-09-03T16-17-19-304Z_01a0680f-77c8-7397-805f-c8651e6051f1.jsonl");
  const SUB = path.join(cfg, "2026-09-03T16-17-19-304Z_01a0680f-77c8-7397-805f-c8651e6051f1", "ContarLinhas.jsonl");
  const estado = (f) => JSON.parse(fs.readFileSync(path.join(cfg, ".hangar-state", `${path.basename(f, ".jsonl")}.json`), "utf8")).state;
  await disparar(piOmp, "session_start", sessao(M, "main-model"));
  assert.equal(bilhete(), M);
  await disparar(piOmp, "agent_start", sessao(M, "main-model"));
  await disparar(piOmp, "session_start", sessao(SUB, "sub-model"));
  await disparar(piOmp, "agent_start", sessao(SUB, "sub-model"));
  await disparar(piOmp, "agent_end", sessao(SUB, "sub-model"));
  await disparar(piOmp, "session_shutdown", sessao(SUB, "sub-model"));
  assert.equal(bilhete(), M, "subagente do omp nao reescreve o bilhete");
  assert.ok(!temEstado(SUB), "subagente nao publica estado");
  assert.equal(estado(M), "working", "o agent_end do subagente nao fecha o turno do main");
  // a sessao do pane continua viva depois do shutdown do subagente: a previa do main ainda sai
  await dispararCom(piOmp, "message_update", { message: { role: "assistant", content: [{ type: "text", text: "oi" }] } }, sessao(M, "main-model"));
  await new Promise((r) => setTimeout(r, 250));
  assert.ok(temPreview(M), "shutdown do subagente nao desliga a sessao do pane");
  await disparar(piOmp, "agent_end", sessao(M, "main-model"));
  assert.equal(estado(M), "idle");
  fs.rmSync(path.join(cfg, ".hangar-pi", "models"), { recursive: true, force: true });
  await disparar(piOmp, "model_changed", sessao(M, "main-model"));
  assert.ok(temCatalogo(M), "model_changed republica o catalogo");
}

fs.rmSync(cfg, { recursive: true, force: true });
console.log("ok: subagente nao publica nada; sessao do usuario publica; psmux chaveia pelo nome (hangar-state.ts)");
process.exit(0);   // conectar() agendou retry do WebSocket; nada a esperar aqui
