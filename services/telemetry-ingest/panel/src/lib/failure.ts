// Cada falha com detalhe em português: o que aconteceu, em que estado a máquina estava e os passos até ela.
// Importa com a extensão: o teste roda direto no node, sem bundler.
import { AFTER_LABEL, AGENTS, CAUSES, CLI_LABEL, CLIENT_CODE_LABEL, CONTEXT_LABEL, HEALTH_CODES, INSTALL_LABEL, MACHINE_LABEL, OS_LABEL,
  SCREEN_FAILURE_TITLE, SCREEN_LABEL, STEP_LABEL, TOOLS, VALUES } from "./labels.ts";
import type { FailureRecord } from "./types";

export const has = (o: object, k: string) => Object.hasOwn(o, k);
const say = (values: Record<string, string> | null, v: string) => (values && has(values, v) ? values[v] : v);
const text = (v: unknown) => (typeof v === "string" && v ? v : null);

export interface Happened { summary: string; tool: string | null; cause: string | null; technical: string }

export function happened(r: FailureRecord): Happened {
  const f = r.fields ?? {};
  const code = text(f.codigo), cause = text(f.causa), action = r.action ?? text(f.action);
  const summary = text(f.detalhe) ?? (code && has(HEALTH_CODES, code) ? HEALTH_CODES[code] : "Falha em " + r.event + ".");
  const tool = action ? (has(TOOLS, action) ? TOOLS[action][0] : has(VALUES.feature, action) ? VALUES.feature[action] : null) : null;
  // A causa explica; sem ela, o código do app diz o que houve na rede.
  const said = cause ? (has(CAUSES, cause) ? CAUSES[cause] : null) : code && has(CLIENT_CODE_LABEL, code) ? CLIENT_CODE_LABEL[code] : null;
  return { summary, tool, cause: said, technical: [r.event, action, code, cause].filter(Boolean).join(" · ") };
}

/** O título da falha na lista: o que houve, em português; o nome técnico fica para a linha apagada. */
// Sigla ou nome próprio no começo da causa ("PC desligado") fica como está.
export const lowerFirst = (s: string) => (/^[A-ZÀ-Ý]{2}/.test(s) ? s : s.charAt(0).toLowerCase() + s.slice(1));

export function headline(r: FailureRecord): string {
  const f = r.fields ?? {};
  const code = text(f.codigo), cause = text(f.causa), action = r.action ?? text(f.action);
  const why = cause && has(CAUSES, cause) ? CAUSES[cause] : null;
  const bare = (s: string) => s.replace(/\.$/, "");
  const withWhy = (title: string, reason: string | null) => title + (reason ? ": " + bare(lowerFirst(reason)) : "");
  let line: string;
  if (r.event === "mcp.tool") line = withWhy((action && has(TOOLS, action) ? TOOLS[action][0] : "Uma ferramenta do agente") + " falhou", why);
  else if (r.event === "http.action") {
    line = withWhy((action && has(VALUES.feature, action) ? VALUES.feature[action] : "Uma ação no app") + " falhou", code && has(CAUSES, code) ? CAUSES[code] : null);
  } else if (r.event.startsWith("health.")) line = code && has(HEALTH_CODES, code) ? bare(HEALTH_CODES[code]) : "O servidor teve uma falha sem frase neste painel";
  else if (has(SCREEN_FAILURE_TITLE, r.event)) line = SCREEN_FAILURE_TITLE[r.event];
  else line = "Falha sem frase neste painel";
  return line.charAt(0).toUpperCase() + line.slice(1);
}

export function stepText(step: string): { text: string; known: boolean } {
  const [name, screen] = step.split(":");
  const where = screen ? say(SCREEN_LABEL, screen) : null;
  if (name === "tela.ver") return { text: "Abriu a tela" + (where ? ": " + where : ""), known: true };
  let said: string | null = null;
  if (has(STEP_LABEL, name)) said = STEP_LABEL[name];
  else if (has(TOOLS, name)) said = "Chamou a ferramenta " + TOOLS[name][0];
  else if (has(VALUES.feature, name)) said = "Pediu: " + VALUES.feature[name];
  if (!said) return { text: step, known: false };
  return { text: said + (where ? " (tela: " + where + ")" : ""), known: true };
}

