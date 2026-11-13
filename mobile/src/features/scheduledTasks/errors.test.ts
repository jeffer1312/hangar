import { expect, it } from 'vitest';
import { scheduledError } from './errors';
import * as m from '../../paraglide/messages';
it('explica a troca de vida do destino e a revisão sem confundir os dois conflitos', () => {
  expect(scheduledError({ status: 409, code: 'scheduled_session_changed' })).toBe(m.scheduled_unavailable_session());
  expect(scheduledError({ status: 409, code: 'scheduled_revision_conflict' })).toBe(m.scheduled_revision_conflict());
});
