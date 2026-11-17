import { afterEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { configureApi, _resetApiEnvForTests, type ApiEnv, type EventSourceLike } from './apiEnv';
import {
  getStatus, openStatusStream, parseStatusEvent,
  type StatusAppReport, type StatusAttention, type StatusAttentionTarget, type StatusConnectState, type StatusDeadSession,
  type StatusDirSize, type StatusDiskUse, type StatusExit, type StatusExitKind, type StatusExitsBlock, type StatusHostBlock,
  type StatusHourBucket, type StatusLeftoversBlock, type StatusMachine, type StatusMachineState, type StatusOutsideBlock,
  type StatusPoint, type StatusRouteStat, type StatusServerBlock, type StatusSessionsBlock, type StatusSeverity,
  type StatusSlowBlock, type StatusSnapshot, type StatusUpdateBlock, type StatusUpdateResult, type StatusUsage,
  type StatusWatchdogEvent, type StatusWatchdogKind,
} from './status';

// O contrato é o Rust: a fixture que o `hangar-api` serializa é lida daqui, sem cópia.
const FIXTURE = join(import.meta.dirname, '..', '..', '..', 'crates/hangar-api/tests/fixtures/status.json');
const fixture = (): Record<string, unknown> => JSON.parse(readFileSync(FIXTURE, 'utf8'));

// Conferência em tempo de execução presa aos tipos: o `Shape<T>` obriga uma entrada por campo do tipo
// (e só eles), e campo opcional no tipo exige `opt(...)`. Assim um campo renomeado em qualquer dos
// lados quebra o teste.
type Check = (v: unknown, path: string) => string[];
type Opt = { opt: Check };
// `{} extends Pick<T, K>` e não `undefined extends T[K]`: com `-?` o TS tira o `undefined` de `T[K]`.
type Shape<T> = { [K in keyof T]-?: {} extends Pick<T, K> ? Opt : Check };

const prim = (kind: 'string' | 'number' | 'boolean'): Check => (v, p) => (typeof v === kind ? [] : [`${p}: esperava ${kind}`]);
const str = prim('string');
const num = prim('number');
const bool = prim('boolean');
const opt = (check: Check): Opt => ({ opt: check });
const enumOf = <T extends string>(all: Record<T, true>): Check => (v, p) =>
  (typeof v === 'string' && v in all ? [] : [`${p}: valor fora do enum (${String(v)})`]);
const list = (item: Check): Check => (v, p) =>
  (Array.isArray(v) ? v.flatMap((x, i) => item(x, `${p}[${i}]`)) : [`${p}: esperava lista`]);
const triple: Check = (v, p) =>
  (Array.isArray(v) && v.length === 3 ? v.flatMap((x, i) => num(x, `${p}[${i}]`)) : [`${p}: esperava 3 números`]);
function obj<T>(shape: Shape<T>): Check {
  return (v, p) => {
    if (!v || typeof v !== 'object' || Array.isArray(v)) return [`${p}: esperava objeto`];
    const rec = v as Record<string, unknown>;
    const spec = shape as unknown as Record<string, Check | Opt>;
    const errors = Object.keys(rec).filter((k) => !(k in spec)).map((k) => `${p}.${k}: campo que o tipo não tem`);
    for (const [k, s] of Object.entries(spec)) {
      if (typeof s === 'function') errors.push(...(k in rec ? s(rec[k], `${p}.${k}`) : [`${p}.${k}: obrigatório ausente`]));
      else if (k in rec) errors.push(...s.opt(rec[k], `${p}.${k}`));
    }
    return errors;
  };
}

const point = obj<StatusPoint>({ ts: str, v: num });
const usage = obj<StatusUsage>({ key: str, count: num, rss_bytes: opt(num), cpu_pct: opt(num) });
const disk = obj<StatusDiskUse>({ used: num, total: num });
const dir = obj<StatusDirSize>({ bytes: opt(num), computing: bool });
const snapshot = obj<StatusSnapshot>({
  generated_at: str,
  server: opt(obj<StatusServerBlock>({
    version: str, commit: opt(str), pid: num, started_at: str, rss_bytes: num, cpu_pct: opt(num),
    rss_series: list(point), cpu_series: list(point), rss_avg_bytes: opt(num), cpu_avg_pct: opt(num),
  })),
  exits: opt(obj<StatusExitsBlock>({
    items: list(obj<StatusExit>({
      ts: str, kind: enumOf<StatusExitKind>({ crash: true, update: true, requested: true }),
      cause: opt(str), from: opt(str), to: opt(str), error_lines: list(str),
    })),
  })),
  update: opt(obj<StatusUpdateBlock>({
    channel: str, installed: str, available: opt(str), last_check: opt(str),
    last_result: opt(obj<StatusUpdateResult>({ ts: str, ok: bool, step: opt(str) })),
  })),
  machines: opt(list(obj<StatusMachine>({
    id: str, label: str, this_machine: bool,
    state: enumOf<StatusMachineState>({ ok: true, down: true, unknown: true }),
    down_since: opt(str), latency_ms: opt(num), server_version: opt(str), outdated: bool, lan: bool,
    app: opt(obj<StatusAppReport>({ version: str, rss_bytes: num, cpu_pct: num, platform: str, host: str })),
  }))),
  sessions: opt(obj<StatusSessionsBlock>({
    open: num, by_provider: list(usage), by_process: list(usage),
    dead: list(obj<StatusDeadSession>({ name: str, cause: str })),
    asleep: num, queued: num, oldest_queued_secs: opt(num),
  })),
  slow: opt(obj<StatusSlowBlock>({
    hours: list(obj<StatusHourBucket>({ hour_start: str, slow: num, errors: num })),
    top_routes: list(obj<StatusRouteStat>({ route: str, count: num, worst_ms: opt(num), last_code: num })),
    watchdog: list(obj<StatusWatchdogEvent>({
      ts: str, kind: enumOf<StatusWatchdogKind>({ late: true, withheld: true }), ms: opt(num),
    })),
  })),
  host: opt(obj<StatusHostBlock>({
    mem_total: num, mem_available: num, swap_total: num, swap_used: num, load: opt(triple), cpus: num,
    disk_home: disk, disk_tmp: opt(disk), dir_hangar: dir, dir_build_cache: dir,
  })),
  outside: opt(obj<StatusOutsideBlock>({
    connect: opt(obj<StatusConnectState>({ enabled: bool, running: bool })), funnel: opt(bool), keyring_locked: opt(bool),
  })),
  leftovers: opt(obj<StatusLeftoversBlock>({ tmux_orphans: opt(list(str)), cano_orphans: opt(num) })),
  attention: list(obj<StatusAttention>({
    severity: enumOf<StatusSeverity>({ bad: true, warn: true }),
    code: str,
    target: enumOf<StatusAttentionTarget>({
      server: true, exits: true, machines: true, sessions: true, slow: true, host: true, outside: true,
    }),
    args: list(str),
  })),
});

const MINIMAL = { generated_at: '2026-11-17T14:23:05Z', attention: [] };
const BLOCKS = ['server', 'exits', 'update', 'machines', 'sessions', 'slow', 'host', 'outside', 'leftovers'] as const;

const target = { id: 'b', label: 'Servidor B', baseUrl: 'https://b.test', token: 'token-b' };
function stubSource(): EventSourceLike {
  return { addEventListener() {}, removeEventListener() {}, close() {}, onerror: null, onopen: null, readyState: 0 };
}
function setup(origin: string | null) {
  const create = vi.fn<ApiEnv['createEventSource']>(() => stubSource());
  configureApi({ getBaseUrl: () => 'https://a.test', getToken: () => 'token-a', onUnauthorized() {}, origin, createEventSource: create });
  return create;
}
afterEach(() => { vi.restoreAllMocks(); _resetApiEnvForTests(); });

describe('retrato de status: contrato C1', () => {
  it('a fixture do Rust tem todos os blocos e casa com o tipo', () => {
    const raw = fixture();
    for (const block of BLOCKS) expect(raw[block], block).toBeDefined();
    expect(snapshot(raw, '$')).toEqual([]);
  });

  it('campo renomeado ou enum novo no Rust aparece na conferência', () => {
    const raw = fixture() as { server: Record<string, unknown>; exits: { items: { kind: string }[] } };
    raw.server.rss = raw.server.rss_bytes;
    delete raw.server.rss_bytes;
    raw.exits.items[0].kind = 'killed';
    expect(snapshot(raw, '$')).toEqual([
      '$.server.rss: campo que o tipo não tem',
      '$.server.rss_bytes: obrigatório ausente',
      '$.exits.items[0].kind: valor fora do enum (killed)',
    ]);
  });

  it('o retrato mínimo é válido e chega com os blocos ausentes', () => {
    expect(snapshot(MINIMAL, '$')).toEqual([]);
    const snap = parseStatusEvent(JSON.stringify(MINIMAL));
    expect(snap).toEqual(MINIMAL);
    for (const block of BLOCKS) expect(snap?.[block], block).toBeUndefined();
  });

  it('evento que não é retrato vira null', () => {
    expect(parseStatusEvent('')).toBeNull();
    expect(parseStatusEvent('{')).toBeNull();
    expect(parseStatusEvent('null')).toBeNull();
    expect(parseStatusEvent('{"attention":[]}')).toBeNull();
    expect(parseStatusEvent('{"generated_at":"x"}')).toBeNull();
  });
});

describe('getStatus e openStatusStream', () => {
  it('getStatus pede /api/status no servidor escolhido com o token dele', async () => {
    setup(null);
    const fetch = vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(new Response(JSON.stringify(MINIMAL)));
    const snap = await getStatus(target);
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe('https://b.test/api/status');
    expect(init?.method ?? 'GET').toBe('GET');
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer token-b');
    expect(snap).toEqual(MINIMAL);
    expect(snap.server).toBeUndefined();
  });

  it('fora da origem, o stream leva token e diag_req; na mesma origem, vai com cookie', () => {
    let create = setup('https://app.test');
    openStatusStream(target, 'diag-x');
    const [url, opts] = create.mock.calls[0];
    const parsed = new URL(url);
    expect(`${parsed.origin}${parsed.pathname}`).toBe('https://b.test/api/status/events');
    expect(parsed.searchParams.get('token')).toBe('token-b');
    expect(parsed.searchParams.get('diag_req')).toBe('diag-x');
    expect(opts.withCredentials).toBe(false);

    create = setup('https://b.test');
    openStatusStream(target, '');
    expect(create.mock.calls[0][0]).toBe('https://b.test/api/status/events');
    expect(create.mock.calls[0][1].withCredentials).toBe(true);
  });
});
