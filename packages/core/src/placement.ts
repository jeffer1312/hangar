// Balanceamento de sessões: as rotas `/api/placement/*` do servidor, o motivo da escolha no idioma do app e a
// validação que a tela faz antes de gravar (a mesma do `hangar-placement`).
import { apiFetchForServer, getIdentificadorEm } from './api';
import type { Server } from './servers';
import * as m from './paraglide/messages';

export type PlacementUiMode = 'off' | 'suggest' | 'ask';
export type PlacementAgentMode = 'off' | 'auto' | 'ask';
export interface PlacementLimits { mem_free_pct: number; working: number; cpu_pct: number }
export interface PlacementProjectRule { project: string; server: string }
/** `days`: 1 = segunda … 7 = domingo; vazio vale todos. */
export interface PlacementScheduleRule { days: number[]; from: string; to: string; tz: string; avoid: string }
/** `~/.hangar/placement.json` do servidor. */
export interface PlacementConfig {
  ui_mode: PlacementUiMode;
  agent_mode: PlacementAgentMode;
  limits: PlacementLimits;
  project_rules: PlacementProjectRule[];
  schedule_rules: PlacementScheduleRule[];
  clone_dirs: Record<string, string>;
}

export type PlacementExclusion = 'missing' | 'not_capable' | 'schedule' | 'memory' | 'working' | 'cpu' | 'no_clone_dir';
export type PlacementReason =
  | { code: 'same_folder' }
  | { code: 'unpublished'; why: 'ahead' | 'dirty' | 'no_upstream' }
  | { code: 'plain_dir' }
  | { code: 'project_rule'; project: string }
  | { code: 'local_within_limits' }
  | { code: 'least_loaded'; local_out: PlacementExclusion }
  | { code: 'no_candidate' }
  | { code: 'not_balanced'; why: string };
export type PlacementWarning =
  | { code: 'rule_server_out'; server: string; why: PlacementExclusion }
  | { code: 'rule_overridden'; server: string }
  | { code: 'all_over_limit' };
/** A escolha: `server` é o identificador da máquina (`CP_SERVER_ID`), não o id do app. */
export interface Placement { server: string; reason: PlacementReason; clone: { dir: string } | null; warnings: PlacementWarning[] }
/** O que a tela usa de `GET /api/placement/facts`. */
export interface PlacementFacts { server_id: string; version: string; repos: { project: string; root: string }[]; clone_dir: string | null }

// A escolha espera até 2 s pelos fatos de cada máquina: o prazo cobre isso e a ida e volta.
const WAIT = 5000;

export function getPlacementConfig(server: Server): Promise<PlacementConfig> {
  return apiFetchForServer<PlacementConfig>(server, '/api/placement/config', undefined, WAIT);
}

/** Grava e devolve o que o servidor guardou. */
export function savePlacementConfig(server: Server, config: PlacementConfig): Promise<PlacementConfig> {
  return apiFetchForServer<PlacementConfig>(server, '/api/placement/config', { method: 'PUT', body: JSON.stringify(config) }, WAIT);
}

export function getPlacementFacts(server: Server): Promise<PlacementFacts> {
  return apiFetchForServer<PlacementFacts>(server, '/api/placement/facts', undefined, WAIT);
}

/** A escolha para um pedido de abertura, sem criar nada. */
export function suggestPlacement(server: Server, body: Record<string, unknown> = { cwd: '~', machine: 'auto' }): Promise<Placement> {
  return apiFetchForServer<Placement>(server, '/api/placement/suggest', { method: 'POST', body: JSON.stringify(body) }, WAIT * 2);
}

/** A sugestão só marca a máquina com a tela em "sugerir" e o pedido dentro do balanceamento. */
export function placementSuggested(config: Pick<PlacementConfig, 'ui_mode'>, placement: Partial<Placement>): Placement | null {
  if (config.ui_mode !== 'suggest') return null;
  const code = placement.reason?.code;
  if (!code || code === 'not_balanced' || !placement.server) return null;
  return placement as Placement;
}

export const placementProjectTitle = (project: string) => project.replace(/^git:/, '');

