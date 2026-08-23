import { describe, it, expect } from 'vitest';
import { buildAnswers, type PickState } from './buildAnswers';
import type { AskQuestionItem } from '@hangar/core';

function q(
  header: string,
  question: string,
  multiSelect: boolean,
  labels: string[],
): AskQuestionItem {
  return {
    header,
    question,
    multiSelect,
    options: labels.map((label) => ({ label, description: '', preview: undefined })),
  };
}

describe('buildAnswers', () => {
  it('monta payload com 3 tipos: option, text e chat', () => {
    const questions: AskQuestionItem[] = [
      q('H1', 'Q1?', false, ['A', 'B', 'C']),
      q('H2', 'Q2?', false, ['X', 'Y']),
      q('H3', 'Q3?', true, ['M', 'N']),
    ];
    const picks: PickState[] = [
      { kind: 'option', indices: [0, 2] },
      { kind: 'text', value: 'minha resposta' },
      { kind: 'chat' },
    ];
    const ans = buildAnswers(questions, picks);

    // opção single/multi: indices + labels
    expect(ans[0]).toEqual({ kind: 'option', indices: [0, 2], multi: false, labels: ['A', 'C'] });
    // texto livre: type_index === options.length
    expect(ans[1]).toEqual({ kind: 'text', value: 'minha resposta', type_index: 2, labels: ['minha resposta'] });
    expect((ans[1] as { type_index: number }).type_index).toBe(questions[1].options.length);
    // conversar: chat_index === options.length + 1
    expect(ans[2]).toEqual({ kind: 'chat', chat_index: 3 });
    expect((ans[2] as { chat_index: number }).chat_index).toBe(questions[2].options.length + 1);
  });

  it('usa fallback option vazio quando pick ausente', () => {
    const questions: AskQuestionItem[] = [q('H', 'Q?', false, ['A'] )];
    const ans = buildAnswers(questions, []);
    expect(ans[0]).toEqual({ kind: 'option', indices: [], multi: false, labels: [] });
  });

  it('preserva labels dos indices escolhidos', () => {
    const questions: AskQuestionItem[] = [q('H', 'Q?', true, ['alpha', 'beta', 'gamma'])];
    const picks: PickState[] = [{ kind: 'option', indices: [1, 2] }];
    const ans = buildAnswers(questions, picks);
    expect(ans[0]).toMatchObject({ labels: ['beta', 'gamma'], indices: [1, 2], multi: true });
  });
});
