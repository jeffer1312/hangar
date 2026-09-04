// Um controlador por WebContentsView. Ele é o dono do estado que ANTES morria a cada comando:
// o CLI abria um socket CDP por verbo e fechava, então `Emulation.setEmulatedMedia` (tema) ia
// junto e não havia onde guardar refs nem console.
const { compactarAX } = require('./preview_fmt.cjs');

const TETO_CONSOLE = 200;   // buffer circular: um app conversador não pode comer memória
const TETO_REDE = 200;
const TEMAS = { claro: 'light', escuro: 'dark', sistema: '' };

// tetoEspera vale pra TODO comando que pode não voltar (wait e eval): view escondido suspende
// requestAnimationFrame, e um comando pendurado trava o agente sem erro nenhum.
function criarControlador({ dbg, capturarPagina, aoNavegar, tetoEspera = 15000 }) {
  let fila = Promise.resolve();
  let refs = new Map();
  let temaAtual = 'sistema';
  let ultimaRede = 0;
  const console_ = [];
  const rede = [];

  const guardar = (lista, linha, teto) => {
    lista.push(linha);
    if (lista.length > teto) lista.shift();
  };

  dbg.on('Runtime.consoleAPICalled', (_e, p) => {
    const texto = (p.args || []).map((a) => (a.value !== undefined ? a.value : a.description || a.type)).join(' ');
    guardar(console_, `${p.type}: ${texto}`, TETO_CONSOLE);
  });
  dbg.on('Log.entryAdded', (_e, p) => guardar(console_, `${p.entry.level}: ${p.entry.text}`, TETO_CONSOLE));
  dbg.on('Network.responseReceived', (_e, p) => {
    ultimaRede = Date.now();   // alimenta o `wait --idle` da Task 4
    guardar(rede, `${p.response.status} ${p.response.url}`, TETO_REDE);
  });

  async function aplicarTema() {
    const valor = TEMAS[temaAtual] ?? '';
    await dbg.sendCommand('Emulation.setEmulatedMedia',
      valor ? { features: [{ name: 'prefers-color-scheme', value: valor }] } : { features: [] });
  }

  // A emulação se perde ao navegar (crbug 1180104, fechado como "not planned" no Electron), e as
  // refs apontam pra nós que já não existem. Os dois se resolvem no mesmo gancho.
  aoNavegar(async () => {
    refs = new Map();
    if (temaAtual !== 'sistema') await aplicarTema();
  });

  function enfileirar(fn) {
    const resultado = fila.then(fn, fn);
    fila = resultado.then(() => {}, () => {});
    return resultado;
  }

  return {
    enfileirar,
    refDe: (ref) => (refs.has(ref) ? refs.get(ref) : null),
    async snapshot() {
      const { nodes } = await dbg.sendCommand('Accessibility.getFullAXTree');
      const compacto = compactarAX(nodes || []);
      refs = compacto.refs;
      return compacto.linhas.join('\n');
    },
    async tema(modo) {
      if (!(modo in TEMAS)) return `erro: tema desconhecido: ${modo}`;
      temaAtual = modo;
      await aplicarTema();
      return `tema: ${modo}`;
    },
    console(limpar) {
      const saida = console_.join('\n');
      if (limpar) console_.length = 0;
      return saida;
    },
    rede: () => rede.join('\n'),
    capturarPagina,
    fechar() { console_.length = 0; rede.length = 0; refs = new Map(); },
    // Ref VELHA e ref INEXISTENTE dão no mesmo lugar de propósito: o `backendDOMNodeId` morre em
    // qualquer re-render que desmonte o nó, não só em navegação — e é justo o caso de uma lista
    // React que muda depois de um clique. Sem o try, isso voltaria como erro genérico de CDP e o
    // agente não saberia que a saída é tirar snapshot de novo.
    async centroDe(ref) {
      const id = refs.get(ref);
      if (id == null) return null;
      try {
        const { model } = await dbg.sendCommand('DOM.getBoxModel', { backendNodeId: id });
        const q = model && model.content;
        if (!q) return null;
        return { x: Math.round((q[0] + q[4]) / 2), y: Math.round((q[1] + q[5]) / 2) };
      } catch {
        return null;
      }
    },
    async clicar(ref) {
      const p = await this.centroDe(ref);
      if (!p) return `erro: ref ${ref} nao existe (rode snapshot de novo)`;
      // Evento REAL, não `.click()` em JS: lista que só ouve mousedown ignora o click sintético.
      const base = { x: p.x, y: p.y, button: 'left', clickCount: 1 };
      await dbg.sendCommand('Input.dispatchMouseEvent', { type: 'mousePressed', ...base });
      await dbg.sendCommand('Input.dispatchMouseEvent', { type: 'mouseReleased', ...base });
      return `ok: click ${ref}`;
    },
    async preencher(ref, texto) {
      const p = await this.centroDe(ref);
      if (!p) return `erro: ref ${ref} nao existe (rode snapshot de novo)`;
      await this.clicar(ref);
      // Seleciona tudo e substitui: `value=` em JS não dispara o onChange do React.
      await dbg.sendCommand('Input.dispatchKeyEvent', { type: 'keyDown', key: 'a', code: 'KeyA', modifiers: 2 });
      await dbg.sendCommand('Input.dispatchKeyEvent', { type: 'keyUp', key: 'a', code: 'KeyA', modifiers: 2 });
      await dbg.sendCommand('Input.insertText', { text: String(texto) });
      return `ok: fill ${ref}`;
    },
    async digitar(texto) {
      await dbg.sendCommand('Input.insertText', { text: String(texto) });
      return 'ok: type';
    },
    async teclar(tecla) {
      await dbg.sendCommand('Input.dispatchKeyEvent', { type: 'keyDown', key: tecla });
      await dbg.sendCommand('Input.dispatchKeyEvent', { type: 'keyUp', key: tecla });
      return `ok: press ${tecla}`;
    },
    async pairar(ref) {
      const p = await this.centroDe(ref);
      if (!p) return `erro: ref ${ref} nao existe (rode snapshot de novo)`;
      await dbg.sendCommand('Input.dispatchMouseEvent', { type: 'mouseMoved', x: p.x, y: p.y });
      return `ok: hover ${ref}`;
    },
    async avaliar(js) {
      // Um frame antes de ler: `eval` que clica e lê no mesmo comando devolvia o DOM de ANTES do
      // React re-renderizar, e isso já foi lido como bug de aplicação. O frame corre contra um
      // timer DENTRO da página porque `requestAnimationFrame` não dispara em view escondido —
      // esperar só por ele deixava o comando pendurado pra sempre quando o painel estava noutra
      // aba. E o teto de fora cobre o resto: script do usuário que nunca resolve.
      const espera = 'Promise.race([new Promise(requestAnimationFrame), new Promise(r=>setTimeout(r,50))])';
      const chamada = dbg.sendCommand('Runtime.evaluate', {
        expression: `(async()=>{const v=(${js});await ${espera};return v})()`,
        awaitPromise: true, returnByValue: true,
      });
      const estouro = new Promise((r) => setTimeout(() => r('__teto__'), tetoEspera));
      const r = await Promise.race([chamada, estouro]);
      if (r === '__teto__') return `erro: eval nao respondeu em ${tetoEspera}ms`;
      if (r.exceptionDetails) return `erro: ${r.exceptionDetails.text}`;
      return `ok: ${JSON.stringify(r.result && r.result.value)}`;
    },
  };
}

module.exports = { criarControlador };