function exclusionText(why: string): string {
  switch (why) {
    case 'missing': return m.native_placement_exclusion_missing();
    case 'not_capable': return m.native_placement_exclusion_not_capable();
    case 'schedule': return m.native_placement_exclusion_schedule();
    case 'memory': return m.native_placement_exclusion_memory();
    case 'working': return m.native_placement_exclusion_working();
    case 'cpu': return m.native_placement_exclusion_cpu();
    case 'no_clone_dir': return m.native_placement_exclusion_no_clone_dir();
    default: return why;
  }
}

function notBalancedText(why: string): string {
  switch (why) {
    case 'guest': return m.native_placement_not_balanced_guest();
    case 'prewarm': return m.native_placement_not_balanced_prewarm();
    case 'local_field': return m.native_placement_not_balanced_local_field();
    case 'link': return m.native_placement_not_balanced_link();
    case 'orq': return m.native_placement_not_balanced_orq();
    case 'mode_off': return m.native_placement_not_balanced_mode_off();
    default: return why;
  }
}

/** O motivo da escolha no idioma do app, montado dos códigos: o `text` do servidor sai sempre em português.
 *  `label` troca o identificador da máquina pelo nome que a pessoa conhece. */
export function placementReasonText(placement: Placement, label: (server: string) => string = (s) => s): string {
  const maquina = label(placement.server);
  const r = placement.reason;
  let text: string;
  switch (r.code) {
    case 'same_folder': text = m.native_placement_reason_same_folder({ maquina }); break;
    case 'unpublished': text = r.why === 'dirty' ? m.native_placement_reason_unpublished_dirty()
      : r.why === 'no_upstream' ? m.native_placement_reason_unpublished_no_upstream() : m.native_placement_reason_unpublished_ahead(); break;
    case 'plain_dir': text = m.native_placement_reason_plain_dir({ maquina }); break;
    case 'project_rule': text = m.native_placement_reason_project_rule({ projeto: placementProjectTitle(r.project), maquina }); break;
    case 'local_within_limits': text = m.native_placement_reason_local_within_limits({ maquina }); break;
    case 'least_loaded': text = m.native_placement_reason_least_loaded({ motivo: exclusionText(r.local_out), maquina }); break;
    case 'no_candidate': text = m.native_placement_reason_no_candidate({ maquina }); break;
    case 'not_balanced': text = m.native_placement_reason_not_balanced({ motivo: notBalancedText(r.why) }); break;
    default: text = (r as { code: string }).code;
  }
  for (const w of placement.warnings ?? []) {
    text += '; ' + (w.code === 'rule_server_out' ? m.native_placement_warning_rule_server_out({ maquina: label(w.server), motivo: exclusionText(w.why) })
      : w.code === 'rule_overridden' ? m.native_placement_warning_rule_overridden({ maquina: label(w.server) })
        : m.native_placement_warning_all_over_limit());
  }
  if (placement.clone) text += '; ' + m.native_placement_clone_note({ maquina, dir: placement.clone.dir });
  return text;
}

export interface PlacementSuggestion { serverId: string; text: string }

const identOf = (server: Server) => getIdentificadorEm(server, undefined, WAIT).then((r) => r.identificador || null, () => null);

/** As máquinas de `machines` fora `skip`, com o identificador de cada uma (`null`: não respondeu), lidos em paralelo. */
async function othersWithIdent(machines: readonly Server[], skip: Server): Promise<Array<[Server, string | null]>> {
  const others = machines.filter((s) => s.id !== skip.id);
  const idents = await Promise.all(others.map(identOf));
  return others.map((s, i) => [s, idents[i]]);
}

/** A máquina da lista (`Server.id`) que a Nova conversa marca e o motivo, perguntados ao servidor ativo.
 *  `null`: nada a marcar, ou a sugerida não está entre `machines`. Falha de leitura rejeita. */
