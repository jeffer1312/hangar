import * as m from '../../paraglide/messages';
export function scheduledError(error: unknown): string {
  const e = error as { status?: number; code?: string; message?: string } | null;
  if (e?.status === 409 && e.code?.includes('revision')) return m.scheduled_revision_conflict();
  if (e?.code?.includes('session') && (e.code.includes('identity') || e.code.includes('unavailable') || e.code.includes('missing') || e.code.includes('changed'))) return m.scheduled_unavailable_session();
  return m.scheduled_operation_failed({ error: e?.message ?? String(error) });
}
