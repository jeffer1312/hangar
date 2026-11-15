import type { Server } from './servers';
import { apiFetchForServer, comTeto } from './api';
import * as m from './paraglide/messages';

export interface ReviewTarget { host: string; repo: string; number: number }
export interface ReviewSummary extends ReviewTarget {
  provider: 'github' | 'gitlab'; url: string; title: string; author: string; mine: boolean;
  review_requested: boolean; draft: boolean; state: string; source: string; source_repo: string;
  target: string; pipeline: string | null; open_threads: number | null; conflicts: boolean | null;
  sessions: string[];
}
export interface ReviewHost { host: string; repos: number; state: string; cli: string | null; user: string | null; error: string | null }
export interface ReviewListing { hosts: ReviewHost[]; reviews: ReviewSummary[] }
export interface ReviewSource { server: Server; listing?: ReviewListing; error: string; loading: boolean }
export interface ReviewComment { author: string; body: string; at: string; bot: boolean }
export interface ReviewThread { id: string; file: string | null; line: number | null; resolved: boolean; code: string; comments: ReviewComment[] }
export interface ReviewFile { path: string; additions: number; deletions: number; patch: string }
export interface ReviewActivity { kind: string; who: string; text: string; at: string }
export function reviewActivityText(activity: ReviewActivity): string {
  const text = activity.text;
  switch (activity.kind) {
    case 'commit': return m.native_review_activity_commit({ texto: text });
    case 'approved': return m.native_review_activity_approved();
    case 'changes_requested': return m.native_review_activity_changes_requested();
    case 'reviewed': return m.native_review_activity_reviewed();
    case 'comment': return m.native_review_activity_comment({ texto: text });
    case 'merged': return m.native_review_activity_merged();
    case 'review_requested': return m.native_review_activity_review_requested({ texto: text });
    case 'ready': return m.native_review_activity_ready();
    case 'force_pushed': return m.native_review_activity_force_pushed();
    case 'closed': return m.native_review_activity_closed();
    case 'reopened': return m.native_review_activity_reopened();
    default: return text || m.native_review_activity_reviewed();
  }
}
export interface ReviewDetail extends ReviewSummary {
  behind: number | null;
  approvals: { state: string; need: number | null; by: string[]; requested: string[]; can_approve: boolean };
  merge: { can_merge: boolean; can_auto: boolean; auto_enabled: boolean; blockers: string[] };
  jobs: { name: string; status: string; duration: number | null }[];
  threads: ReviewThread[]; files: ReviewFile[]; activity: ReviewActivity[];
}
export type ReviewAction = 'reply' | 'resolve' | 'approve' | 'merge' | 'auto_merge';

export function canMergeReview(review: ReviewDetail): boolean {
  return review.state === 'open' && !review.draft && review.conflicts !== true && review.merge.can_merge
    && (review.pipeline == null || review.pipeline === 'success' || review.pipeline === 'skipped');
}
export function reviewNeedsAttention(review: ReviewSummary): boolean {
  return review.state === 'open' && (review.review_requested || review.mine
    && (review.pipeline === 'failed' || review.conflicts === true || (review.open_threads ?? 0) > 0));
}
export function reviewIdentity(review: ReviewTarget): string { return JSON.stringify([review.host, review.repo, review.number]); }
export function mergeReviewLocations<T extends { review: ReviewSummary }>(rows: readonly T[]): T[] {
  const unique = new Map<string, T>();
  for (const row of rows) {
    const key = row.review.url || reviewIdentity(row.review);
    const previous = unique.get(key);
    if (!previous || !previous.review.sessions.length && row.review.sessions.length) unique.set(key, row);
  }
  return [...unique.values()];
}
export async function readReviewSources(servers: readonly Server[], options: {
  signal?: AbortSignal; previous?: readonly ReviewSource[]; onUpdate?: (sources: ReviewSource[]) => void;
} = {}): Promise<ReviewSource[]> {
  const sources = servers.map((server): ReviewSource => {
    const old = options.previous?.find((item) => item.server.id === server.id && item.server.baseUrl === server.baseUrl && item.server.token === server.token);
    return { server, listing: old?.listing, error: '', loading: true };
  });
  const publish = () => { if (!options.signal?.aborted) options.onUpdate?.([...sources]); };
  publish();
  await Promise.all(sources.map(async (source, index) => {
    try {
      const listing = await getReviewsForServer(source.server, options.signal);
      const previous = new Map(source.listing?.reviews.map((review) => [reviewIdentity(review), review]));
      listing.reviews = listing.reviews.map((review) => {
        const old = previous.get(reviewIdentity(review));
        return old && JSON.stringify(old) === JSON.stringify(review) ? old : review;
      });
      sources[index] = { server: source.server, listing, error: '', loading: false };
    } catch (error) {
      if (options.signal?.aborted) throw error;
      sources[index] = { ...source, error: error instanceof Error ? error.message : m.erro_desconhecido(), loading: false };
    }
    publish();
  }));
  return sources;
}

export async function getReviewsForServer(server: Server, signal?: AbortSignal): Promise<ReviewListing> {
  const listing = await apiFetchForServer<ReviewListing>(server, '/api/reviews', { signal: comTeto(signal, 35_000) }, 35_000);
  if (!Array.isArray(listing.hosts) || !Array.isArray(listing.reviews)) throw new Error(m.native_invalid_response());
  return listing;
}
export async function getReviewForServer(server: Server, target: ReviewTarget, signal?: AbortSignal): Promise<ReviewDetail> {
  const query = new URLSearchParams({ host: target.host, repo: target.repo, number: String(target.number) });
  const detail = await apiFetchForServer<ReviewDetail>(server, `/api/reviews/detail?${query}`, { signal: comTeto(signal, 35_000) }, 35_000);
  if (!detail.merge || !detail.approvals || !Array.isArray(detail.threads) || !Array.isArray(detail.files) || !Array.isArray(detail.activity)) throw new Error(m.native_invalid_response());
  return detail;
}
export async function reviewActionForServer(server: Server, target: ReviewTarget, action: ReviewAction,
  options: { thread?: string; body?: string } = {}, current: () => boolean = () => true): Promise<void> {
  const changed = () => Object.assign(new Error(m.native_request_changed()), { status: 409, code: 'review_context_changed' });
  if (!current()) throw changed();
  if (action === 'merge' || action === 'auto_merge') {
    const detail = await getReviewForServer(server, target);
    const allowed = action === 'merge' ? canMergeReview(detail) : detail.state === 'open' && !detail.draft && detail.merge.can_auto && !detail.merge.auto_enabled;
    if (!allowed) {
      const reason = detail.pipeline === 'running' || detail.pipeline === 'pending' ? m.native_review_block_pipeline_running() : m.native_review_block_checking();
      throw Object.assign(new Error(m.native_review_blocked({ motivos: reason })), { status: 409, code: 'review_not_ready' });
    }
  }
  if (!current()) throw changed();
  const result = await apiFetchForServer<{ ok: boolean }>(server, '/api/reviews/action', {
    method: 'POST', body: JSON.stringify({ host: target.host, repo: target.repo, number: target.number, action, thread: options.thread, body: options.body }),
  }, 90_000);
  if (result?.ok !== true) throw new Error(m.native_invalid_response());
}
