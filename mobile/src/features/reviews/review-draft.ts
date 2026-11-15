import { fetchSessionsForServer, type ReviewDetail, type ReviewThread, type Server } from '@hangar/core';
import { resolveDraftTranscript, writeDraft } from '../../stores/drafts';
import * as m from '../../paraglide/messages';

export function reviewThreadPrompt(detail: ReviewDetail, threads: ReviewThread[]): string {
  const number = `${detail.provider === 'gitlab' ? '!' : '#'}${detail.number}`;
  return [m.native_review_send_intro({ numero: number, titulo: detail.title }), detail.url,
    ...threads.map((thread) => [thread.file ? `${thread.file}${thread.line ? `:${thread.line}` : ''}` : m.native_review_thread_general(),
      thread.code, ...thread.comments.map((comment) => `${comment.author}:\n${comment.body}`)].filter(Boolean).join('\n\n'))].join('\n\n');
}

/** Acrescenta ao rascunho existente; enviar à sessão continua sendo o botão do compositor. */
export async function prepareReviewDraft(server: Server, name: string, text: string, current: () => boolean): Promise<void> {
  const sessions = await fetchSessionsForServer(server);
  const session = sessions.find((item) => item.name === name);
  if (!session || !current()) throw new Error(m.native_request_changed());
  const { draft, recoverable } = resolveDraftTranscript(server.id, name, session.jsonl ?? null);
  if (recoverable) throw new Error(m.mobile_review_draft_changed());
  const base = draft ?? { version: 1 as const, text: '', revision: 0, transcript: session.jsonl ?? null, attachments: [], submission: null };
  writeDraft(server.id, name, { ...base, text: [base.text.trimEnd(), text].filter(Boolean).join('\n\n'), revision: base.revision + 1 });
}
