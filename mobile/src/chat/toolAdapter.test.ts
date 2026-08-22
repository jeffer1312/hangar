import { describe, expect, it } from 'vitest';
import { toToolCall, pairTools } from './toolAdapter';
import type { ChatEvent } from '@hangar/core';

const use = { kind: 'tool_use', id: 'a:1', tool_name: 'Bash', tool_input: { command: 'ls' }, tool_use_id: 'tu1', ts: 100 } as unknown as ChatEvent;
const res = { kind: 'tool_result', id: 'a:2', tool_use_id: 'tu1', result: 'ok', is_error: false, ts: 101 } as unknown as ChatEvent;

describe('toolAdapter', () => {
  it('sem resultado = running', () => {
    expect(toToolCall(use).state).toBe('running');
  });
  it('com resultado = completed e result', () => {
    const t = toToolCall(use, res);
    expect(t.state).toBe('completed');
    expect(t.result).toBe('ok');
    expect(t.completedAt).toBe(101000);
  });
  it('erro', () => {
    const err = { ...res, is_error: true } as unknown as ChatEvent;
    expect(toToolCall(use, err).state).toBe('error');
  });
  it('pairTools junta por tool_use_id', () => {
    expect(pairTools([use, res] as unknown as ChatEvent[]).get('tu1')?.result).toBe(res);
  });
  it('pairTools suporta resultado antes do use', () => {
    const m = pairTools([res, use] as unknown as ChatEvent[]);
    expect(m.get('tu1')?.use).toBe(use);
    expect(m.get('tu1')?.result).toBe(res);
  });
});
