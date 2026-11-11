import { expect, test } from 'vitest';
import type { ChatEvent } from '@hangar/core';
import { subagentRunning, turnStart, workingLabel } from './liveWork';

test('rótulo do terminal: texto, o tempo que o terminal já conta e os tokens', () => {
  expect(workingLabel('Sketching… (6s · esc to interrupt)')).toEqual({ text: 'Sketching…', elapsed: '6s', detail: null });
  expect(workingLabel('Bash: T=$(grep x) | tr … (5m 11s · ↓ 5.5k tokens · thought for 50s)'))
    .toEqual({ text: 'Bash: T=$(grep x) | tr …', elapsed: '5m 11s', detail: '↓ 5.5k tokens · thought for 50s' });
  expect(workingLabel('Moonwalking… (1m 29s · ↓ 6.0k tokens)')).toEqual({ text: 'Moonwalking…', elapsed: '1m 29s', detail: '↓ 6.0k tokens' });
  expect(workingLabel('Writing tests…')).toEqual({ text: 'Writing tests…', elapsed: null, detail: null });
  expect(workingLabel('Running (esc to interrupt)')).toEqual({ text: 'Running', elapsed: null, detail: null });
  expect(workingLabel(null)).toEqual({ text: null, elapsed: null, detail: null });
});

test('começo do turno: o mais novo entre o envio gravado e a virada vista; fila não conta', () => {
  const ev = (id: string, ts: number): ChatEvent => ({ id, kind: 'user_msg', text: 'x', ts });
  expect(turnStart([ev('u1', 100)], null)).toBe(100_000);
  expect(turnStart([ev('u1', 100)], 150_000)).toBe(150_000);
  expect(turnStart([ev('u1', 100), ev('queued-1', 200)], null)).toBe(100_000);
  expect(turnStart([], null)).toBeNull();
});

test('subagente: o próprio arquivo vence o pai; ilegível não afirma estado', () => {
  expect(subagentRunning({ finished: true }, true)).toBe(false);
  expect(subagentRunning({}, false)).toBe(false);
  expect(subagentRunning({}, null)).toBeNull();
  expect(subagentRunning({ ilegivel: true, finished: false }, true)).toBeNull();
});