export async function placementSuggestionFor(active: Server, machines: readonly Server[]): Promise<PlacementSuggestion | null> {
  const [config, placement] = await Promise.all([getPlacementConfig(active), suggestPlacement(active)]);
  const chosen = placementSuggested(config, placement);
  if (!chosen) return null;
  const names = new Map<string, string>();
  const reply = (server: Server) => ({ serverId: server.id, text: placementReasonText(chosen, (id) => names.get(id) ?? id) });
  // Quase sempre a sugerida é a ativa: uma pergunta só.
  const here = await identOf(active);
  if (here) names.set(here, active.label);
  if (here === chosen.server) return reply(active);
  const others = await othersWithIdent(machines, active);
  for (const [s, id] of others) if (id) names.set(id, s.label);
  const found = others.find(([, id]) => id === chosen.server)?.[0];
  return found ? reply(found) : null;
}

/** A pasta escolhida tem regra de projeto para outra máquina: a Nova conversa oferece abrir lá. `server`: o id que a
 *  máquina informa (vai no `machine` do pedido); `serverId`: a mesma máquina na lista do app; `cwd`: a pasta da oferta. */
export interface PlacementRuleOffer { server: string; serverId: string; project: string; cwd: string }

/** A máquina e o projeto da regra, quando o motivo da escolha é uma regra de projeto. */
export function placementRuleOf(placement: Partial<Placement>): { server: string; project: string } | null {
  const r = placement.reason;
  if (r?.code !== 'project_rule' || !r.project || !placement.server) return null;
  return { server: placement.server, project: placementProjectTitle(r.project) };
}

/** A oferta da regra para `cwd`, perguntada à máquina marcada; com a tela em "desligado" nada é pedido.
 *  `null`: sem regra, regra para a própria marcada, ou máquina fora de `machines`. Falha de leitura rejeita. */
export async function placementRuleOfferFor(marked: Server, machines: readonly Server[], cwd: string): Promise<PlacementRuleOffer | null> {
  const config = await getPlacementConfig(marked);
  if (config.ui_mode === 'off') return null;
  const rule = placementRuleOf(await suggestPlacement(marked, { cwd, machine: 'auto' }));
  if (!rule || await identOf(marked) === rule.server) return null;
  const found = (await othersWithIdent(machines, marked)).find(([, id]) => id === rule.server)?.[0];
  return found ? { server: rule.server, serverId: found.id, project: rule.project, cwd } : null;
}

// Conta, motor, perfil e reserva são da máquina daqui: com eles o servidor recusa abrir noutra. Lá vale o padrão de lá.
const LOCAL_FIELDS = ['config_dir', 'force_account', 'engine', 'engine_account', 'engine_binding', 'codex_account', 'omp_profile', 'prewarm'];

/** O corpo de abertura que o servidor daqui encaminha à máquina da regra (`server`): ele troca a pasta pela do mesmo projeto lá. */
export function placementElsewhereBody<T extends object>(body: T, server: string): T & { machine: string } {
  const out: Record<string, unknown> = { ...(body as Record<string, unknown>) };
  for (const key of LOCAL_FIELDS) delete out[key];
  return { ...out, machine: server } as T & { machine: string };
}

export type PlacementRead = { state: 'ready'; config: PlacementConfig } | { state: 'old' } | { state: 'failed'; error: string };
/** Uma máquina vista pela tela Balanceamento. */
export interface PlacementMachine { serverId: string; ident: string | null; read: PlacementRead; facts: PlacementFacts | null }

const errorText = (e: unknown) => e instanceof Error ? e.message : String(e);

/** Lê regras, fatos e identificador de cada máquina, em paralelo; a que falha não impede as outras. */
export function readPlacementAll(servers: readonly Server[]): Promise<PlacementMachine[]> {
  return Promise.all(servers.map(async (server): Promise<PlacementMachine> => {
    const [ident, read, facts] = await Promise.all([
      identOf(server),
      getPlacementConfig(server).then((config): PlacementRead => ({ state: 'ready', config }), (e: { status?: number; code?: string }): PlacementRead =>
        // Rota que não existe lá (404 sem código) é versão sem o balanceamento.
        e?.status === 404 && !e.code ? { state: 'old' } : { state: 'failed', error: errorText(e) }),
      getPlacementFacts(server).catch(() => null),
    ]);
    return { serverId: server.id, ident, read, facts };
  }));
}

export interface PlacementSaved { serverId: string; ok: boolean; config?: PlacementConfig; error?: string }

