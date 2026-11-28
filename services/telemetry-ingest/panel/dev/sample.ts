// Dados de exemplo para o modo dev e para os prints: mesma forma de /panel/data e /panel/trail.
import type { Dims, FailureRecord, LogLine, Summary, Trail, TrailDay } from "../src/lib/types";

let seed = 7;
const rnd = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
const pick = <T,>(xs: T[]) => xs[Math.floor(rnd() * xs.length)];
const int = (a: number, b: number) => a + Math.floor(rnd() * (b - a + 1));
const hex = (n: number) => Array.from({ length: n }, () => "0123456789abcdef"[int(0, 15)]).join("");

const DAYS = 90;
const today = () => { const d = new Date(); d.setUTCHours(0, 0, 0, 0); return d; };
const dayOf = (i: number) => new Date(today().getTime() - i * 86400000).toISOString().slice(0, 10);

const MACHINES = Array.from({ length: 48 }, (_, i) => ({
  id: hex(32), os: i % 5 === 0 ? "windows" : i % 7 === 0 ? "macos" : "linux",
  version: i % 4 === 0 ? "0.44.1" : i % 3 === 0 ? "0.44.0" : "0.45.0", channel: i % 6 === 0 ? "beta" : "latest",
}));

type Row = { day: string; event: string; dims: Dims; bucket: string | null; count: number; machines: number };
const rows: Row[] = [];
const BUCKETS = ["lt100ms", "lt300ms", "lt1s", "lt3s", "lt10s", "lt30s", "ge30s"];
const PROVIDERS: [string, number][] = [["claude", 0.52], ["codex", 0.21], ["pi", 0.11], ["kimi", 0.09], ["omp", 0.05], ["antigravity", 0.02]];
const MODELS: Record<string, string[]> = { claude: ["claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5-2"], codex: ["gpt-6.1-sol", "gpt-6-astra"],
  pi: ["k3-256k", "engine"], kimi: ["kimi-k3"], omp: ["engine"], antigravity: ["other"] };
const active = (i: number) => Math.round(26 + 10 * Math.sin(i / 9) + (i % 7 === 0 || i % 7 === 6 ? -6 : 0) + (i < 20 ? 6 : 0));

const add = (day: string, event: string, dims: Dims, count: number, machines: number, bucket: string | null = null) => {
  if (count > 0) rows.push({ day, event, dims, bucket, count, machines: Math.max(1, Math.min(machines, count)) });
};
const spread = (day: string, event: string, dims: Dims, total: number, m: number, peak: number) => {
  BUCKETS.forEach((b, j) => add(day, event, dims, Math.round(total * Math.exp(-((j - peak) ** 2) / 2.2) / 2.4), m, b));
};

