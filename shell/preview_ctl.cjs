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
  };
}

module.exports = { criarControlador };
