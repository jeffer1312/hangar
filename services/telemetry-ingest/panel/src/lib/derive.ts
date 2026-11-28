// Tudo o que as seções desenham, calculado uma vez a partir de /panel/data.
import { AGENT_ORDER, agentKey, type AgentKey } from "./agents";
import { BAD, BUCKET_LABEL, BUCKETS, EFFORTS, EVENTS, FIELD_LABEL, FLAGS, KEYS, SETTING_GROUPS, SETTINGS, SETTING_VALUES, AGENTS, VALUES } from "./labels";
import { fmt, quantity } from "./format";
import { afterText, happened, has, stateRows, stepText } from "./failure";
import { SPEED_EVENTS } from "./speed";
import type { Dims, FailureRecord, Summary } from "./types";


export function dimsLabel(dims: Dims | null | undefined) {
  const parts: string[] = [];
  for (const [k, v] of Object.entries(dims ?? {}).sort()) {
    // Só "true"/"false" viram a frase da bandeira; outro valor aparece como veio.
    if (has(FLAGS, k) && (v === "true" || v === "false")) {
      const said = FLAGS[k][v === "true" ? 0 : 1];
      if (said) parts.push(said);
    } else if (has(VALUES, k) && has(VALUES[k], v)) parts.push(VALUES[k][v]);
    else parts.push(has(KEYS, k) ? KEYS[k] + " " + v : v);
  }
  return parts.length ? parts.join(" · ") : "todas";
}

// Série diária completa do período (dia sem pacote = 0), em UTC como os pacotes.
export function series<T>(days: number, valueOf: (day: string) => T, skipToday: boolean) {
  const end = new Date(); end.setUTCHours(0, 0, 0, 0);
  const out: { day: string; value: T }[] = [];
  for (let i = days - 1; i >= 0; i--) {
    const day = new Date(end.getTime() - i * 86400000).toISOString().slice(0, 10);
    out.push({ day, value: valueOf(day) });
  }
  return skipToday ? out.slice(0, -1) : out;
}

const bucketRank = (b: string) => { const i = BUCKETS.indexOf(b); return i < 0 ? BUCKETS.length : i; };
export const byBucket = (x: string, y: string) => bucketRank(x) - bucketRank(y);
export const bucketLabel = (b: string) => (has(BUCKET_LABEL, b) ? BUCKET_LABEL[b] : b);
const UPPER: Record<string, string> = { lt100ms: "100 ms", lt300ms: "300 ms", lt1s: "1 s", lt3s: "3 s", lt10s: "10 s", lt30s: "30 s", ge30s: "mais de 30 s" };

export function quantileBucket(buckets: Map<string, number>, q: number) {
  const order = [...buckets.keys()].sort(byBucket);
  const total = order.reduce((s, b) => s + buckets.get(b)!, 0);
  let seen = 0;
  for (const b of order) { seen += buckets.get(b)!; if (seen >= q * total) return b; }
  return order[order.length - 1];
}
// Faixas de tempo em frase: a faixa onde fica a metade das vezes e onde ficam 9 em cada 10.
export function timeSentence(buckets: Map<string, number>) {
  const total = [...buckets.values()].reduce((s, n) => s + n, 0);
  const say = (b: string) => b === "ge30s" ? "leva mais de 30 s" : has(UPPER, b) ? "fica em até " + UPPER[b] : "fica na faixa " + b;
  if (total < 5) return "Poucas medidas ainda (" + total + "): o retrato fica confiável com mais dias.";
  return "Metade das vezes " + say(quantileBucket(buckets, 0.5)) + "; 9 em cada 10 " + say(quantileBucket(buckets, 0.9)) + ".";
}

export const eventTitle = (event: string) => (has(EVENTS, event) ? EVENTS[event][0] : event);
// Falha é o evento de falha inteiro, ou a linha que diz que deu errado. Atualização que falha conta uma vez, pelo código.
export const isFailure = (event: string, dims: Dims) => BAD.has(event)
  || (event !== "update_step" && event !== "update_finished" && (dims.error === "true" || dims.ok === "false"));

export type Bar = { key: string; label: string; value: number; machines?: number };
export const effortLabel = (e: string) => (e === "" ? "sem esforço" : has(SETTING_VALUES, e) ? SETTING_VALUES[e] : e);
export const effortRank = (e: string) => { const i = EFFORTS.findIndex(([k]) => k === e); return i < 0 ? EFFORTS.length : i; };
export const effortColor = (e: string) => EFFORTS.find(([k]) => k === e)?.[1] ?? "#5d555b";