const daily: { day: string; machines: number }[] = [];
for (let i = DAYS - 1; i >= 0; i--) {
  const day = dayOf(i), m = active(i);
  daily.push({ day, machines: m });
  const grow = 1 + (DAYS - i) / 140;
  for (const [p, share] of PROVIDERS) {
    const msgs = Math.round(m * 9 * share * grow * (0.8 + rnd() * 0.4));
    add(day, "message_sent", { provider: p, surface: "terminal" }, Math.round(msgs * 0.62), Math.ceil(m * share * 1.6));
    add(day, "message_sent", { provider: p, surface: "headless" }, Math.round(msgs * 0.38), Math.ceil(m * share));
    add(day, "turn_finished", { provider: p }, Math.round(msgs * 0.94), Math.ceil(m * share * 1.6));
    for (const model of MODELS[p]) {
      const opened = Math.round(m * share * 1.1 * (0.6 + rnd()));
      const e = p === "claude" ? pick(["high", "xhigh", "max", "medium"]) : p === "codex" ? pick(["medium", "high", "xhigh"]) : pick(["", "low", "medium"]);
      spread(day, "session_opened", { provider: p, model, effort: e, surface: rnd() > 0.4 ? "terminal" : "headless", origin: rnd() > 0.2 ? "person" : "session" },
        opened, Math.ceil(m * share), 2);
    }
  }
  for (const [c, f] of [["native", 0.48], ["ios", 0.28], ["android", 0.07], ["pwa", 0.17]] as [string, number][])
    add(day, "app_visible_minutes", { client: c }, Math.round(m * 95 * f * (0.8 + rnd() * 0.4)), Math.ceil(m * f * 1.4));
  for (const [feat, w] of [["search", 9], ["session.answer", 14], ["session.interrupt", 6], ["session.rename", 3], ["session.pair", 4], ["ask_history", 5],
    ["session.queue", 7], ["session.browser", 5], ["session.account", 2], ["session.model", 3], ["broadcast", 1], ["vault.use", 2], ["diag_report", 0.4],
    ["session.reopen", 2], ["session.close", 8], ["archive.resume", 1.2]] as [string, number][])
    add(day, "feature_used", { feature: feat }, Math.round(m * w * 0.3 * (0.6 + rnd() * 0.8)), Math.ceil(m * 0.5));
  for (const [tool, w, err] of [["browser", 22, 0.04], ["send", 14, 0.01], ["html_render", 9, 0.02], ["sessions", 8, 0], ["new_session", 4, 0.06],
    ["browser_goal", 3, 0.12], ["contract", 2, 0], ["browser_open", 6, 0.03], ["vault_fill", 1, 0.05], ["mission", 1.5, 0]] as [string, number, number][]) {
    const n = Math.round(m * w * 0.4 * (0.6 + rnd() * 0.8));
    add(day, "mcp_tool", { tool }, Math.round(n * (1 - err)), Math.ceil(m * 0.4));
    add(day, "mcp_tool", { tool, error: "true" }, Math.round(n * err), Math.ceil(m * 0.1));
  }
  spread(day, "chat_open", { client: "native" }, m * 30, m, 1);
  spread(day, "chat_open", { client: "ios" }, m * 14, m, 2);
  spread(day, "dictation", { route: "sessao" }, m * 3, m, 3);
  spread(day, "voice_call", {}, Math.round(m * 0.4), m, 5);
  spread(day, "client_route", { kind: "tailscale", client: "ios" }, m * 2, m, 2);
  spread(day, "client_route", { kind: "lan", client: "native" }, m * 3, m, 0);
  spread(day, "server_startup", { kind: "update" }, Math.round(m * 0.3), m, 3);
  add(day, "peers", { kind: "tailscale" }, m, m);
  add(day, "peers", { kind: "lan" }, Math.round(m * 0.6), m);
  add(day, "server_exit", { kind: "update" }, int(0, 4), 3);
  add(day, "server_exit", { kind: "crash" }, int(0, 1), 1);
  add(day, "server_exit", { kind: "requested" }, int(0, 3), 3);
  add(day, "problem_shown", { code: pick(["limit_switch_login_lost", "terminal_died", "session_kill_unconfirmed"]) }, int(0, 4), 3);
  add(day, "session_open_failed", { provider: pick(["claude", "codex", "kimi"]), code: pick(["erro_pasta", "motor_invalido", "conta_sem_login"]) }, int(0, 5), 3);
  add(day, "send_failed", { code: pick(["runtime_unhealthy", "codex_composer_busy"]) }, int(0, 3), 2);
  add(day, "session_fallen", { cause: pick(["terminal_out_of_memory", "terminal_died"]) }, i % 5 === 0 ? int(1, 3) : 0, 2);
  add(day, "client_fatal", { client: pick(["ios", "native", "android"]) }, i % 9 === 0 ? 1 : 0, 1);
  add(day, "limit_switch", { ok: rnd() > 0.15 ? "true" : "false" }, int(0, 3), 2);
  add(day, "update_finished", { from: "0.44.1", to: "0.45.0", result: rnd() > 0.1 ? "updated" : "failed" }, i < 12 ? int(1, 5) : 0, 5);
  add(day, "update_step", { step: pick(["download", "swap", "restart"]), ok: "true" }, i < 12 ? int(2, 8) : 0, 5);
  add(day, "update_failed_code", { code: "update_health_timeout" }, i === 4 || i === 9 ? 1 : 0, 1);
  if (i === 3) add(day, "diary_dropped", { part: "trail" }, 120, 1);
}
const SETTINGS: [string, [string, number][]][] = [
  ["theme", [["dark", 30], ["auto", 11], ["desktop", 6], ["light", 3]]], ["palette", [["classic", 33], ["neutral", 12]]],
  ["text_size", [["normal", 36], ["large", 8], ["small", 4]]], ["tool_look", [["chips", 24], ["tree", 14], ["terminal", 8]]],
  ["delivery", [["queue", 22], ["direct", 16], ["jev", 6]]], ["navigation", [["sidebar", 21], ["tabs", 12], ["bottom_tabs", 5]]],
  ["language", [["pt", 38], ["en", 9]]], ["default_provider", [["claude", 29], ["codex", 10], ["pi", 4], ["kimi", 3]]],
  ["default_effort", [["high", 18], ["xhigh", 14], ["max", 7], ["medium", 6]]], ["connect", [["on", 14], ["off", 32]]],
  ["computer_use", [["on", 9], ["off", 37]]], ["whatsapp", [["installed", 5], ["no", 41]]], ["conversation_beta", [["on", 12], ["off", 34]]],
  ["dictation_provider", [["groq", 13], ["openai", 9], ["whisper_cpp", 4], ["none", 20]]],
];

