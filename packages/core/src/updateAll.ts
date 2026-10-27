import { acceptUpdateWarningsEm, getAtualizacaoEm, getIdentificadorEm, getUpdateNotesEm, iniciarAtualizacaoEm, type UpdateAcceptResult } from './api';
import { isOwnMachine } from './machineImport';
import { registrar as registerDiagnostic } from './diag';
import type { Server } from './servers';
import type { Atualizacao, AtualizacaoEstado, UpdateMode, UpdateNotes } from './types';

export type UpdateHoldReason = 'branch' | 'mudancas' | 'rodando' | 'falta';
export type UpdateClassification = 'iniciar' | 'em_dia' | `travado:${UpdateHoldReason}`;
export type UpdateFollow = 'rodando' | 'esperando' | 'atualizado' | 'atualizado_manual' | 'falhou';

export type UpdateSnapshot = {
  atualizacao_disponivel?: boolean;
  pending_warning?: boolean;
  versoes?: Partial<Atualizacao['versoes']>;
  pre_voo?: Partial<Atualizacao['pre_voo']>;
  estado?: { fase?: string };
};

/** Versão nova de fato, ou disco à frente do processo; o aviso pendente sozinho não conta. */
export function hasRealUpdate(snapshot: UpdateSnapshot): boolean {
  const clean = (version: string | undefined) => version?.replace(/(?:-dirty)+$/, '');
  const disk = clean(snapshot.versoes?.repo);
  const live = clean(snapshot.versoes?.backend);
  return snapshot.atualizacao_disponivel === true || (disk !== undefined && live !== undefined && disk !== live);
}

/** Mesmo pré-voo do nativo; o canal do app local não participa das outras máquinas. */
export function classify(snapshot: UpdateSnapshot): UpdateClassification {
  // Aviso pendente pede a folha mesmo sem versão nova; quem decide se inicia é `afterAccept`.
  if (!hasRealUpdate(snapshot) && snapshot.pending_warning !== true) return 'em_dia';
  const pre = snapshot.pre_voo;
  // Aceitar o aviso não faz `git pull`: edição ou branch de trabalho não o seguram. Com versão nova, o `iniciar`
  // depois do aceite é que recusa, com o motivo.
  const warning = snapshot.pending_warning === true;
  if (!warning && pre?.branch_de_trabalho === true) return 'travado:branch';
  if (!warning && ((pre?.sujo ?? 0) > 0 || (pre?.ahead ?? 0) > 0 || pre?.divergiu === true)) return 'travado:mudancas';
  if (snapshot.estado?.fase === 'rodando') return 'travado:rodando';
  if (pre?.pode === false) return 'travado:falta';
  return 'iniciar';
}

