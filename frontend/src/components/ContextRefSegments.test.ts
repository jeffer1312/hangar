// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import { composeWithContextRefs, type ContextRef } from '@hangar/core';
import ContextRefSegments from './ContextRefSegments.svelte';
import UserBubble from './UserBubble.svelte';
import * as m from '../paraglide/messages';

const quote: ContextRef = { kind: 'quote', source: 'ev-1', before: '', after: '', text: 'guardado por **sessão**', comment: 'explica isto' };
const element: ContextRef = {
  kind: 'element', url: 'http://127.0.0.1:5173/login', title: 'Entrar', role: 'button', name: 'Entrar agora',
  tag: 'button', selector: 'form > button', html: '<button>Entrar agora</button>', comment: '',
};

const code: ContextRef = {
  kind: 'code', path: 'desktop-native/src/app/files.rs', start: 12, end: 14, lang: 'rs', text: 'fn a() {\n    b();\n}', comment: 'renomeie',
};

function montar(Comp: typeof ContextRefSegments | typeof UserBubble, props: Record<string, unknown>) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const comp = mount(Comp, { target, props: props as never });
  return { target, comp };
}

describe('ContextRefSegments', () => {
  it('citação em destaque, comentário fora dela, texto digitado sem o bloco', () => {
    const { target, comp } = montar(ContextRefSegments, { text: composeWithContextRefs([quote], 'e mais isto') });
    const bq = target.querySelector('blockquote')!;
    expect(bq.textContent).toContain(m.cite_quoted());
    expect(bq.querySelector('strong')?.textContent).toBe('sessão');
    expect(bq.textContent).not.toContain('explica isto');
    expect(target.querySelector('.cref-comment')?.textContent).toBe('explica isto');
    expect(target.querySelector('.cref-typed')?.textContent).toBe('e mais isto');
    expect(target.textContent).not.toContain('<hangar-');
    unmount(comp);
  });

  it('toque na citação chama com a referência; sem source fica estática', async () => {
    const onQuoteClick = vi.fn();
    const { target, comp } = montar(ContextRefSegments, { text: composeWithContextRefs([quote], ''), onQuoteClick });
    target.querySelector<HTMLElement>('[role="button"]')!.click();
    await tick();
    expect(onQuoteClick).toHaveBeenCalledWith(expect.objectContaining({ source: 'ev-1', text: quote.text }));
    unmount(comp);
    const semFonte = montar(ContextRefSegments, { text: composeWithContextRefs([{ ...quote, source: '' }], ''), onQuoteClick });
    expect(semFonte.target.querySelector('[role="button"]')).toBeNull();
    unmount(semFonte.comp);
  });

  it('elemento vira cartão com papel, nome, url e HTML recolhido', () => {
    const { target, comp } = montar(ContextRefSegments, { text: composeWithContextRefs([element], '') });
    expect(target.querySelector('.cref-el-name')?.textContent).toBe('button "Entrar agora"');
    expect(target.querySelector('.cref-el-url')?.textContent).toBe('http://127.0.0.1:5173/login');
    const det = target.querySelector('details')!;
    expect(det.open).toBe(false);
    expect(det.querySelector('pre')?.textContent).toBe('<button>Entrar agora</button>');
    unmount(comp);
  });
});

describe('ContextRefSegments: linhas de arquivo', () => {
  it('cartão com o nome e as linhas, o caminho, o código cru e o comentário fora', () => {
    const { target, comp } = montar(ContextRefSegments, { text: composeWithContextRefs([code], '') });
    expect(target.querySelector('.cref-code .cref-head')?.textContent).toBe(m.cite_code());
    expect(target.querySelector('.cref-code-name')?.textContent).toBe('files.rs L12–L14');
    expect(target.querySelector('.cref-code .cref-el-url')?.textContent).toBe('desktop-native/src/app/files.rs');
    expect(target.querySelector('.cref-code-text')?.textContent).toBe('fn a() {\n    b();\n}');
    expect(target.querySelector('.cref-comment')?.textContent).toBe('renomeie');
    unmount(comp);
  });
});

describe('UserBubble com citação', () => {
  it('mostra os segmentos no lugar do texto cru', () => {
    const { target, comp } = montar(UserBubble, { text: composeWithContextRefs([quote], 'oi') });
    expect(target.querySelector('blockquote')).not.toBeNull();
    expect(target.textContent).not.toContain('<hangar-ref');
    unmount(comp);
  });

  it('mensagem comum continua texto puro', () => {
    const { target, comp } = montar(UserBubble, { text: 'só **texto**' });
    expect(target.querySelector('blockquote')).toBeNull();
    expect(target.querySelector('.bubble-text')?.textContent).toBe('só **texto**');
    unmount(comp);
  });
});
