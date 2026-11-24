// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { composeWithContextRefs, type ContextRef } from '@hangar/core';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('../paraglide/messages', () => Object.fromEntries(
  'cite_quoted cite_go_to_source cite_element cite_element_html cite_code'.split(' ').map((k) => [k, () => k]),
));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('./AssistantBubble', () => ({ mkMarkdownStyle: () => ({ paragraph: {} }), mkMutedMarkdownStyle: () => ({ paragraph: {} }) }));
vi.mock('react-native-enriched-markdown', () => ({
  EnrichedMarkdownText: ({ markdown }: { markdown: string }) => createElement('pre', null, markdown),
}));

const { ContextRefView } = await import('./ContextRefView');

const quote: ContextRef = { kind: 'quote', source: 'ev-1', before: '', after: '', text: 'guardado por **sessão**', comment: 'explica' };
const element: ContextRef = {
  kind: 'element', url: 'about:blank', title: '', role: '', name: '', tag: 'div', selector: '', html: '<div></div>', comment: '',
};

const code: ContextRef = {
  kind: 'code', path: 'src/a.rs', start: 7, end: 7, lang: 'rs', text: 'let x = 1;', comment: 'por quê?',
};

function render(props: Record<string, unknown>) {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(ContextRefView, props as never)));
  return { container, root };
}

describe('ContextRefView (app)', () => {
  it('citação, comentário e texto digitado, sem o bloco cru; toque vai à origem', () => {
    const onQuoteClick = vi.fn();
    const { container, root } = render({ text: composeWithContextRefs([quote], 'e isto'), onQuoteClick });
    expect(container.textContent).toContain('cite_quoted');
    expect(container.querySelector('pre')?.textContent).toBe('guardado por **sessão**');
    expect(container.textContent).toContain('explica');
    expect(container.textContent).toContain('e isto');
    expect(container.textContent).not.toContain('<hangar-');
    act(() => container.querySelector<HTMLButtonElement>('[aria-label="cite_go_to_source"]')!.click());
    expect(onQuoteClick).toHaveBeenCalledWith(expect.objectContaining({ source: 'ev-1' }));
    act(() => root.unmount());
  });

  it('linhas de arquivo mostram o nome e a linha, o código e o comentário', () => {
    const { container, root } = render({ text: composeWithContextRefs([code], '') });
    expect(container.textContent).toContain('cite_code');
    expect(container.textContent).toContain('a.rs L7');
    expect(container.textContent).toContain('src/a.rs');
    expect(container.textContent).toContain('let x = 1;');
    expect(container.textContent).toContain('por quê?');
    expect(container.textContent).not.toContain('<hangar-');
    act(() => root.unmount());
  });

  it('elemento mostra o rótulo e o HTML só ao abrir', () => {
    const { container, root } = render({ text: composeWithContextRefs([element], '') });
    expect(container.textContent).toContain('<div>');
    expect(container.textContent).not.toContain('<div></div>');
    const toggle = [...container.querySelectorAll('button')].find((b) => b.textContent === 'cite_element_html')!;
    act(() => toggle.click());
    expect(container.textContent).toContain('<div></div>');
    act(() => root.unmount());
  });
});
