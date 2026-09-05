// Lógica PURA do hangar-preview (o preview_ctl.cjs é todo efeito: CDP, fila, buffers). Separado
// pra rodar no `node --test` sem bootar Electron — mesmo padrão do navegador.cjs.

// Papéis que não carregam informação pro agente: só estruturam o desenho.
const MUDOS = new Set(['none', 'generic', 'presentation', 'InlineTextBox', 'LineBreak']);
// Papéis que o agente pode acionar; só esses ganham ref, pra o número não virar ruído.
const ACIONAVEIS = new Set([
  'button', 'link', 'textbox', 'searchbox', 'checkbox', 'radio', 'combobox', 'listbox',
  'option', 'menuitem', 'tab', 'switch', 'slider', 'spinbutton',
]);

function compactarAX(nos) {
  const porId = new Map(nos.map((n) => [n.nodeId, n]));
  const linhas = [];
  const refs = new Map();
  let seq = 0;

  const visita = (id, nivel) => {
    const no = porId.get(id);
    if (!no) return;
    const papel = no.role && no.role.value;
    const nome = (no.name && no.name.value) || '';
    const util = !no.ignored && papel && !MUDOS.has(papel);
    let filhoNivel = nivel;
    if (util) {
      let linha = `${'  '.repeat(nivel)}- ${papel}`;
      if (nome) linha += ` "${nome}"`;
      if (ACIONAVEIS.has(papel) && no.backendDOMNodeId != null) {
        const ref = `@e${++seq}`;
        refs.set(ref, no.backendDOMNodeId);
        linha += ` [ref=${ref}]`;
      }
      linhas.push(linha);
      filhoNivel = nivel + 1;
    }
    for (const filho of no.childIds || []) visita(filho, filhoNivel);
  };

  if (nos.length) visita(nos[0].nodeId, 0);
  return { linhas, refs };
}

// Uma linha = um comando. O primeiro campo é o verbo, o resto vai inteiro pro último argumento
// quando o verbo aceita texto — `fill @e2 dois nomes` não pode virar três argumentos.
function parseLote(texto) {
  const COM_TEXTO = new Set(['fill', 'type', 'eval', 'wait']);
  return String(texto)
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith('#'))
    .map((l) => {
      const [verbo, ...resto] = l.split(/\s+/);
      if (!COM_TEXTO.has(verbo) || resto.length < 2) return { verbo, args: resto };
      // fill leva a ref antes do texto; wait leva a flag (--text/--url) antes dele — colada ao
      // texto, o wait nunca a reconhecia e estourava o teto com o texto já na tela.
      const corte = verbo === 'fill' || (verbo === 'wait' && resto[0].startsWith('--')) ? 1 : 0;
      return { verbo, args: [...resto.slice(0, corte), resto.slice(corte).join(' ')] };
    });
}

module.exports = { compactarAX, parseLote };