/** Grava a mesma configuração em cada máquina; quem chama passa só as que responderam com o balanceamento. */
export function savePlacementAll(servers: readonly Server[], config: PlacementConfig): Promise<PlacementSaved[]> {
  return Promise.all(servers.map(async (server): Promise<PlacementSaved> => {
    try { return { serverId: server.id, ok: true, config: await savePlacementConfig(server, config) }; }
    catch (e) { return { serverId: server.id, ok: false, error: errorText(e) }; }
  }));
}

// O servidor devolve `clone_dirs` em ordem alfabética; o rascunho, na ordem em que foi editado.
const configKey = (c: PlacementConfig) => JSON.stringify([c.ui_mode, c.agent_mode, c.limits.mem_free_pct, c.limits.working, c.limits.cpu_pct,
  c.project_rules.map((r) => [r.project, r.server]), c.schedule_rules.map((r) => [r.days, r.from, r.to, r.tz, r.avoid]),
  Object.entries(c.clone_dirs).sort(([a], [b]) => a.localeCompare(b))]);
export const samePlacementConfig = (a: PlacementConfig, b: PlacementConfig) => configKey(a) === configKey(b);

export type PlacementStatus = 'loading' | 'old' | 'silent' | 'same' | 'different';
/** Como a máquina está em relação às regras da máquina de referência. */
export function placementStatus(read: PlacementRead | null, here: PlacementConfig | null): PlacementStatus {
  if (!read) return 'loading';
  if (read.state === 'old') return 'old';
  if (read.state === 'failed') return 'silent';
  return here && samePlacementConfig(read.config, here) ? 'same' : 'different';
}

/** Os projetos que alguma máquina conhece, sem repetição e em ordem. */
export function placementProjects(machines: readonly PlacementMachine[]): string[] {
  return [...new Set(machines.flatMap((m) => m.facts?.repos.map((r) => r.project) ?? []))].sort();
}

export type PlacementCheck<T> = { ok: true; value: T } | { ok: false; error: string };

/** Os números dos limites como o servidor aceita (`store::validate`). */
export function placementLimits(mem: string, working: string, cpu: string): PlacementCheck<PlacementLimits> {
  const int = (s: string) => /^\d+$/.test(s.trim()) ? Number(s.trim()) : NaN;
  const [memPct, cpuPct, sessions] = [int(mem), int(cpu), int(working)];
  if (!(memPct <= 100) || !(cpuPct <= 100)) return { ok: false, error: m.native_placement_limit_pct_invalid() };
  if (!(sessions >= 1)) return { ok: false, error: m.native_placement_limit_working_invalid() };
  return { ok: true, value: { mem_free_pct: memPct, working: sessions, cpu_pct: cpuPct } };
}

function validTimeZone(tz: string): boolean {
  const accepts = (zone: string) => { try { new Intl.DateTimeFormat('en-US', { timeZone: zone }); return true; } catch { return false; } };
  // Motor sem fusos no Intl recusa até o UTC: aí quem confere é o servidor.
  return accepts(tz) || !accepts('UTC');
}

/** A regra de horário como o servidor aceita: hora `HH:MM` e fuso IANA, senão ela nunca valeria. */
export function placementScheduleRule(days: readonly number[], from: string, to: string, tz: string, avoid: string | null): PlacementCheck<PlacementScheduleRule> {
  const time = (s: string) => {
    const hit = /^([01]?\d|2[0-3]):([0-5]\d)$/.exec(s.trim());
    return hit ? `${hit[1].padStart(2, '0')}:${hit[2]}` : null;
  };
  const [start, end] = [time(from), time(to)];
  if (!start || !end) return { ok: false, error: m.native_placement_schedule_time_invalid() };
  const zone = tz.trim();
  if (!zone || !validTimeZone(zone)) return { ok: false, error: m.native_placement_schedule_tz_invalid() };
  if (!avoid) return { ok: false, error: m.native_placement_schedule_machine_missing() };
  return { ok: true, value: { days: [...days].sort((a, b) => a - b), from: start, to: end, tz: zone, avoid } };
}

/** O fuso deste aparelho pelo nome IANA; vazio quando o motor não diz. */
export function placementLocalTz(): string {
  try { return Intl.DateTimeFormat().resolvedOptions().timeZone ?? ''; } catch { return ''; }
}
