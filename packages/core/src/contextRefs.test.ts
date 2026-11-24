import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import fixtures from './contextRefs.fixtures.json';
import {
  ELEMENT_NOTE, QUOTE_NOTE, buildQuoteRef, composeWithContextRefs, contextRefLabel, contextRefsPlainText,
  hasContextRefs, locateInMarkdown, locateQuote, parseContextRefs, quoteAffixesInMarkdown, quoteAffixes, quoteTooLong, serializeContextRef,
  splitContextRefs, withContextRefComment, type ContextRef, type QuoteRef,
} from './contextRefs';
import { parseContextRefs as parsePluginContextRefs } from '../../../plugins/hangar/hooks/contextRefs';

describe('contextRefs: casos compartilhados', () => {
  it('as notas são as do contrato', () => {
    expect(QUOTE_NOTE).toBe(fixtures.quoteNote);
    expect(ELEMENT_NOTE).toBe(fixtures.elementNote);
  });

  for (const c of fixtures.serialize) {
    it(`monta: ${c.name}`, () => {
      expect(composeWithContextRefs(c.refs as ContextRef[], c.typed)).toBe(c.message);
    });
    it(`lê de volta: ${c.name}`, () => {
      expect(parseContextRefs(c.message)).toEqual(c.segments);
    });
  }

  for (const c of fixtures.parse) {
    it(`lê: ${c.name}`, () => {
      expect(parseContextRefs(c.message)).toEqual(c.segments);
    });
  }

  for (const c of fixtures.labels) {
    it(`rótulo: ${c.label}`, () => {
      expect(contextRefLabel(c.ref as ContextRef)).toBe(c.label);
    });
  }
});

// O plugin guarda uma cópia dos casos em .ts (o kit de teste dele não lê JSON): ela não pode divergir calada.
describe('contextRefs: cópia dos casos no plugin', () => {
  it('plugins/hangar/hooks/contextRefs.fixtures.ts exporta o mesmo objeto do JSON', () => {
    const raiz = join(import.meta.dirname, '..', '..', '..');
    const json = JSON.parse(readFileSync(join(raiz, 'packages/core/src/contextRefs.fixtures.json'), 'utf8'));
    const ts = readFileSync(join(raiz, 'plugins/hangar/hooks/contextRefs.fixtures.ts'), 'utf8');
    const inicio = /^export const fixtures = /m.exec(ts);
    expect(inicio, 'a cópia não tem `export const fixtures = `').not.toBeNull();
    const corpo = ts.slice(inicio!.index + inicio![0].length).replace(/;\s*$/, '');
    expect(JSON.parse(corpo)).toEqual(json);
  });

  // O kit de teste do plugin só roda dentro do Claude Code; o leitor dele é TypeScript puro.
  for (const c of [...fixtures.parse, ...fixtures.serialize]) {
    it(`o leitor do plugin lê: ${c.name}`, () => {
      expect(parsePluginContextRefs(c.message)).toEqual(c.segments);
    });
  }
});

const quote = (extra: Partial<QuoteRef> = {}): QuoteRef =>
  ({ kind: 'quote', source: 'ev-1', before: '', after: '', text: 'trecho', comment: '', ...extra });