const RECORDS = ((): Omit<FailureRecord, "daily" | "count" | "first" | "last">[] => [
  ["error", "sessao.abrir", "erro_pasta", "abrir", null, null, "A pasta /home/x/projeto não existe mais nesta máquina."],
  ["error", "browser.agir", "erro_navegador_pausado", null, "cdp", "browser_click", "o usuario pausou o navegador"],
  ["warn", "fila.entregar", "queue_redelivered", "confirmar", null, null, "entrega confirmada depois de 2 leituras"],
  ["error", "atualizacao.reiniciar", "update_health_timeout", "reiniciar", "timeout", null, "a versão nova não respondeu à saúde em 90 s"],
  ["warn", "lista.fatos", "state_facts_unavailable", null, "io", null, null],
  ["error", "codex.composer", "codex_composer_busy", "digitar", null, null, "o campo do Codex não estava vazio"],
  ["warn", "ditado.organizar", "dictation_org_failed", "organizar", "http_429", null, "o provedor recusou por limite; ficou o texto cru"],
  ["error", "terminal.ws", "terminal_origin_blocked", "abrir", null, null, null],
  ["warn", "peer.conferir", "peer_unreachable", null, "timeout", null, "notebook fora do ar por 4 s"],
  ["error", "cofre.preencher", "vault_entry_missing", "preencher", null, "vault_fill", "entrada github removida"],
  ["warn", "app.voltar", "resume_stale_list", null, null, null, null],
  ["error", "sessao.renomear", "rename_target_taken", "reservar", null, null, "já existe uma sessão com esse nome"],
].map(([level, event, codigo, etapa, erro_tipo, action, detalhe], k) => ({
  level: level as "error" | "warn", event: event as string, codigo, etapa, erro_tipo, action,
  machines: [9, 6, 14, 3, 11, 4, 5, 2, 8, 1, 7, 2][k], versions: k % 3 ? ["0.45.0"] : ["0.44.1", "0.45.0"], os: k % 4 ? ["linux"] : ["linux", "windows"],
  sample_at: null,
  fields: { codigo, etapa, erro_tipo, action, detalhe, tela: k % 2 ? "chat" : "lista", ms: k % 3 === 0 ? String(int(80, 9000)) : undefined, provider: k % 2 ? "claude" : "codex" },
  trail: k % 5 === 4 ? [] : ["app.visivel", "lista.abrir", pick(["chat.abrir", "busca.abrir", "sessao.nova"]), pick(["mensagem.enviar", "pergunta.responder"]), "chat.rolar"].slice(0, int(2, 5)),
})))();

