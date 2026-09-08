// @vitest-environment happy-dom
// `escapes`: uma pergunta que veio de um SELETOR DE TUI (Codex sem thread, picker do pane) só tem
// as opções — "Digitar resposta" e "Conversar sobre isso" mandariam texto por um caminho que
// aquela sessão não tem. A primeira versão desligava as saídas com `{#if !textOpen && escapes}`,
// e o `{:else}` desenhava justamente o campo de texto: o oposto do pedido, e sem teste nenhum
// dizendo isso.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import AskQuestionStepper from './AskQuestionStepper.svelte';
import type { AskQuestionPayload } from '@hangar/core';

const payload: AskQuestionPayload = {
  questions: [{
    header: 'Codex',
    question: 'Confia nos hooks?',
    multiSelect: false,
    options: [{ label: 'Revisar', description: '' }, { label: 'Confiar', description: '' }],
  }],
};

function montar(escapes: boolean) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(AskQuestionStepper, {
    target: el,
    props: { open: true, payload, onSubmit: vi.fn(), onClose: vi.fn(), escapes },
  });
  return { el, comp };
}

describe('AskQuestionStepper: saídas de texto', () => {
  it('escapes=false não desenha campo de texto nem as saídas', () => {
    const { el, comp } = montar(false);
    expect(el.querySelector('input[type="text"]')).toBeNull();
    expect(el.querySelectorAll('.option-btn').length).toBe(2);   // as opções continuam
    unmount(comp);
  });

  it('escapes=true (o padrão do AskUserQuestion) mantém as duas saídas', () => {
    const { el, comp } = montar(true);
    expect(el.querySelectorAll('.escapes .ghost-btn').length).toBe(2);
    expect(el.querySelector('input[type="text"]')).toBeNull();   // só depois de clicar em digitar
    unmount(comp);
  });
});