describe('contextRefs: montagem', () => {
  it('sem citação, só o texto digitado sem bordas', () => {
    expect(composeWithContextRefs([], '  oi  ')).toBe('oi');
  });

  it('source fora do formato sai do bloco', () => {
    expect(serializeContextRef(quote({ source: 'com espaço' }))).not.toContain('source=');
    expect(serializeContextRef(quote({ source: 'x'.repeat(201) }))).not.toContain('source=');
  });

  it('comentário só de espaços não vira filho', () => {
    expect(serializeContextRef(quote({ comment: '   ' }))).not.toContain('<hangar-comment>');
  });

  it('buildQuoteRef corta prefixo e sufixo em 32 code points e recusa trecho vazio ou longo', () => {
    const r = buildQuoteRef({ source: 'ev', text: 'x', before: `${'a'.repeat(40)}😀`, after: `😀${'b'.repeat(40)}` });
    expect(Array.from(r!.before)).toHaveLength(32);
    expect(r!.before.endsWith('😀')).toBe(true);
    expect(Array.from(r!.after)).toHaveLength(32);
    expect(r!.after.startsWith('😀')).toBe(true);
    expect(buildQuoteRef({ source: 'ev', text: '  \n ' })).toBeNull();
    expect(buildQuoteRef({ source: 'ev', text: 'x'.repeat(8001) })).toBeNull();
    expect(buildQuoteRef({ source: 'ev', text: 'x'.repeat(8000) })).not.toBeNull();
    expect(buildQuoteRef({ source: 'id inválido', text: 'x' })!.source).toBe('');
  });

  it('quoteTooLong conta code points, não unidades UTF-16', () => {
    expect(quoteTooLong('😀'.repeat(8000))).toBe(false);
    expect(quoteTooLong('😀'.repeat(8001))).toBe(true);
  });

  it('quoteAffixes pega o contexto em volta do trecho', () => {
    expect(quoteAffixes('antes TRECHO depois', 6, 12)).toEqual({ before: 'antes ', after: ' depois' });
  });

  it('withContextRefComment troca só o comentário', () => {
    const r = withContextRefComment(quote(), 'novo');
    expect(r).toEqual(quote({ comment: 'novo' }));
  });
});

describe('contextRefs: volta', () => {
  const msg = composeWithContextRefs([quote({ text: 'guardado por sessão' })], 'explica');

  it('hasContextRefs e texto de uma linha', () => {
    expect(hasContextRefs(msg)).toBe(true);
    expect(hasContextRefs('sem bloco')).toBe(false);
    expect(contextRefsPlainText(msg)).toBe('“guardado por sessão” explica');
    expect(contextRefsPlainText('nada aqui')).toBe('nada aqui');
    expect(contextRefsPlainText(msg.slice(0, 60))).toBe('…');
  });

  it('splitContextRefs desmonta o formato do app e recusa bloco no meio do texto', () => {
    expect(splitContextRefs(msg)).toEqual({ refs: [quote({ text: 'guardado por sessão' })], typed: 'explica' });
    expect(splitContextRefs(`antes\n${msg}`)).toBeNull();
    expect(splitContextRefs('sem bloco')).toBeNull();
  });

  it('bloco torto antes de um válido não engole o válido', () => {
    const torto = '<hangar-ref kind="quote" v="1">\nsem fim\n';
    const segs = parseContextRefs(`${torto}${msg}`);
    expect(segs[0]).toEqual({ text: torto });
    expect('ref' in segs[1]).toBe(true);
  });

  it('nome de tag em maiúsculas não é bloco', () => {
    const upper = msg.replace('<hangar-ref', '<HANGAR-REF');
    expect(parseContextRefs(upper)).toEqual([{ text: upper }]);
  });
});

describe('locateQuote', () => {
  it('desempata pelo contexto', () => {
    const hay = 'o valor é 10. depois o valor é 10 de novo';
    expect(locateQuote(hay, 'valor é 10', 'depois o ', ' de novo')).toBe(hay.lastIndexOf('valor é 10'));
    expect(locateQuote(hay, 'valor é 10', 'o ', '. depois')).toBe(hay.indexOf('valor é 10'));
  });

  it('sem ocorrência devolve -1', () => {
    expect(locateQuote('abc', 'z')).toBe(-1);
    expect(locateQuote('abc', '')).toBe(-1);
  });
});

describe('trecho desenhado na fonte Markdown', () => {
  const src = 'O cache é guardado por **sessão** e expira em 1 h.';

  it('acha o trecho sem os marcadores e devolve prefixo e sufixo da fonte', () => {
    const at = locateInMarkdown(src, 'guardado por sessão')!;
    expect(src.slice(at.start, at.end)).toBe('guardado por **sessão');
    expect(quoteAffixesInMarkdown(src, 'guardado por sessão')).toEqual({ before: 'O cache é ', after: '** e expira em 1 h.' });
  });

  it('quebra de linha da seleção não atrapalha; trecho ausente fica sem contexto', () => {
    expect(locateInMarkdown('linha um\nlinha dois', 'um linha')).not.toBeNull();
    expect(quoteAffixesInMarkdown(src, 'não existe')).toEqual({ before: '', after: '' });
    expect(locateInMarkdown(src, '  ')).toBeNull();
  });
});