export interface EventCard { event: string; title: string; explain: string; bars: Bar[] }
export interface SpeedCard { event: string; title: string; explain: string; sentence: string; bars: Bar[]; total: number }

export function derive(data: Summary) {
  const today = new Date().toISOString().slice(0, 10);
  // Sem pacote de hoje, os gráficos param em ontem; um parcial de hoje já conta.
  const skipToday = !data.daily.some(d => d.day === today);
  const byEvent = new Map<string, { dims: Bar[]; buckets: Map<string, { value: number; machines: number }> }>();
  const entry = (e: string) => byEvent.get(e) ?? byEvent.set(e, { dims: [], buckets: new Map() }).get(e)!;
  for (const c of data.by_dims) entry(c.event).dims.push({ key: JSON.stringify(c.dims), label: dimsLabel(c.dims), value: c.count, machines: c.machines });
  for (const c of data.by_bucket) entry(c.event).buckets.set(c.bucket, { value: c.count, machines: c.machines });
  const total = (e: string) => (byEvent.get(e)?.dims ?? []).reduce((s, d) => s + d.value, 0);

  const failures: Bar[] = data.by_dims.filter(c => isFailure(c.event, c.dims ?? {}))
    .map(c => ({ key: c.event + JSON.stringify(c.dims), label: eventTitle(c.event) + " · " + dimsLabel(c.dims), value: c.count, machines: c.machines }))
    .sort((x, y) => y.value - x.value);
  const failTotal = failures.reduce((s, f) => s + f.value, 0);

  const perDay = (keep: (event: string, dims: Dims) => boolean) => {
    const sums = new Map<string, number>();
    for (const t of data.trend) if (keep(t.event, t.dims ?? {})) sums.set(t.day, (sums.get(t.day) ?? 0) + t.count);
    return series(data.days, day => sums.get(day) ?? 0, skipToday);
  };
  const machinesDaily = series(data.days, day => data.daily.find(d => d.day === day)?.machines ?? 0, skipToday);

  // Mensagens por dia de cada agente.
  const msgDay = new Map<string, Partial<Record<AgentKey, number>>>();
  for (const t of data.trend) {
    if (t.event !== "message_sent") continue;
    const k = agentKey(t.dims?.provider);
    const d = msgDay.get(t.day) ?? msgDay.set(t.day, {}).get(t.day)!;
    d[k] = (d[k] ?? 0) + t.count;
  }
  const agentDays = series(data.days, day => msgDay.get(day) ?? {}, skipToday)
    .map(({ day, value }) => ({ day, ...value, total: Object.values(value).reduce((s, n) => s + (n ?? 0), 0) }));
  const agentTotals: Partial<Record<AgentKey, number>> = {};
  for (const r of agentDays) for (const k of [...AGENT_ORDER, "outro"] as AgentKey[]) if (r[k]) agentTotals[k] = (agentTotals[k] ?? 0) + r[k]!;
  const agents = ([...AGENT_ORDER, "outro"] as AgentKey[]).filter(k => agentTotals[k]);

  // O que mais usam: recursos e ferramentas MCP (com as que falharam).
  const features = new Map<string, number>(), tools = new Map<string, { value: number; failed: number }>();
  for (const c of data.by_dims) {
    const d = c.dims ?? {};
    if (c.event === "feature_used" && typeof d.feature === "string") features.set(d.feature, (features.get(d.feature) ?? 0) + c.count);
    if (c.event === "mcp_tool" && typeof d.tool === "string") {
      const t = tools.get(d.tool) ?? tools.set(d.tool, { value: 0, failed: 0 }).get(d.tool)!;
      t.value += c.count;
      if (d.error === "true") t.failed += c.count;
    }
  }

  // Modelo e esforço das sessões abertas.
  const modelMap = new Map<string, { provider: string; model: string; total: number; efforts: Map<string, number> }>();
  for (const c of data.by_dims) {
    if (c.event !== "session_opened") continue;
    const d = c.dims ?? {};
    const provider = typeof d.provider === "string" ? d.provider : "other", model = typeof d.model === "string" ? d.model : "";
    const effort = typeof d.effort === "string" ? d.effort : "";
    const key = provider + "|" + model;
    const m = modelMap.get(key) ?? modelMap.set(key, { provider, model, total: 0, efforts: new Map() }).get(key)!;
    m.total += c.count;
    m.efforts.set(effort, (m.efforts.get(effort) ?? 0) + c.count);
  }
  const models = [...modelMap.values()].sort((x, y) => y.total - x.total);

  // De onde usam: minutos de tela por app e mensagens com/sem terminal.
  const minutes = new Map<string, number>(), surfaces = new Map<string, number>();
  for (const c of data.by_dims) {
    const d = c.dims ?? {};
    if (c.event === "app_visible_minutes") { const k = typeof d.client === "string" ? d.client : "other"; minutes.set(k, (minutes.get(k) ?? 0) + c.count); }
    if (c.event === "message_sent" && typeof d.surface === "string") surfaces.set(d.surface, (surfaces.get(d.surface) ?? 0) + c.count);
  }

  const speed: SpeedCard[] = [];
  const usage: EventCard[] = [];
  const health: EventCard[] = [];
  for (const [event, e] of [...byEvent].sort(([a], [b]) => a.localeCompare(b))) {
    const [title, group, explain] = has(EVENTS, event) ? EVENTS[event] : [event, "outros", "Evento novo, ainda sem descrição neste painel."];
    if (e.buckets.size) {
      const counts = new Map([...e.buckets].map(([b, v]) => [b, v.value]));
      const order = [...e.buckets.keys()].sort(byBucket);
      speed.push({ event, title, explain, sentence: timeSentence(counts), total: [...counts.values()].reduce((s, n) => s + n, 0),
        bars: order.map(b => ({ key: b, label: bucketLabel(b), ...e.buckets.get(b)! })) });
    }
    // As medidas do bloco Velocidade têm cartão próprio lá.
    if (BAD.has(event) || event === "settings" || event === "feature_used" || event === "mcp_tool" || SPEED_EVENTS.includes(event) || !e.dims.length) continue;
    const bars = [...e.dims].sort((x, y) => y.value - x.value);
    if (group === "saude") { if (has(EVENTS, event)) health.push({ event, title, explain, bars }); }
    else usage.push({ event, title, explain, bars });
  }

  // Preferências: máquinas por valor, por app quando mais de um app manda a mesma.
  const bySetting = new Map<string, { value: string; client?: string; machines: number }[]>();
  for (const c of data.by_dims) {
    const d = c.dims ?? {};
    if (c.event !== "settings" || typeof d.setting !== "string" || typeof d.value !== "string") continue;
    (bySetting.get(d.setting) ?? bySetting.set(d.setting, []).get(d.setting)!).push({ value: d.value, client: d.client, machines: c.machines });
  }
  const settingGroups = new Map<string, EventCard[]>([...SETTING_GROUPS, "Outras"].map(g => [g, []]));
  for (const key of Object.keys(SETTINGS).concat([...bySetting.keys()].filter(k => !has(SETTINGS, k)).sort())) {
    const items = bySetting.get(key);
    if (!items) continue;
    const [title, group, explain] = has(SETTINGS, key) ? SETTINGS[key] : [key, "Outras", "Preferência nova, ainda sem descrição neste painel."];
    const clients = new Set(items.map(i => i.client)).size;
    const table = key === "default_provider" ? AGENTS : SETTING_VALUES;
    const label = (v: string) => (has(table, v) ? table[v] : v);
    // O mesmo valor em dois apps sai em duas barras: somar contaria duas vezes a máquina que tem os dois.
    const bars = [...items].sort((x, y) => y.machines - x.machines).map(i => ({ key: i.value + "|" + i.client,
      label: clients > 1 ? label(i.value) + " · " + (VALUES.client[i.client ?? ""] ?? String(i.client)) : label(i.value), value: i.machines }));
    settingGroups.get(group)!.push({ event: key, title, explain, bars });
  }

  const droppedOf = (part: string) => data.by_dims.filter(c => c.event === "diary_dropped" && c.dims?.part === part).reduce((s, c) => s + c.count, 0);
  return {
    skipToday, failures, failTotal, failuresDaily: perDay(isFailure), machinesDaily, turnsDaily: perDay(e => e === "turn_finished"),
    messages: total("message_sent"), turns: total("turn_finished"),
    agentDays, agentTotals, agents,
    features: [...features].sort((x, y) => y[1] - x[1]),
    tools: [...tools].sort((x, y) => y[1].value - x[1].value),
    models, minutes, surfaces, speed, usage, health,
    settingGroups: [...settingGroups].filter(([, cards]) => cards.length), settingCount: bySetting.size,
    droppedRecords: droppedOf("records"), droppedSteps: droppedOf("trail"),
    hasErrors: failTotal > 0 || data.records.some(r => r.level === "error"),
  };
}
export type Derived = ReturnType<typeof derive>;

