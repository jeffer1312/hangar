import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from './apiEnv';
import { canMergeReview, getReviewForServer, getReviewsForServer, mergeReviewLocations, readReviewSources, reviewActionForServer, reviewActivityText, reviewNeedsAttention, type ReviewDetail, type ReviewSource } from './reviews';
import type { Server } from './servers';
import { getLocale, overwriteGetLocale } from './paraglide/runtime';
const originalLocale = getLocale;

const server: Server = { id: 'server-a', label: 'Servidor A', baseUrl: 'http://machine.test', token: 'machine-token' };
const target = { host: 'git.example.test', repo: 'team/sub/app', number: 42 };
const detail: ReviewDetail = { ...target, provider: 'gitlab', url: 'https://git.example.test/team/sub/app/-/merge_requests/42',
  title: 'Busca', author: 'dev', mine: true, review_requested: false, draft: false, state: 'open', source: 'PM-42', source_repo: 'team/sub/app', target: 'main',
  pipeline: 'success', open_threads: 0, conflicts: false, sessions: [], behind: 0,
  approvals: { state: 'approved', need: 0, by: [], requested: [], can_approve: false },
  merge: { can_merge: true, can_auto: true, auto_enabled: false, blockers: [] }, jobs: [], threads: [], files: [], activity: [] };

beforeEach(() => {
  overwriteGetLocale(() => 'pt');
  configureApi({ getBaseUrl: () => 'http://active.test', getToken: () => 'active-token', origin: null,
    onUnauthorized: () => {}, createEventSource: () => { throw new Error('Não há stream nesta prova.'); } });
});
afterEach(() => { vi.unstubAllGlobals(); overwriteGetLocale(originalLocale); });

describe('Revisões pela API pública', () => {
  it('consulta e executa na máquina capturada, preservando o alvo e a thread', async () => {
    const requests: { url: URL; init: RequestInit | undefined }[] = [];
    vi.stubGlobal('fetch', vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = new URL(String(input)); requests.push({ url, init });
      return new Response(JSON.stringify(url.pathname.endsWith('/action') ? { ok: true } : url.pathname.endsWith('/detail') ? detail : { hosts: [], reviews: [detail] }), { status: 200 });
    }));
    expect((await getReviewsForServer(server)).reviews).toHaveLength(1);
    expect((await getReviewForServer(server, target)).number).toBe(42);
    await reviewActionForServer(server, target, 'reply', { thread: 'thread-1', body: 'Resposta com acentos.' });
    expect(requests.map(({ url }) => url.origin)).toEqual(['http://machine.test', 'http://machine.test', 'http://machine.test']);
    expect(requests[1].url.searchParams.get('repo')).toBe('team/sub/app');
    expect(requests[1].url.searchParams.get('number')).toBe('42');
    expect(new Headers(requests[2].init?.headers).get('authorization')).toBe('Bearer machine-token');
    expect(JSON.parse(String(requests[2].init?.body))).toEqual({ ...target, action: 'reply', thread: 'thread-1', body: 'Resposta com acentos.' });
  });

  it('não apresenta pipeline em andamento como pronta para mesclar', () => {
    expect(canMergeReview(detail)).toBe(true);
    for (const pipeline of ['running', 'pending', 'failed', 'canceled', 'unknown']) expect(canMergeReview({ ...detail, pipeline })).toBe(false);
    expect(canMergeReview({ ...detail, draft: true })).toBe(false);
    expect(canMergeReview({ ...detail, merge: { ...detail.merge, can_merge: false } })).toBe(false);
    expect(canMergeReview({ ...detail, pipeline: null })).toBe(true);
  });

  it('conta pedidos de revisão e problemas dos seus próprios MR/PR', () => {
    expect(reviewNeedsAttention({ ...detail, mine: false, review_requested: true })).toBe(true);
    expect(reviewNeedsAttention({ ...detail, pipeline: 'failed' })).toBe(true);
    expect(reviewNeedsAttention({ ...detail, mine: false, pipeline: 'failed' })).toBe(false);
    expect(reviewNeedsAttention({ ...detail, open_threads: 1 })).toBe(true);
    expect(reviewNeedsAttention(detail)).toBe(false);
  });

  it('confere a pipeline novamente antes de enviar uma mesclagem', async () => {
    const writes: string[] = [];
    vi.stubGlobal('fetch', vi.fn(async (_input: string, init?: RequestInit) => {
      if (init?.method === 'POST') writes.push(String(init.body));
      return new Response(JSON.stringify(init?.method === 'POST' ? { ok: true } : { ...detail, pipeline: 'running' }), { status: 200 });
    }));
    await expect(reviewActionForServer(server, target, 'merge')).rejects.toMatchObject({ status: 409 });
    expect(writes).toEqual([]);
  });

  it('uma revisão em duas máquinas aparece uma vez, na origem com sessão na branch', () => {
    const rows = mergeReviewLocations([
      { serverId: 'server-a', review: { ...detail, review_requested: true } },
      { serverId: 'server-b', review: { ...detail, sessions: ['session-1'], review_requested: true } },
    ]);
    expect(rows.map((row) => row.serverId)).toEqual(['server-b']);
    expect(rows.filter((row) => reviewNeedsAttention(row.review))).toHaveLength(1);
  });

  it('publica a máquina rápida antes de a outra terminar', async () => {
    let finishSlow!: (response: Response) => void;
    const slow = new Promise<Response>((resolve) => { finishSlow = resolve; });
    vi.stubGlobal('fetch', vi.fn(async (input: string) => new URL(String(input)).hostname === 'slow.test' ? slow
      : new Response(JSON.stringify({ hosts: [], reviews: [detail] }), { status: 200 })));
    const updates: ReviewSource[][] = [];
    const reading = readReviewSources([server, { ...server, id: 'server-b', baseUrl: 'http://slow.test' }], { onUpdate: (value) => updates.push(value) });
    await vi.waitFor(() => expect(updates.some((sources) => sources[0]?.listing?.reviews.length === 1 && sources[1]?.loading)).toBe(true));
    finishSlow(new Response(JSON.stringify({ hosts: [], reviews: [] }), { status: 200 }));
    expect((await reading).every((source) => !source.loading)).toBe(true);
  });

  it('descreve o verbo da atividade mesmo quando o texto do provedor está vazio', () => {
    const activity = { who: 'dev', text: '', at: '' };
    expect(reviewActivityText({ ...activity, kind: 'approved' })).toBe('aprovou');
    expect(reviewActivityText({ ...activity, kind: 'force_pushed' })).toBe('reescreveu a branch');
    expect(reviewActivityText({ ...activity, kind: 'review_requested', text: 'ana' })).toBe('pediu a revisão de ana');
  });
});
