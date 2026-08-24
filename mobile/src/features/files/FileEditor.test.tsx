/**
 * @vitest-environment happy-dom
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import React, { act } from 'react';
import { createRoot } from 'react-dom/client';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('../../vendor/happy/components/MultiTextInput', () => import('../../vendor/happy/components/__mocks__/MultiTextInput'));
vi.mock('../../paraglide/messages', () => ({
  arq_salvar: () => 'Salvar',
  arq_salvando: () => 'Salvando…',
  arq_salvo: () => 'Salvo',
  arq_descartar: () => 'Descartar',
  arq_nao_salvo: () => 'alterações não salvas',
}));

import { FileEditor } from './FileEditor';

const roots: Array<{ unmount: () => void }> = [];

async function renderEditor(props: Partial<React.ComponentProps<typeof FileEditor>> = {}) {
  const defaultProps: React.ComponentProps<typeof FileEditor> = {
    path: 'README.md',
    initialText: 'old',
    onSalvar: vi.fn().mockResolvedValue(null),
    onDescartar: vi.fn(),
    ...props,
  };
  const container = document.createElement('div');
  document.body.appendChild(container);
  const root = createRoot(container);
  roots.push(root);
  await act(async () => {
    root.render(React.createElement(FileEditor, defaultProps));
  });
  return { container, props: defaultProps };
}

function editorInput(container: HTMLElement) {
  const input = container.querySelector('textarea[data-testid="editor-input"]');
  expect(input).not.toBeNull();
  return input as HTMLTextAreaElement;
}

function saveButton(container: HTMLElement) {
  const button = Array.from(container.querySelectorAll('button')).find((candidate) => candidate.textContent?.includes('Salvar'));
  expect(button).not.toBeUndefined();
  return button as HTMLButtonElement;
}

async function changeText(container: HTMLElement, text: string) {
  const input = editorInput(container);
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set;
  if (!setter) throw new Error('setter de textarea indisponível');
  await act(async () => {
    setter.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
  });
}

async function flushPromises() {
  await Promise.resolve();
  await Promise.resolve();
}

describe('FileEditor', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    vi.useRealTimers();
  });

  afterEach(() => {
    for (const root of roots.splice(0)) {
      act(() => root.unmount());
    }
    document.body.innerHTML = '';
    vi.useRealTimers();
  });

  it('mostra Salvando enquanto salvar está pendente', async () => {
    let resolveSalvar!: (valor: string | null) => void;
    const onSalvar = vi.fn(() => new Promise<string | null>((resolve) => {
      resolveSalvar = resolve;
    }));
    const { container } = await renderEditor({ onSalvar });

    await changeText(container, 'new');
    expect(container.textContent).toContain('alterações não salvas');

    await act(async () => {
      saveButton(container).click();
      await flushPromises();
    });
    expect(onSalvar).toHaveBeenCalledWith('new');
    expect(container.textContent).toContain('Salvando…');
    expect(container.textContent).not.toContain('Salvo');

    await act(async () => {
      resolveSalvar(null);
      await flushPromises();
    });
    expect(container.textContent).toContain('Salvo');
  });

  it('mostra Salvo por 2 segundos sem desmontar o editor', async () => {
    vi.useFakeTimers();
    const onSalvar = vi.fn().mockResolvedValue(null);
    const { container } = await renderEditor({ onSalvar });

    await changeText(container, 'new');
    await act(async () => {
      saveButton(container).click();
      await flushPromises();
    });

    expect(container.textContent).toContain('Salvo');
    expect(editorInput(container)).toBeTruthy();
    expect(container.textContent).toContain('Descartar');

    await act(async () => {
      vi.advanceTimersByTime(1999);
    });
    expect(container.textContent).toContain('Salvo');

    await act(async () => {
      vi.advanceTimersByTime(1);
    });
    expect(container.textContent).not.toContain('Salvo');
  });

  it('mostra a mensagem de erro e não mostra Salvo quando salvar falha', async () => {
    const onSalvar = vi.fn().mockResolvedValue('erro_arq_mudou_no_disco');
    const { container } = await renderEditor({ onSalvar });

    await changeText(container, 'new');
    await act(async () => {
      saveButton(container).click();
      await flushPromises();
    });

    expect(container.textContent).toContain('erro_arq_mudou_no_disco');
    expect(container.textContent).not.toContain('Salvo');
    expect(container.textContent).not.toContain('Salvando…');
  });

  it('mantém editor e descarte montados depois do sucesso', async () => {
    const onSalvar = vi.fn().mockResolvedValue(null);
    const onDescartar = vi.fn();
    const { container } = await renderEditor({ onSalvar, onDescartar });

    await changeText(container, 'new');
    await act(async () => {
      saveButton(container).click();
      await flushPromises();
    });

    expect(editorInput(container)).toBeTruthy();
    const discard = Array.from(container.querySelectorAll('button')).find((candidate) => candidate.textContent?.includes('Descartar'));
    expect(discard).not.toBeUndefined();
    expect(onDescartar).not.toHaveBeenCalled();
  });
});
