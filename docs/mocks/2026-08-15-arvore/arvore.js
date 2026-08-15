// Nós da árvore de exemplo, um lugar só — os três mocks leem daqui.
// [nível, tipo, nome, marca, adicionadas, removidas, selecionado]
const NOS = [
  [0, 'dir',  'backend',                 'M', 46,  9, false, 'aberta'],
  [1, 'dir',  'app',                     'M', 46,  9, false, 'aberta'],
  [2, 'file', 'filetree.py',             'A', 128, 0, false],
  [2, 'file', 'git_ops.py',              'M', 42,  8, true],
  [2, 'file', 'kimi_hook_installer.py',  null, 0,  0, false],
  [1, 'dir',  'tests',                   'M', 96,  1, false, 'fechada'],
  [0, 'dir',  'docs',                    '?', 0,   0, false, 'aberta'],
  [1, 'file', 'pesquisa-c1-c2-terreno.md', '?', 0, 0, false],
  [0, 'dir',  'frontend',                null, 0,  0, false, 'fechada'],
  [0, 'file', 'README.md',               null, 0,  0, false],
];

const ICO_DIR = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';
const ICO_FILE = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>';

// PADRÃO: só o que mudou. O botão de olho é que mostra a árvore inteira.
// Decisão do usuário em 15/08/2026 — quem quer navegar tudo pede; quem acabou de
// trabalhar quer ver o que a sessão mexeu, sem filtrar nada.
function montarArvore(seletor, { recuo = 14, base = 8, tudo = false } = {}) {
  const alvo = document.querySelector(seletor);
  const lista = tudo ? NOS : NOS.filter(([, , , marca]) => marca !== null);
  alvo.innerHTML = lista.map(([nivel, tipo, nome, marca, add, del, sel, estado]) => {
    const chev = tipo === 'dir' ? (estado === 'aberta' ? '▾' : '▸') : '';
    // O par +N −M só aparece onde houve mudança — arquivo intocado fica limpo.
    const num = (add || del)
      ? `<span class="num"><span class="stat-add">+${add}</span> <span class="stat-del">−${del}</span></span>`
      : '<span class="num"></span>';
    const m = marca ? `<span class="marca m-${marca === '?' ? 'Q' : marca}">${marca}</span>` : '<span class="marca"></span>';
    return `<button class="no ${tipo === 'dir' ? 'pasta' : ''} ${sel ? 'sel' : ''}" style="padding-left:${base + nivel * recuo}px">
      <span class="chev">${chev}</span>
      <span class="ico">${tipo === 'dir' ? ICO_DIR : ICO_FILE}</span>
      <span class="nome">${nome}</span>
      ${num}${m}
    </button>`;
  }).join('');
}
