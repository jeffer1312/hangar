/**
 * @vitest-environment happy-dom
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import React, { act } from 'react';
import { createRoot } from 'react-dom/client';
import { configureApi } from '@hangar/core';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('react-native-webview', () => ({
  WebView: (props: any) => React.createElement(
    React.Fragment,
    null,
    React.createElement(
      'button',
      { 'data-testid': 'webview-error', onClick: () => props.onError?.({ nativeEvent: { description: '' } }) },
      'WebView error',
    ),
    React.createElement(
      'button',
      {
        'data-testid': 'webview-http-error',
        onClick: () => {
          props.onHttpError?.({ nativeEvent: { statusCode: 401, description: '' } });
          props.onLoadStart?.();
          props.onLoad?.();
        },
      },
      'WebView HTTP error',
    ),
  ),
}));
vi.mock('../../vendor/happy/components/diff/DiffView', () => ({ DiffView: () => null }));
vi.mock('../../paraglide/messages', () => ({
  arquivo_carregar_erro: () => 'Erro ao carregar o arquivo',
  sessao_expirada: () => 'Sessão expirada',
  comum_carregando: () => 'Carregando…',
}));

import { FileViewer } from './FileViewer';

const content = {
  path: 'README.html',
  text: '<h1>teste</h1>',
  size: 15,
  truncated: false,
  digest: 'digest',
};

function renderViewer() {
  const container = document.createElement('div');
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(
      React.createElement(FileViewer, {
        path: 'README.html',
        conteudo: content,
        diff: null,
        loading: false,
        erro: null,
        escopo: 'branch',
        onEscopo: vi.fn(),
        onEditar: vi.fn(),
        name: 'fixture-files',
      }),
    );
  });
  return { container, root };
}

describe('FileViewer — erros de documento', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
    configureApi({
      getBaseUrl: () => 'http://127.0.0.1:8765',
      getToken: () => 'token-de-teste',
      onUnauthorized: vi.fn(),
      origin: null,
      createEventSource: vi.fn() as any,
    });
  });

  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('usa mensagem de arquivo quando onError não informa descrição', async () => {
    const { container, root } = renderViewer();
    await act(async () => {
      (container.querySelector('[data-testid="webview-error"]') as HTMLButtonElement).click();
    });
    expect(container.textContent).toContain('Erro ao carregar o arquivo');
    root.unmount();
  });

  it('usa mensagem de arquivo quando onHttpError 401 não informa descrição', async () => {
    const { container, root } = renderViewer();
    await act(async () => {
      (container.querySelector('[data-testid="webview-http-error"]') as HTMLButtonElement).click();
    });
    expect(container.textContent).toContain('Erro ao carregar o arquivo');
    expect(container.textContent).not.toContain('Falha ao carregar modelos');
    root.unmount();
  });
});