function timestamp(value: string): { seconds: number; fraction: string } | null {
  const parts = /^(\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?([Zz]|[+-]\d{2}:\d{2})$/.exec(value);
  if (!parts) return null;
  const seconds = Date.parse(`${parts[1]}${parts[3]}`);
  // O estado Python usa microssegundos; Date.parse sozinho perderia essa ordem.
  return Number.isFinite(seconds) ? { seconds, fraction: (parts[2] ?? '').padEnd(9, '0') } : null;
}

function afterBaseline(ts: string | undefined, baseline: string | null | undefined): boolean {
  if (baseline == null) return true;
  if (ts === undefined) return false;
  const current = timestamp(ts);
  const previous = timestamp(baseline);
  if (!current || !previous) return ts !== baseline;
  return current.seconds > previous.seconds || (current.seconds === previous.seconds && current.fraction > previous.fraction);
}

/** O desfecho precisa ser posterior ao GET anterior ao iniciar, independentemente do pid. */
export function follow(state: AtualizacaoEstado, baseTs?: string | null): UpdateFollow {
  if (state.fase === 'rodando') return 'rodando';
  if (state.fase !== 'pronto' || !afterBaseline(state.ts, baseTs)) return 'esperando';
  if (state.ok !== true) return 'falhou';
  return state.reiniciar_manual === true ? 'atualizado_manual' : 'atualizado';
}

export function ownUpdatableServers(list: readonly Server[]): Server[] {
  return list.filter((server) => isOwnMachine(server) && !server.disabled);
}

/** O que a pessoa aceitou na folha para uma máquina: os ids lidos e, com o aviso da migração, o modo. */
export interface UpdateAcceptance { ids: string[]; mode?: UpdateMode }
export type AfterAccept = 'started' | 'start' | 'done';

/** `null` = servidor antigo, sem a rota: inicia como antes. Aceite gravado sem lançar inicia só com versão nova. */
export function afterAccept(accepted: UpdateAcceptResult | null | undefined, snapshot: UpdateSnapshot): AfterAccept {
  if (accepted === null) return 'start';
  if (accepted?.started) return 'started';
  return hasRealUpdate(snapshot) ? 'start' : 'done';
}

/** Aceita os avisos e inicia conforme `afterAccept`; `null` = nada foi iniciado. Sem `snapshot`, relê o estado. */
export async function acceptAndStart(server: Server, acceptance: UpdateAcceptance | undefined, snapshot?: UpdateSnapshot,
  fetcher?: typeof fetch): Promise<{ ok: boolean; pid: number } | null> {
  const accepted = acceptance?.ids.length ? await acceptUpdateWarningsEm(server, acceptance.ids, acceptance.mode, fetcher) : undefined;
  if (accepted?.started) return { ok: true, pid: accepted.pid! };
  const next = afterAccept(accepted, accepted === null ? {} : snapshot ?? await getAtualizacaoEm(server, false, fetcher));
  return next === 'start' ? iniciarAtualizacaoEm(server, fetcher) : null;
}

/** As máquinas próprias, uma por identidade; `last` vence como representante da dele. */
export async function distinctMachines(servers: readonly Server[], opts: Pick<UpdateAllOptions, 'last' | 'fetch'> = {}): Promise<Server[]> {
  const candidates = ownUpdatableServers(servers);
  const identities = await Promise.all(candidates.map(async (server) => {
    try {
      const response = await getIdentificadorEm(server, opts.fetch);
      return typeof response.identificador === 'string' ? response.identificador.trim() : '';
    } catch (error) {
      registerDiagnostic({ evento: 'update_all.identity_failed', nivel: 'aviso',
        codigo: error instanceof Error ? error.name : 'erro' }, server.baseUrl);
      return '';
    }
  }));
  const representatives = new Map<string, Server>();
  candidates.forEach((server, index) => {
    const identity = identities[index];
    if (identity && (!representatives.has(identity) || server.id === opts.last)) representatives.set(identity, server);
  });
  return candidates.filter((server, index) => !identities[index] || representatives.get(identities[index]) === server);
}

export interface MachineNotes { server: Server; notes: UpdateNotes | null }
export interface PreparedUpdateAll { run: MachineNotes[]; failed: Server[] }

/** Avisos de cada máquina a atualizar; quem não responde fica de fora (`failed`), `null` = servidor antigo. */
export async function prepareUpdateAll(servers: readonly Server[], opts: Pick<UpdateAllOptions, 'last' | 'fetch'> = {}): Promise<PreparedUpdateAll> {
  const targets = await distinctMachines(servers, opts);
  const read = await Promise.all(targets.map(async (server): Promise<MachineNotes | null> => {
    try {
      return { server, notes: await getUpdateNotesEm(server, opts.fetch) };
    } catch (error) {
      registerDiagnostic({ evento: 'update_all.notes_failed', nivel: 'aviso',
        codigo: error instanceof Error ? error.name : 'erro' }, server.baseUrl);
      return null;
    }
  }));
  return { run: read.filter((entry): entry is MachineNotes => entry !== null), failed: targets.filter((_, index) => !read[index]) };
}

export interface UpdateAllState {
  status: 'consultando' | 'iniciando' | 'em_dia' | 'travado' | 'fora_do_ar' | UpdateFollow;
  reason?: UpdateHoldReason;
  error?: string;
  errorCode?: 'silent' | 'timeout';
  /** Campos originais do motor: passo, total, texto, erro e avisos. */
  state?: AtualizacaoEstado;
  preflight?: Atualizacao['pre_voo'];
  warnings: string[];
}

export interface UpdateAllOptions {
  /** Servidor da página PWA: seu reinício só começa depois das outras máquinas. */
  last?: string;
  /** Aceite da folha por id de servidor, gravado antes de iniciar cada máquina. */
  accept?: Readonly<Record<string, UpdateAcceptance>>;
  fetch?: typeof fetch;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

export interface UpdateAllResult { server: Server; state: UpdateAllState }

const POLL_MS = 2000;
const SILENT_READS = 300;
const FOLLOW_CEILING_MS = 30 * 60 * 1000;
const errorMessage = (error: unknown): string => error instanceof Error ? error.message : String(error);
const httpStatus = (error: unknown): unknown =>
  typeof error === 'object' && error !== null && 'status' in error ? error.status : undefined;

/** Publica só representantes de máquinas distintas e devolve seus desfechos na ordem da lista. */
export async function updateAll(
  servers: readonly Server[],
  onChange: (serverId: string, state: UpdateAllState) => void,
  opts: UpdateAllOptions = {},
): Promise<UpdateAllResult[]> {
  const now = opts.now ?? (() => performance.now());
  const sleep = opts.sleep ?? ((ms) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  const unique = await distinctMachines(servers, opts);

  async function run(server: Server): Promise<UpdateAllResult> {
    let previous = '';
    const publish = (value: UpdateAllState): UpdateAllState => {
      const signature = JSON.stringify(value);
      if (signature !== previous) { previous = signature; onChange(server.id, value); }
      return value;
    };
    const finish = (value: UpdateAllState): UpdateAllResult => ({ server, state: publish(value) });
    publish({ status: 'consultando', warnings: [] });
    let snapshot: Atualizacao;
    try {
      snapshot = await getAtualizacaoEm(server, true, opts.fetch);
    } catch (error) {
      return finish({ status: httpStatus(error) !== undefined ? 'falhou' : 'fora_do_ar', error: errorMessage(error), warnings: [] });
    }
    const classification = classify(snapshot);
    if (classification === 'em_dia') return finish({ status: 'em_dia', warnings: [] });
    if (classification !== 'iniciar') return finish({ status: 'travado', reason: classification.slice(8) as UpdateHoldReason,
      state: snapshot.estado, preflight: snapshot.pre_voo, warnings: [] });

    const baseTs = snapshot.estado.ts;
    const startedAt = now();
    publish({ status: 'iniciando', warnings: [] });
    try {
      if (!await acceptAndStart(server, opts.accept?.[server.id], snapshot, opts.fetch)) return finish({ status: 'em_dia', warnings: [] });
    } catch (error) {
      return finish({ status: 'falhou', error: errorMessage(error), warnings: [] });
    }
    let silent = 0;
    let latest: UpdateAllState = { status: 'esperando', warnings: [] };
    for (;;) {
      await sleep(POLL_MS);
      let value: UpdateAllState;
      try {
        const { estado: state } = await getAtualizacaoEm(server, false, opts.fetch);
        silent = 0;
        const status = follow(state, baseTs);
        value = { status, state, warnings: state.avisos ?? [] };
        if (status === 'falhou') value.error = state.erro?.replace(/\.+$/, '') || '?';
        if (status === 'atualizado' || status === 'atualizado_manual' || status === 'falhou') return finish(value);
      } catch (error) {
        const status = httpStatus(error);
        if (status === 401 || status === 403) {
          return finish({ status: 'falhou', error: errorMessage(error), state: latest.state, warnings: latest.warnings });
        }
        silent++;
        value = { ...latest, status: 'esperando', error: errorMessage(error) };
      }
      if (silent >= SILENT_READS || now() - startedAt >= FOLLOW_CEILING_MS) {
        return finish({ status: 'falhou', errorCode: silent >= SILENT_READS ? 'silent' : 'timeout',
          state: value.state, warnings: value.warnings });
      }
      latest = publish(value);
    }
  }

  // O representante de last também fica para o fim quando outro endereço aponta à mesma máquina.
  const results = await Promise.all(unique.filter((server) => server.id !== opts.last).map(run));
  const last = unique.find((server) => server.id === opts.last);
  if (last) results.push(await run(last));
  return unique.map((server) => results.find((result) => result.server === server)!);
}
