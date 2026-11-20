import { describe, expect, it } from 'vitest';
import { handoffTitle, modelDisplay } from './handoff';
import type { SessionHandoff } from './types';

describe('modelDisplay', () => {
  it('lê os modelos como o divisor do T3 Code', () => {
    expect(modelDisplay('claude', 'claude-opus-5-5')).toBe('Opus 5.5');
    expect(modelDisplay('claude', 'opus')).toBe('Opus');
    expect(modelDisplay('claude', 'claude-opus-5-5[1m]')).toBe('Opus 5.5 1M');
    expect(modelDisplay('codex', 'gpt-6.2-sol')).toBe('GPT-6.2-Sol');
  });

  it('sem modelo mostra o agente', () => {
    expect(modelDisplay('codex', '')).toBe('Codex');
    expect(modelDisplay('claude', null)).toBe('Claude');
  });
});

describe('handoffTitle', () => {
  const handoff = (state: SessionHandoff['state'], code?: string): SessionHandoff => ({ state, provider: 'codex', conta: 'codex:a', code });

  it('distingue espera com a sessão trabalhando, troca em curso e falha', () => {
    const waiting = handoffTitle(handoff('waiting'), 'idle');
    expect(handoffTitle(handoff('waiting'), 'working')).not.toBe(waiting);
    expect(handoffTitle(handoff('running'), 'working')).not.toBe(waiting);
  });

  it('a falha traz o motivo traduzido, ou o código quando ele não tem frase', () => {
    expect(handoffTitle(handoff('failed', 'codigo_sem_frase_xyz'), 'idle')).toContain('codigo_sem_frase_xyz');
    expect(handoffTitle(handoff('failed', 'session_transfer_busy'), 'idle')).not.toContain('session_transfer_busy');
  });
});
