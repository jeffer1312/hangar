const test = require('node:test');
const assert = require('node:assert/strict');
const { compactarAX, parseLote } = require('./preview_fmt.cjs');

test('compactarAX indenta pela profundidade e só numera o que é acionável', () => {
  const nos = [
    { nodeId: '1', role: { value: 'RootWebArea' }, name: { value: 'Exemplo' }, childIds: ['2'], backendDOMNodeId: 1 },
    { nodeId: '2', role: { value: 'button' }, name: { value: 'Salvar' }, childIds: [], backendDOMNodeId: 7 },
  ];
  const { linhas, refs } = compactarAX(nos);
  assert.deepEqual(linhas, ['- RootWebArea "Exemplo"', '  - button "Salvar" [ref=@e1]']);
  assert.equal(refs.get('@e1'), 7);
});

test('compactarAX pula nó ignorado e nó sem nome nem papel útil', () => {
  const nos = [
    { nodeId: '1', role: { value: 'RootWebArea' }, name: { value: '' }, childIds: ['2', '3'] },
    { nodeId: '2', role: { value: 'none' }, name: { value: '' }, childIds: [] },
    { nodeId: '3', role: { value: 'link' }, name: { value: 'Saiba mais' }, childIds: [], backendDOMNodeId: 9, ignored: false },
  ];
  const { linhas, refs } = compactarAX(nos);
  assert.deepEqual(linhas, ['- RootWebArea', '  - link "Saiba mais" [ref=@e1]']);
  assert.equal(refs.size, 1);
});

test('parseLote ignora vazio e comentário e preserva texto com espaço', () => {
  assert.deepEqual(parseLote('click @e1\n\n# comentário\nfill @e2 dois nomes\n'), [
    { verbo: 'click', args: ['@e1'] },
    { verbo: 'fill', args: ['@e2', 'dois nomes'] },
  ]);
});
