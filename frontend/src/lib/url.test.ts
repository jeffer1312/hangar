import { describe, it, expect } from 'vitest';
import { normalizeBaseUrl } from './url';

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
