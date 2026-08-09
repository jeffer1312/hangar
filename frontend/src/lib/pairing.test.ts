// Validação estrita de pareamento (round 2 da 4b): base absoluta http/https com hostname, api
// válida quando presente, token não vazio, URL sem ?token= recusada — nada chega ao storage.
import { describe, it, expect } from 'vitest';

// auth.ts toca localStorage no load (migrate()); vitest env=node não tem — stub mínimo ANTES do
// import dinâmico (mesmo padrão do auth.test.ts).
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};

const { validarPareamento } = await import('./auth');

describe('validarPareamento', () => {
  it('aceita URL http/https com token', () => {
    expect(validarPareamento('http://host:8765/?token=abc')).toEqual({ base: 'http://host:8765', token: 'abc' });
    expect(validarPareamento('https://host/path?token=xyz')).toEqual({ base: 'https://host', token: 'xyz' });
  });

  it('aceita ?api= absoluto válido como base', () => {
    expect(validarPareamento('http://host/?token=abc&api=https://outro:9999'))
      .toEqual({ base: 'https://outro:9999', token: 'abc' });
  });

  it('recusa protocolo não-http (htp://, ftp://)', () => {
    expect(validarPareamento('htp://host/?token=abc')).toBeNull();
    expect(validarPareamento('ftp://host/?token=abc')).toBeNull();
    expect(validarPareamento('file:///etc/passwd?token=abc')).toBeNull();
  });

  it('recusa URL sem token e token vazio', () => {
    expect(validarPareamento('https://host/sem-token')).toBeNull();
    expect(validarPareamento('https://host/?token=')).toBeNull();
    expect(validarPareamento('https://host/?token=   ')).toBeNull();
  });

  it('recusa api malformada ou vazia', () => {
    expect(validarPareamento('http://host/?token=abc&api=https://')).toBeNull();   // api sem hostname
    expect(validarPareamento('http://host/?token=abc&api=ftp://outro')).toBeNull();
    expect(validarPareamento('http://host/?token=abc&api=nao-e-url')).toBeNull();
  });

  it('api= presente mas vazio falha (não cai na base da URL)', () => {
    expect(validarPareamento('https://front/?token=abc&api=')).toBeNull();
  });

  it('recusa lixo e token cru sem URL', () => {
    expect(validarPareamento('abc123')).toBeNull();
    expect(validarPareamento('')).toBeNull();
    expect(validarPareamento('   ')).toBeNull();
  });

  it('recusa token com whitespace interno (round 4)', () => {
    expect(validarPareamento('https://host/?token=ab%20cd')).toBeNull();   // %20 decodifica p/ espaço
    expect(validarPareamento('https://host/?token=ab+cd')).toBeNull();     // `+` também é espaço
  });

  it('recusa token duplicado e api duplicada (round 4)', () => {
    expect(validarPareamento('https://host/?token=one&token=two')).toBeNull();
    expect(validarPareamento('https://host/?token=abc&api=https://b&api=https://c')).toBeNull();
  });

  it('recusa api com whitespace (round 4)', () => {
    expect(validarPareamento('https://host/?token=abc&api=https://ho%20st')).toBeNull();
  });

  it('token cru sem aceitarTokenCru -> null; com a opção vira { base: "", token }', () => {
    expect(validarPareamento('abc123', { aceitarTokenCru: true })).toEqual({ base: '', token: 'abc123' });
    expect(validarPareamento('  abc123  ', { aceitarTokenCru: true })).toEqual({ base: '', token: 'abc123' });
  });

  it('token cru com espaço NÃO passa nem com aceitarTokenCru (round 4)', () => {
    expect(validarPareamento('ab cd', { aceitarTokenCru: true })).toBeNull();
  });
});
