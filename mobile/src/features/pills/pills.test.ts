import { pillLabels, reconcileChosen, semEsforco } from './pills';
import type { StatusFields } from '@hangar/core';

function fakeStatus(over: Partial<StatusFields> = {}): StatusFields {
  return { raw: 'fake', model: 'Opus 5', effort: 'high', ...over };
}

test('pillLabels devolve chosen quando presente senão status', () => {
  const f = fakeStatus({ model: 'Opus', effort: 'high' });
  expect(pillLabels(f, { model: 'Sonnet', effort: null })).toEqual({ model: 'Sonnet', effort: 'high' });
  expect(pillLabels(f, {})).toEqual({ model: 'Opus', effort: 'high' });
  expect(pillLabels(null, { model: 'K3' })).toEqual({ model: 'K3', effort: null });
});

test('reconcileChosen zera model quando status confirma', () => {
  const f = fakeStatus({ model: 'Opus 5' });
  expect(reconcileChosen(f, { model: 'opus', effort: 'high' })).toEqual({ model: null, effort: 'high' });
  expect(reconcileChosen(f, { model: 'sonnet', effort: 'high' })).toEqual({ model: 'sonnet', effort: 'high' });
  expect(reconcileChosen(null, { model: 'opus' })).toEqual({ model: 'opus' });
});

test('semEsforco detecta haiku case-insensitive', () => {
  expect(semEsforco('Haiku')).toBe(true);
  expect(semEsforco('claude-haiku-3.5')).toBe(true);
  expect(semEsforco('Opus')).toBe(false);
  expect(semEsforco(null)).toBe(false);
  expect(semEsforco(undefined)).toBe(false);
});