const LOGS: [string, string, string][] = [
  ["error", "hangar_server::runtime::claude", "cano fechou antes do fim do turno (exit 137)"],
  ["warn", "hangar_server::list::bridge", "lista sem fatos do plugin por 3 s; mantida a última rodada boa"],
  ["error", "hangar_server::update::http", "download da versão 0.45.0 falhou: connection reset by peer"],
  ["warn", "hangar_server::groups::outbox", "máquina dona do grupo fora do ar; recado na fila"],
  ["warn", "hangar_server::cota", "fonte de cota levou 429; espera 10 min"],
  ["error", "hangar_server::share", "funnel não confirmou o endereço público em 30 s"],
  ["warn", "hangar_transcript::codex", "linha desconhecida no rollout ignorada (type=turn_context_v3)"],
  ["warn", "hangar_server::keyring", "chaveiro login trancado depois do boot"],
  ["error", "hangar_server::mcp", "identidade do chamador ambígua: dois panes com a mesma chave"],
  ["warn", "hangar_dream", "consolidação adiada: trava .consolidate-lock de outra máquina"],
];

export function summary(days: number): Summary {
  const from = dayOf(days - 1);
  const inP = rows.filter(r => r.day >= from);
  const key = (...p: unknown[]) => JSON.stringify(p);
  const group = <T,>(xs: Row[], k: (r: Row) => string, make: (r: Row) => T, fold: (acc: T, r: Row) => void) => {
    const m = new Map<string, T>();
    for (const r of xs) { const kk = k(r); if (!m.has(kk)) m.set(kk, make(r)); fold(m.get(kk)!, r); }
    return [...m.values()];
  };
  const by_dims = group(inP, r => key(r.event, r.dims), r => ({ event: r.event, dims: r.dims, count: 0, machines: 0 }),
    (a, r) => { a.count += r.count; a.machines = Math.min(48, Math.max(a.machines, r.machines) + 1); });
  const settingsDays = Math.min(days, 7);
  for (const [setting, vals] of SETTINGS) for (const [value, m] of vals) {
    by_dims.push({ event: "settings", dims: { setting, value, client: "server" }, count: m * settingsDays, machines: m });
  }
  by_dims.push({ event: "settings", dims: { setting: "theme", value: "dark", client: "ios" }, count: 60, machines: 17 },
    { event: "settings", dims: { setting: "theme", value: "auto", client: "ios" }, count: 30, machines: 9 });
  const by_bucket = group(inP.filter(r => r.bucket), r => key(r.event, r.bucket), r => ({ event: r.event, bucket: r.bucket!, count: 0, machines: 0 }),
    (a, r) => { a.count += r.count; a.machines = Math.min(48, Math.max(a.machines, r.machines) + 1); });
  const trend = group(inP, r => key(r.day, r.event, r.dims), r => ({ day: r.day, event: r.event, dims: r.dims, count: 0 }), (a, r) => { a.count += r.count; });
  const counters = group(inP, r => key(r.event, r.dims, r.bucket), r => ({ event: r.event, dims: r.dims, bucket: r.bucket, count: 0, machines: 0 }),
    (a, r) => { a.count += r.count; a.machines = Math.min(48, Math.max(a.machines, r.machines) + 1); });
  const records: FailureRecord[] = RECORDS.map((r, k) => {
    const d: { day: string; count: number }[] = [];
    for (let i = days - 1; i >= 0; i--) {
      const n = (i + k) % (k % 3 + 2) === 0 ? int(1, 3 + (k % 4) * 2) : 0;
      if (n && !(k === 3 && i > 14)) d.push({ day: dayOf(i), count: n });
    }
    const count = d.reduce((s, x) => s + x.count, 0) || 1;
    const first = (d[0]?.day ?? dayOf(0)) + " 08:" + String(10 + k).padStart(2, "0") + ":03";
    const last = (d.at(-1)?.day ?? dayOf(0)) + " " + String(9 + (k % 12)).padStart(2, "0") + ":4" + (k % 10) + ":17";
    return { ...r, count, daily: d, first, last, sample_at: last, fields: Object.fromEntries(Object.entries(r.fields ?? {}).filter(([, v]) => typeof v === "string")) };
  }).sort((a, b) => b.machines - a.machines || b.count - a.count);
  const logs: LogLine[] = LOGS.map(([level, target, message], k) => ({ level: level as "error" | "warn", target, message,
    count: [412, 96, 33, 210, 58, 7, 1204, 19, 4, 61][k], machines: [12, 9, 4, 6, 10, 2, 21, 5, 1, 3][k],
    first_day: dayOf(Math.min(days - 1, 3 + k * 2)), last_day: dayOf(k % 3), versions: k % 2 ? ["0.45.0"] : ["0.44.1", "0.45.0"] }));
  const steps = ["app.visivel", "lista.abrir", "chat.abrir", "mensagem.enviar", "pergunta.responder", "busca.abrir", "sessao.nova", "chat.rolar", "app.oculto", "navegador.abrir"];
  const transitions = steps.flatMap((a, i) => steps.filter((b, j) => b !== a && (j === i + 1 || (i * 3 + j) % 7 === 0)).map(b => ({ from: a, to: b,
    count: Math.round((900 - i * 70 - steps.indexOf(b) * 20) * days / 30), machines: Math.max(2, 30 - i * 2) })))
    .sort((x, y) => y.count - x.count).slice(0, 50);
  const trail_days: TrailDay[] = [];
  for (let i = 0; i < Math.min(days, 10); i++) for (const m of MACHINES.slice(0, 8 - (i % 3))) trail_days.push({ machine: m.id, label: m.id.slice(0, 8),
    day: dayOf(i), os: m.os, version: m.version, steps: 80 + ((i * 37 + m.id.charCodeAt(0)) % 300), problems: (i + m.id.charCodeAt(1)) % 4 });
  const env = ["version", "os", "channel"].flatMap(k => {
    const c = new Map<string, number>();
    for (const m of MACHINES.slice(0, 44)) c.set((m as Record<string, string>)[k], (c.get((m as Record<string, string>)[k]) ?? 0) + 1);
    return [...c].sort((a, b) => b[1] - a[1]).map(([value, machines]) => ({ key: k, value, machines }));
  });
  const p = daily.filter(d => d.day >= from);
  return {
    records, records_total: records.length, transitions, transitions_total: transitions.length + 18, trail_days, trail_days_total: trail_days.length,
    refusals: { declined: days >= 30 ? 9 : 3, revoked: days >= 30 ? 2 : 1 }, days,
    active: { machines_7d: 37, machines_30d: 44, owners_7d: 26, owners_30d: 31 },
    period: { batches: p.reduce((s, d) => s + d.machines, 0), machines: Math.min(44, 20 + days), partial_batches: 6 },
    daily: p, env, per_owner: [{ machines: 1, owners: 22 }, { machines: 2, owners: 6 }, { machines: 3, owners: 2 }, { machines: 4, owners: 1 }],
    by_dims, by_bucket, logs, trend, counters,
  };
}

