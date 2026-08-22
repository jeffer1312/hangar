import { parsePairing } from './pairing';

test.each([
  ['http://192.168.0.10:8765/?token=abc', { base: 'http://192.168.0.10:8765', token: 'abc' }],
  ['http://casa.ts.net/?token=abc&api=http://100.64.0.1:8765', { base: 'http://100.64.0.1:8765', token: 'abc' }],
  ['http://x/?token=', null],
  ['http://x/?token=a&token=b', null],
  ['abc', null],
  ['ftp://x/?token=a', null],
  ['http://x/?token=a b', null],
  ['http:///?token=a', null],
])('%s', (entrada, esperado) => expect(parsePairing(entrada)).toEqual(esperado));
