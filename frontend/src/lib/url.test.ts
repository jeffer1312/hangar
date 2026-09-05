import { describe, it, expect } from 'vitest';
import { normalizeBaseUrl, normalizarEndereco } from './url';

describe('normalizeBaseUrl', () => {
  it('prepends http:// when scheme is missing', () => {
    expect(normalizeBaseUrl('localhost:8765')).toBe('http://localhost:8765');
  });
  it('keeps an explicit http scheme', () => {
    expect(normalizeBaseUrl('http://192.168.0.5:8765')).toBe('http://192.168.0.5:8765');
  });
  it('keeps an explicit https scheme', () => {
    expect(normalizeBaseUrl('https://casa.ts.net')).toBe('https://casa.ts.net');
  });
  it('trims whitespace and trailing slashes', () => {
    expect(normalizeBaseUrl('  http://h:1/  ')).toBe('http://h:1');
  });
  it('returns empty string for empty input', () => {
    expect(normalizeBaseUrl('   ')).toBe('');
  });
});

describe('normalizarEndereco', () => {
  it('IP sem porta ganha a porta padrão do backend', () => {
    expect(normalizarEndereco('192.168.0.10')).toEqual({ base: 'http://192.168.0.10:8765', token: null, alternativa: null });
  });
  it('IP com porta fica como veio', () => {
    expect(normalizarEndereco('192.168.0.10:9000')).toEqual({ base: 'http://192.168.0.10:9000', token: null, alternativa: null });
  });
  it('nome de domínio sem esquema vira https sem porta, com http na porta padrão de reserva', () => {
    expect(normalizarEndereco('casa.ts.net')).toEqual({ base: 'https://casa.ts.net', token: null, alternativa: 'http://casa.ts.net:8765' });
  });
  it('nome sem ponto e .local são rede local: http na porta padrão', () => {
    expect(normalizarEndereco('notebook')).toEqual({ base: 'http://notebook:8765', token: null, alternativa: null });
    expect(normalizarEndereco('notebook.local')).toEqual({ base: 'http://notebook.local:8765', token: null, alternativa: null });
    expect(normalizarEndereco('localhost')).toEqual({ base: 'http://localhost:8765', token: null, alternativa: null });
  });
  it('esquema explícito é respeitado, com ou sem porta, e sem reserva', () => {
    expect(normalizarEndereco('http://casa.ts.net')).toEqual({ base: 'http://casa.ts.net', token: null, alternativa: null });
    expect(normalizarEndereco('https://casa.ts.net:8443/')).toEqual({ base: 'https://casa.ts.net:8443', token: null, alternativa: null });
  });
  it('link de pareamento inteiro separa o token e descarta o caminho', () => {
    expect(normalizarEndereco('http://192.168.0.10:8765/?token=abc123')).toEqual({ base: 'http://192.168.0.10:8765', token: 'abc123', alternativa: null });
  });
  it('token duplicado, vazio ou com espaço recusa o endereço', () => {
    expect(normalizarEndereco('http://h:1/?token=a&token=b')).toBeNull();
    expect(normalizarEndereco('http://h:1/?token=')).toBeNull();
    expect(normalizarEndereco('http://h:1/?token=a%20b')).toBeNull();
  });
  it('porta explícita igual à padrão do esquema não é engolida', () => {
    expect(normalizarEndereco('192.168.0.10:80')).toEqual({ base: 'http://192.168.0.10', token: null, alternativa: null });
    expect(normalizarEndereco('casa.ts.net:80')).toEqual({ base: 'http://casa.ts.net', token: null, alternativa: null });
    expect(normalizarEndereco('casa.ts.net:443')).toEqual({ base: 'http://casa.ts.net:443', token: null, alternativa: null });
  });
  it('IPv6 com e sem porta explícita', () => {
    expect(normalizarEndereco('[::1]:9000')).toEqual({ base: 'http://[::1]:9000', token: null, alternativa: null });
    expect(normalizarEndereco('[::1]')).toEqual({ base: 'http://[::1]:8765', token: null, alternativa: null });
  });
  it('lixo recusa', () => {
    expect(normalizarEndereco('')).toBeNull();
    expect(normalizarEndereco('   ')).toBeNull();
    expect(normalizarEndereco('ftp://h')).toBeNull();
    expect(normalizarEndereco('http://')).toBeNull();
  });
});