export function trail(machine: string, day: string): Trail {
  const steps = [];
  let h = 8 * 3600 + (machine.charCodeAt(2) % 40) * 60;
  const flow = ["app.visivel", "lista.abrir", "chat.abrir", "mensagem.enviar", "chat.rolar", "pergunta.responder", "mensagem.enviar", "busca.abrir",
    "sessao.nova", "sessao.abrir", "navegador.abrir", "browser.agir", "chat.abrir", "mensagem.enviar", "app.oculto"];
  for (let r = 0; r < 4; r++) for (const event of flow) {
    h += int(4, 600);
    const t = new Date(h * 1000).toISOString().slice(11, 19);
    const level = event === "browser.agir" && r === 1 ? "error" : event === "sessao.abrir" && r === 2 ? "warn" : "ok";
    steps.push({ t, event, level, screen: event.startsWith("chat") || event.startsWith("mensagem") ? "chat" : event.startsWith("lista") ? "lista" : null,
      count: event === "chat.rolar" ? int(2, 9) : 1 });
  }
  return { machine, day, partial: day === dayOf(0), steps };
}

export function bump() {
  const day = dayOf(0);
  add(day, "message_sent", { provider: pick(["claude", "codex", "pi"]), surface: "terminal" }, int(3, 12), 3);
  daily[daily.length - 1].machines += rnd() > 0.7 ? 1 : 0;
}
