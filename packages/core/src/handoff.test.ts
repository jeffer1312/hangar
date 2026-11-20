import { describe, expect, it } from 'vitest';
import { modelDisplay } from './handoff';

describe('modelDisplay', () => {
  it('lê os modelos como o divisor do T3 Code', () => {
    expect(modelDisplay('claude', 'claude-opus-5-5')).toBe('Opus 5.5');
    expect(modelDisplay('claude', 'opus')).toBe('Opus');
    expect(modelDisplay('claude', 'claude-sonnet-4-5-20250929')).toBe('Sonnet 4.5');
    expect(modelDisplay('claude', 'claude-opus-5-5[1m]')).toBe('Opus 5.5 1M');
    expect(modelDisplay('codex', 'gpt-6.2-sol')).toBe('GPT-6.2-Sol');
  });

  it('sem modelo mostra o agente', () => {
    expect(modelDisplay('codex', '')).toBe('Codex');
    expect(modelDisplay('claude', null)).toBe('Claude');
  });
});
