import { describe, expect, it } from 'vitest';
import { loopBadge } from './loop';

describe('loopBadge', () => {
  it('null quando sem loop ou terminal silencioso', () => {
    expect(loopBadge(null)).toBeNull();
    expect(loopBadge(undefined)).toBeNull();
  });
  it('running mostra N/M', () => {
    expect(loopBadge('running', 3, 10)).toEqual({ label: '↻ 3/10', tone: 'ok' });
  });
  it('paused_awaiting e done_claimed pedem atenção', () => {
    expect(loopBadge('paused_awaiting', 2, 10)?.tone).toBe('attention');
    expect(loopBadge('done_claimed', 2, 10)?.tone).toBe('attention');
  });
  it('exhausted/failed avisam', () => {
    expect(loopBadge('exhausted', 10, 10)?.tone).toBe('warn');
    expect(loopBadge('failed', 1, 10)?.tone).toBe('warn');
  });
  it('done é ok, stopped é muted', () => {
    expect(loopBadge('done', 4, 10)?.tone).toBe('ok');
    expect(loopBadge('stopped', 4, 10)?.tone).toBe('muted');
  });
  it('null quando status desconhecido (payload de peer em versão diferente)', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- runtime guard contra valor fora do union
    expect(loopBadge('bogus' as any)).toBeNull();
  });
});