export const afterText = (after: string | null | undefined) => (after ? say(AFTER_LABEL, after) : null);

export function skewText(s: number | null | undefined) {
  if (s == null) return "sem a hora do envio (versão antiga)";
  const a = Math.abs(s);
  // A ida pela rede soma alguns segundos: abaixo de um minuto conta como certo.
  if (a < 60) return "certo";
  const size = a < 3600 ? Math.round(a / 60) + " min" : a < 2 * 86400 ? Math.round(a / 3600) + " h" : Math.round(a / 86400) + " dias";
  return size + (s > 0 ? " atrasado" : " adiantado");
}

export interface StateRow { key: string; label: string; value: string; first?: string }

const CONTEXT_ORDER = Object.keys(CONTEXT_LABEL);

// Retrato da falha (primeira e última vez) e o ambiente da máquina no lote da amostra.
export function stateRows(r: FailureRecord) {
  const b = r.batch;
  const build: StateRow[] = [];
  if (b) {
    build.push({ key: "version", label: "Versão", value: b.version + (b.commit === "dev" ? " · build de desenvolvimento" : " · commit " + b.commit) });
    if (b.install) build.push({ key: "install", label: "Instalação", value: say(INSTALL_LABEL, b.install) });
    build.push({ key: "clock", label: "Relógio da máquina", value: skewText(b.clock_skew_s) });
  }
  const m = b?.machine ?? {};
  const machine: StateRow[] = [];
  if (m.os || m.os_name) {
    const name = m.os_name ? say(OS_LABEL, m.os_name) : say(OS_LABEL, m.os);
    machine.push({ key: "os", label: "Sistema", value: name + (m.os_version && m.os_version !== "absent" ? " " + m.os_version : "") });
  }
  for (const [k, [label, values]] of Object.entries(MACHINE_LABEL)) if (m[k]) machine.push({ key: k, label, value: say(values, m[k]) });
  if (m.mux) machine.push({ key: "mux", label: "Terminal", value: m.mux === "none" ? "nenhum" : m.mux + (m.mux_version && m.mux_version !== "absent" ? " " + m.mux_version : "") });
  for (const [p, label] of Object.entries(CLI_LABEL)) {
    const v = m["cli." + p];
    if (v) machine.push({ key: "cli." + p, label, value: v === "absent" ? "não instalado" : v === "other" ? "versão fora do formato" : v });
  }
  for (const [p, label] of Object.entries(CLI_LABEL)) {
    const v = m["accounts." + p];
    if (v && v !== "0") machine.push({ key: "accounts." + p, label: "Contas " + (AGENTS[p] ?? label), value: say(CONTEXT_LABEL.accounts[1], v) });
  }

  const first = r.ctx_first ?? {}, last = r.ctx_last ?? {};
  const keys = [...new Set([...Object.keys(first), ...Object.keys(last)])];
  const label = (k: string): [string, Record<string, string> | null] => {
    if (has(CONTEXT_LABEL, k)) return CONTEXT_LABEL[k];
    const [, p, surface] = k.split(".");
    if (k.startsWith("sessions.") && p) return ["Sessões " + (has(AGENTS, p) ? AGENTS[p] : p) + " " + (surface && has(VALUES.surface, surface) ? VALUES.surface[surface] : (surface ?? "")), CONTEXT_LABEL.sessions[1]];
    return [k, null];
  };
  const order = (k: string) => { const i = CONTEXT_ORDER.indexOf(k.startsWith("sessions.") ? "sessions" : k); return i < 0 ? 99 : i; };
  const moment: StateRow[] = keys.sort((x, y) => order(x) - order(y) || x.localeCompare(y)).map(k => {
    const [name, values] = label(k);
    const now = has(last, k) ? say(values, last[k]) : "não informado", before = has(first, k) ? say(values, first[k]) : "não informado";
    return { key: k, label: name, value: now, ...(now !== before ? { first: before } : {}) };
  });
  return { build, machine, moment };
}
