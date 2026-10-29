import { expect, it } from 'vitest';
import { parseCodexOutput } from './codexCommands';

it('lê retratos completos da saída por item e aceita a limpeza vazia', () => {
  expect(parseCodexOutput('{"exec-a":"primeiro\\nsegundo","exec-b":"outro"}'))
    .toEqual({ 'exec-a': 'primeiro\nsegundo', 'exec-b': 'outro' });
  expect(parseCodexOutput('{}')).toEqual({});
});

it.each(['null', '[]', '"texto"', '{"exec-a":12}', '{'])('recusa saída fora do contrato: %s', (data) => {
  expect(() => parseCodexOutput(data)).toThrow();
});