export const fieldsOf = (r: FailureRecord) => Object.entries(FIELD_LABEL).filter(([key]) => typeof r.fields?.[key] === "string")
  .map(([key, label]) => ({ key, label, value: r.fields![key] as string }));
export const recordKey = (r: FailureRecord) => JSON.stringify([r.event, r.action, r.codigo, r.etapa, r.erro_tipo, r.fields?.causa ?? null]);
export const SAMPLE_TIME = "Amostra (primeira ocorrência no lote)";
export const SAMPLE_TRAIL = "Passos antes da primeira ocorrência da amostra";

export function recordMarkdown(r: FailureRecord) {
  const out = ["## Falha do Hangar: " + r.event, "",
    "Telemetria anônima. Os campos e passos são da amostra mais recente, não da última ocorrência no período.", "",
    "- Nível: " + (r.level === "error" ? "erro" : "aviso"),
    "- Ocorrências: " + quantity(r.count, "vez", "vezes") + " em " + quantity(r.machines, "máquina", "máquinas"),
    "- Primeira vez no período: " + r.first + " UTC; última: " + r.last + " UTC",
    ...(r.sample_at ? ["- " + SAMPLE_TIME + ": " + r.sample_at + " UTC"] : []),
    "- Versões: " + (r.versions ?? []).join(", "),
    "- Sistemas: " + (r.os ?? []).join(", ")];
  const h = happened(r);
  out.push("", "### O que aconteceu", "", h.summary, ...(h.tool ? ["Ferramenta ou ação: " + h.tool] : []),
    ...(h.cause ? ["Por quê: " + h.cause + "."] : []), ...(r.capped ? ["A contagem bateu no teto: foram mais vezes."] : []), "Nomes técnicos: " + h.technical);
  const state = stateRows(r);
  const rows = [...state.moment, ...state.machine, ...state.build];
  out.push("", "### Em que estado a máquina estava", "");
  if (rows.length) out.push(...rows.map(s => "- " + s.label + ": " + s.value + (s.first ? " (na primeira vez: " + s.first + ")" : "") + " [" + s.key + "]"));
  else out.push("Esta falha veio de uma versão que ainda não mandava o estado da máquina.");
  const steps = (title: string, xs: string[]) => {
    out.push("", "### " + title, "");
    if (xs.length) xs.forEach((s, i) => { const t = stepText(s); out.push(i + 1 + ". " + t.text + (t.known ? " [" + s + "]" : "")); });
    else out.push("O diário não guardou passos antes desta falha.");
  };
  steps("Como reproduzir: " + SAMPLE_TRAIL.toLowerCase(), r.trail ?? []);
  const same = (r.trail_last ?? []).join("|") === (r.trail ?? []).join("|");
  if (r.count > 1 && r.trail_last?.length && !same) steps("Passos antes da última vez", r.trail_last);
  const after = afterText(r.after);
  if (after) out.push("", "Depois da última vez: " + after);
  out.push("", "### Campos da amostra", "");
  const fields = fieldsOf(r).map(({ key, value }) => key + ": " + value).join("\n");
  if (fields) {
    let fence = "~~~";
    while (fields.includes(fence)) fence += "~";
    out.push(fence, fields, fence);
  } else out.push("Sem campos nesta falha.");
  return out.join("\n") + "\n";
}

// Vezes no período: por dia até 31 dias, por semana acima disso (a última semana termina no último dia mostrado).
export const SPARK_WEEKLY_FROM = 32;
export const sparkNote = (days: number) => (days < SPARK_WEEKLY_FROM ? "por dia no período" : "por semana no período");
export function sparkSlots(daily: { day: string; count: number }[], days: number, skipToday: boolean) {
  const counts = new Map(daily.map(d => [d.day, d.count]));
  let slots = series(days, day => counts.get(day) ?? 0, skipToday).map(s => ({ label: s.day, value: s.value }));
  if (days >= SPARK_WEEKLY_FROM) {
    const weeks: typeof slots = [];
    for (let end = slots.length; end > 0; end -= 7) {
      const week = slots.slice(Math.max(0, end - 7), end);
      weeks.unshift({ label: "semana até " + week.at(-1)!.label, value: week.reduce((s, d) => s + d.value, 0) });
    }
    slots = weeks;
  }
  return slots;
}

export const machinesLabel = (n: number) => fmt.format(n) + " " + (n === 1 ? "máquina" : "máquinas");
