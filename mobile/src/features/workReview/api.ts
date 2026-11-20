import { apiFetchForServer, baseOf, fileAuthHeader, parseWorkReviewDecideResponse, parseWorkReviewResponse } from '@hangar/core';
import type { Server, WorkReviewDecideResponse, WorkReviewDecisionsBody, WorkReviewResponse } from '@hangar/core';

const route = (name: string) => `/api/sessions/${encodeURIComponent(name)}/work-review`;
// O servidor pode precisar acordar o conector antes de responder: o teto padrão de 8 s cortava a primeira leitura.
const READ_TIMEOUT_MS = 30_000;
const DECIDE_TIMEOUT_MS = 60_000;

const invalid = () => Object.assign(new Error('invalid_response'), { code: 'invalid_response' });

export async function fetchWorkReview(server: Server, name: string): Promise<WorkReviewResponse> {
  const parsed = parseWorkReviewResponse(await apiFetchForServer<unknown>(server, route(name), undefined, READ_TIMEOUT_MS));
  if (!parsed) throw invalid();
  return parsed;
}

export async function sendWorkReviewDecisions(server: Server, name: string, body: WorkReviewDecisionsBody): Promise<WorkReviewDecideResponse> {
  const parsed = parseWorkReviewDecideResponse(await apiFetchForServer<unknown>(server, `${route(name)}/decisions`,
    { method: 'POST', body: JSON.stringify(body) }, DECIDE_TIMEOUT_MS));
  if (!parsed) throw invalid();
  return parsed;
}

/** Imagem da revisão: o token vai no cabeçalho, como nas imagens da conversa; a URL muda com o `review_id`. */
export function workReviewAssetSource(server: Server, name: string, reviewId: string, asset: string): { uri: string; headers: Record<string, string> } {
  return {
    uri: `${baseOf(server)}${route(name)}/assets/${encodeURIComponent(asset)}?review_id=${encodeURIComponent(reviewId)}`,
    headers: fileAuthHeader(server),
  };
}

export const isStaleReview = (e: unknown) => {
  const { status, code } = (e ?? {}) as { status?: unknown; code?: unknown };
  return code === 'stale_review' || (status === 409 && code == null);
};
