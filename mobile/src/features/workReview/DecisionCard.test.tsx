// @vitest-environment happy-dom
import { act, createElement, forwardRef, useImperativeHandle } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { WorkReview, WorkReviewChoices } from '@hangar/core';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const focus = vi.hoisted(() => vi.fn());
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/MultilineInput', () => ({
  MultilineInput: forwardRef((p: { value: string }, ref) => {
    useImperativeHandle(ref, () => ({ focus }));
    return createElement('textarea', { value: p.value, readOnly: true });
  }),
}));
vi.mock('./ZoomFrame', () => ({ ZoomFrame: () => null }));

import { DecisionCard } from './DecisionCard';

const review: WorkReview = {
  id: 'r-1', title: 'Revisão', updated_at: '', state: 'waiting', links: [],
  decisions: ['a', 'b', 'c'].map((id) => ({
    id, title: `Decisão ${id}`,
    choices: [{ id: 'igualar', label: 'Igualar' }, { id: 'manter', label: 'Manter', requires_reason: true }],
  })),
};

let root: Root;
let container: HTMLDivElement;
const onIndex = vi.fn();
beforeEach(() => {
  vi.useFakeTimers();
  onIndex.mockReset();
  focus.mockReset();
  container = document.createElement('div');
  root = createRoot(container);
});
afterEach(() => { act(() => root.unmount()); vi.useRealTimers(); });

const render = (index: number, choices: WorkReviewChoices = {}) => act(() => root.render(createElement(DecisionCard, {
  review, choices, index, sending: false, sendError: null, asset: (asset: string) => ({ uri: asset, headers: {} }),
  onIndex, onPick: vi.fn(), onReason: vi.fn(), onSend: vi.fn(), onCompare: vi.fn(), onAssetError: vi.fn(),
})));
const click = (label: string) => act(() => [...container.querySelectorAll('button')]
  .find((b) => b.textContent?.includes(label) || b.getAttribute('aria-label') === label)!.click());

describe('DecisionCard', () => {
  it('voltar logo depois de escolher cancela o avanço para a próxima', () => {
    render(1);
    click('Igualar');
    click(m.work_review_previous_short());
    expect(onIndex).toHaveBeenLastCalledWith(0);
    act(() => { vi.advanceTimersByTime(250); });
    expect(onIndex).toHaveBeenCalledTimes(1);
  });

  it('o avanço não dispara depois de o cartão sair da tela', () => {
    render(0);
    click('Igualar');
    act(() => root.unmount());
    root = createRoot(container);
    act(() => { vi.advanceTimersByTime(250); });
    expect(onIndex).not.toHaveBeenCalled();
  });

  it('escolha sem motivo avança depois do respiro', () => {
    render(0);
    click('Igualar');
    act(() => { vi.advanceTimersByTime(250); });
    expect(onIndex).toHaveBeenCalledWith(1);
  });

  it('o campo do motivo recebe o foco quando monta para a escolha que o pede', () => {
    render(0);
    click('Manter');
    expect(focus).not.toHaveBeenCalled();
    render(0, { a: { choice: 'manter', reason: '' } });
    expect(focus).toHaveBeenCalledTimes(1);
    // Voltar a esta decisão depois não rouba o foco.
    render(1);
    render(0, { a: { choice: 'manter', reason: '' } });
    expect(focus).toHaveBeenCalledTimes(1);
  });
});
