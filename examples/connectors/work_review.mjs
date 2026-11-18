// Conector de exemplo de `work_review/v1` em JavaScript, sem dependências: um ticket em cada
// estado da revisão (rodando, esperando você, concluída). As imagens são PNGs gerados em memória.
// Uso: o manifesto ao lado (`work_review.json`) roda `node ./work_review.mjs`.
import { readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { deflateSync } from "node:zlib";

// Cada comando do CLI abre um processo novo: a decisão fica num arquivo para valer no seguinte.
const STATE_FILE = process.env.WORK_REVIEW_DEMO_STATE ?? join(tmpdir(), "hangar-work-review-demo.json");
const W = 800;
const H = 450;

const TICKETS = [
  { id: "rev-running", key: "REV-1", title: "Tela de clientes", status: { id: "doing", name: "Fazendo", category: "in_progress" } },
  { id: "rev-waiting", key: "REV-2", title: "Controle bancário", status: { id: "review", name: "Em revisão", category: "in_progress" } },
  { id: "rev-done", key: "REV-3", title: "Relatório de caixa", status: { id: "done", name: "Feito", category: "done" } },
];

const summary = (ticket) => ({
  ...ticket, url: `https://exemplo.invalid/tickets/${ticket.id}`, assignee: null, project: null, updated_at: "2026-11-18T09:00:00Z",
});

const STEPS = ["Prints", "Comparação", "Correções", "Sua decisão", "Pronto"];
const BOX = [120, 90, 360, 180];
const side = (label, value, asset) => ({ label, value, asset, width: W, height: H, box: BOX });

// Cor do fundo e do trecho em destaque de cada imagem.
const ASSETS = {
  "mock.png:1": [[236, 240, 246], [70, 110, 200]],
  "tela.png:1": [[236, 240, 246], [200, 120, 60]],
  "rodape.png:1": [[250, 246, 236], [150, 90, 180]],
  "antes-1.png:1": [[240, 236, 236], [190, 60, 60]],
  "depois-1.png:1": [[236, 244, 238], [60, 160, 90]],
  "antes-2.png:1": [[240, 236, 236], [190, 60, 60]],
  "depois-2.png:1": [[236, 244, 238], [60, 160, 90]],
};

const RESOLVED = {
  label: "Corrigidas pelo agente", count: 15,
  items: [
    { title: "Cabeçalho do mock", before: "antes-1.png:1", after: "depois-1.png:1" },
    { title: "Espaçamento da grade", before: "antes-2.png:1", after: "depois-2.png:1" },
  ],
};

const KEPT = { label: "Mantidas pelo agente, com motivo", items: [{ title: "Selo Open Finance", reason: "dado sem fonte no backend" }] };

const WAITING_DECISIONS = [
  {
    id: "47", tag: "Diferença 47", title: "Janela da Movimentação menor que no mock",
    reason_label: "Por que o agente não igualou", reason: "padrão do kit: a tela usa a área no tamanho padrão",
    left: side("Mock", "~1110 px", "mock.png:1"), right: side("Tela", "~830 px", "tela.png:1"),
    choices: [
      { id: "igualar", label: "Igualar ao mock", hint: "o agente corrige a tela" },
      { id: "manter", label: "Manter como está", hint: "fica registrado o porquê", requires_reason: true,
        default_reason: "padrão do kit: a área fica no tamanho padrão", reason_prompt: "Por que manter? Vai para o relatório" },
      { id: "adiar", label: "Decidir depois", hint: "vira uma pendência no ticket" },
    ],
  },
  {
    id: "48", tag: "Diferença 48", title: "Rodapé sem a legenda de saldo",
    left: side("Mock", "com legenda", "rodape.png:1"),
    choices: [
      { id: "igualar", label: "Incluir a legenda" },
      { id: "manter", label: "Deixar sem", requires_reason: true },
    ],
  },
  {
    id: "49", tag: "Diferença 49", title: "Botão Exportar fora do mock",
    reason: "o botão existe no sistema atual e não aparece no mock",
    choices: [
      { id: "remover", label: "Remover o botão" },
      { id: "manter", label: "Manter o botão", requires_reason: true, default_reason: "usado no fechamento do mês" },
    ],
  },
];

function load() {
  try { return JSON.parse(readFileSync(STATE_FILE, "utf8")); } catch { return {}; }
}

function save(state) { writeFileSync(STATE_FILE, JSON.stringify(state)); }

function fail(code, message) {
  const error = new Error(message);
  error.code = code;
  throw error;
}

function reviewOf(ticketId, { count = false } = {}) {
  if (ticketId === "rev-running") {
    return { id: "rodada-clientes-1", title: "Comparação com o mock", subject: "Tela de clientes", updated_at: "2026-11-18T10:05:00-03:00",
      state: "running", steps: { labels: STEPS, current: 1 }, progress: { label: "Capturando os estados", done: 9, total: 16 },
      headline: { value: "9", text: "estados capturados de 16" }, badge: { text: "mock: comparando", tone: "info" } };
  }
  if (ticketId === "rev-done") {
    return { id: "rodada-caixa-3", title: "Comparação com o mock", subject: "Relatório de caixa", updated_at: "2026-11-17T18:00:00-03:00",
      state: "done", steps: { labels: STEPS, current: STEPS.length }, headline: { value: "15", text: "diferenças corrigidas", detail: "1 mantida com motivo." },
      badge: { text: "mock: pronto", tone: "success" }, resolved: RESOLVED, kept: KEPT,
      links: [{ label: "Relatório completo", url: "http://127.0.0.1:47613/c/caixa/", local: true }] };
  }
  if (ticketId !== "rev-waiting") return TICKETS.some((t) => t.id === ticketId) ? null : fail("not_found", "ticket não encontrado");
  const id = "rodada-bancario-2";
  const common = { id, title: "Comparação com o mock", subject: "Controle bancário" };
  const stored = load()[ticketId] ?? { answers: {}, reads: 0, at: "2026-11-18T10:00:00-03:00" };
  const answers = Object.entries(stored.answers);
  // Lotes: as decisões já guardadas saem do get, e a rodada continua a mesma até a última.
  const pending = WAITING_DECISIONS.filter((d) => !stored.answers[d.id]);
  if (pending.length) {
    return { ...common, updated_at: stored.at, state: "waiting", steps: { labels: STEPS, current: 3 },
      headline: { value: String(pending.length), text: "diferenças esperam a sua decisão", detail: "O agente já corrigiu 15 sozinho." },
      badge: { text: `mock: ${pending.length} para você`, tone: "warning" }, decisions: pending, resolved: RESOLVED, kept: KEPT };
  }
  // A primeira leitura depois da última decisão mostra o agente aplicando; as seguintes, o fim.
  if (count) { stored.reads += 1; save({ ...load(), [ticketId]: stored }); }
  if (stored.reads <= 1) {
    return { ...common, updated_at: stored.at, state: "applying", steps: { labels: STEPS, current: 4 },
      headline: { value: String(answers.length), text: "decisões sendo aplicadas pelo agente" }, badge: { text: "mock: aplicando", tone: "info" } };
  }
  const kept = answers.filter(([, a]) => a.reason).map(([id, a]) => ({ title: `Diferença ${id}`, reason: a.reason }));
  return { ...common, updated_at: stored.at, state: "done", steps: { labels: STEPS, current: STEPS.length },
    headline: { value: String(15 + answers.length), text: "diferenças resolvidas" }, badge: { text: "mock: pronto", tone: "success" },
    resolved: { ...RESOLVED, count: 15 + answers.length - kept.length }, kept: { ...KEPT, items: [...KEPT.items, ...kept] } };
}

function get({ ticket_id }) {
  const ticket = TICKETS.find((item) => item.id === ticket_id) ?? fail("not_found", "ticket não encontrado");
  return { ...summary(ticket), description_markdown: `Revisão de exemplo: **${ticket.title}**.` };
}

function current(ticketId, reviewId) {
  const review = reviewOf(ticketId) ?? fail("not_found", "ticket sem revisão");
  if (review.id !== reviewId) fail("stale_review", "a revisão mudou; leia de novo");
  return review;
}

function asset({ ticket_id, review_id, asset }) {
  current(ticket_id, review_id);
  if (!ASSETS[asset]) fail("not_found", "imagem não encontrada");
  return { mime: "image/png", data: png(asset).toString("base64") };
}

function decide({ ticket_id, review_id, decisions }) {
  const review = current(ticket_id, review_id);
  if (!Array.isArray(decisions) || !decisions.length) fail("invalid_params", "nenhuma decisão no pedido");
  const ids = decisions.map((d) => d.id);
  if (new Set(ids).size !== ids.length) fail("invalid_params", "decisão repetida no pedido");
  const state = load();
  const stored = state[ticket_id] ?? { answers: {}, reads: 0, at: null };
  // Lote já guardado, com as mesmas escolhas: o app reenvia quando a resposta se perde.
  const same = (d) => stored.answers[d.id]?.choice === d.choice;
  const fresh = decisions.filter((d) => !same(d));
  if (!fresh.length) return { ok: true };
  if (review.state !== "waiting") fail("invalid_params", "a revisão não espera decisões");
  for (const answer of fresh) {
    const pending = review.decisions.find((d) => d.id === answer.id) ?? fail("invalid_params", `decisão ${answer.id} não está pendente`);
    const choice = pending.choices.find((c) => c.id === answer.choice) ?? fail("invalid_params", `escolha inexistente em ${answer.id}`);
    if (choice.requires_reason && !answer.reason?.trim()) fail("invalid_params", `a decisão ${answer.id} precisa do motivo`);
  }
  for (const answer of fresh) stored.answers[answer.id] = { choice: answer.choice, reason: answer.reason ?? null };
  stored.at = new Date().toISOString();
  state[ticket_id] = stored;
  save(state);
  return { ok: true };
}

const CRC = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(bytes) {
  let c = 0xffffffff;
  for (const byte of bytes) c = CRC[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

const PNGS = new Map();

// Fundo de uma cor e o trecho do `box` de outra: basta para ver o zoom e a moldura no painel.
function png(name) {
  if (PNGS.has(name)) return PNGS.get(name);
  const [background, highlight] = ASSETS[name];
  const [bx, by, bw, bh] = BOX;
  const raw = Buffer.alloc(H * (1 + W * 3));
  for (let y = 0; y < H; y++) {
    const row = y * (1 + W * 3);
    for (let x = 0; x < W; x++) {
      const inside = x >= bx && x < bx + bw && y >= by && y < by + bh;
      raw.set(inside ? highlight : background, row + 1 + x * 3);
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(W, 0);
  header.writeUInt32BE(H, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const image = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
  PNGS.set(name, image);
  return image;
}

const METHODS = {
  initialize: () => ({ protocol_version: 1, id: "js-review", capabilities: { work_tracking: 1, work_review: 1 } }),
  "work_tracking.list": ({ limit }) => ({ items: TICKETS.slice(0, limit).map(summary), next_cursor: null, total: TICKETS.length }),
  "work_tracking.get": get,
  "work_review.get": ({ ticket_id }) => ({ review: reviewOf(ticket_id, { count: true }) }),
  "work_review.asset": asset,
  "work_review.decide": decide,
};

const lines = createInterface({ input: process.stdin });
for await (const line of lines) {
  const request = JSON.parse(line);
  if (request.method === "shutdown") {
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: null }) + "\n");
    break;
  }
  let response;
  try {
    const method = METHODS[request.method] ?? fail("unsupported_capability", `método não oferecido: ${request.method}`);
    response = { jsonrpc: "2.0", id: request.id, result: method(request.params ?? {}) };
  } catch (error) {
    const code = error.code ?? "unavailable";
    // Diagnóstico vai ao stderr; o stdout é só do protocolo.
    console.error(`work_review.mjs: ${code}`);
    const rpc = code === "invalid_params" ? -32602 : -32000;
    response = { jsonrpc: "2.0", id: request.id, error: { code: rpc, message: error.message, data: { code } } };
  }
  process.stdout.write(JSON.stringify(response) + "\n");
}
