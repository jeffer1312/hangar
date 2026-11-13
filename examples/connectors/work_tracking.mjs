// Conector mínimo de `work_tracking/v1` em JavaScript, sem dependências: prova que o host não
// depende da linguagem. Tickets em memória, com IDs que não se parecem com PM.
// Uso: o manifesto ao lado (`work_tracking.json`) roda `node ./work_tracking.mjs`.
import { createInterface } from "node:readline";

const TICKETS = [
  { id: "c0ffee-01", key: "OPS/7", title: "Trocar a impressora do 2º andar", status: { id: "doing", name: "Fazendo", category: "in_progress" },
    assignee: { id: "u:42", name: "Ana" }, project: { id: "infra", name: "Infraestrutura" }, updated_at: "2026-11-12T08:30:00+01:00",
    description_markdown: "Impressora **sem toner**.\n\n- conferir o modelo\n- pedir o cartucho" },
  { id: "c0ffee-02", key: "OPS/8", title: "Renovar certificado", status: { id: "todo", name: "A fazer", category: "todo" },
    assignee: null, project: null, updated_at: null, description_markdown: "" },
  { id: "c0ffee-03", key: "OPS/9", title: "Migrar backup", status: { id: "won't", name: "Descartado", category: "canceled" },
    assignee: { id: "u:7", name: "Bruno" }, project: { id: "infra", name: "Infraestrutura" }, updated_at: "2026-10-01T00:00:00Z",
    description_markdown: "Fora do escopo." },
];

const summary = ({ description_markdown, ...rest }) => ({ ...rest, url: `https://exemplo.invalid/tickets/${encodeURIComponent(rest.id)}` });

function fail(code, message) {
  const error = new Error(message);
  error.code = code;
  throw error;
}

function list({ filter = {}, limit, cursor = null }) {
  const unknown = Object.keys(filter).filter((name) => !["query", "assignee", "state"].includes(name));
  if (unknown.length) fail("invalid_params", `filtro desconhecido: ${unknown.join(", ")}`);
  const tag = JSON.stringify([filter.query ?? null, filter.assignee ?? null, filter.state ?? null, limit]);
  let start = 0;
  if (cursor !== null) {
    const [offset, bound] = cursor.split("|");
    if (bound !== tag) fail("invalid_cursor", "cursor de outra consulta");
    start = Number(offset);
  }
  const me = "u:42";
  const matches = TICKETS.filter((ticket) =>
    (!filter.query || ticket.title.toLowerCase().includes(filter.query.toLowerCase()))
    && (!filter.assignee || ticket.assignee?.id === (filter.assignee === "me" ? me : filter.assignee))
    && (!filter.state || (filter.state === "done") === ["done", "canceled"].includes(ticket.status.category)));
  const items = matches.slice(start, start + limit).map(summary);
  const next = start + limit < matches.length ? `${start + limit}|${tag}` : null;
  return { items, next_cursor: next, total: matches.length };
}

function get({ ticket_id }) {
  const ticket = TICKETS.find((item) => item.id === ticket_id) ?? fail("not_found", "ticket não encontrado");
  return { ...summary(ticket), description_markdown: ticket.description_markdown };
}

const METHODS = {
  initialize: () => ({ protocol_version: 1, id: "js-demo", capabilities: { work_tracking: 1 } }),
  "work_tracking.list": list,
  "work_tracking.get": get,
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
    console.error(`work_tracking.mjs: ${code}`);
    response = { jsonrpc: "2.0", id: request.id, error: { code: -32000, message: error.message, data: { code } } };
  }
  process.stdout.write(JSON.stringify(response) + "\n");
}
