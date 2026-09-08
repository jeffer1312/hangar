// Um controlador por WebContentsView. Ele é o dono do estado que ANTES morria a cada comando:
// o CLI abria um socket CDP por verbo e fechava, então `Emulation.setEmulatedMedia` (tema) ia
// junto e não havia onde guardar refs nem console.
const { compactarAX } = require('./preview_fmt.cjs');

const TETO_CONSOLE = 200;   // buffer circular: um app conversador não pode comer memória
const TETO_REDE = 200;
const TEMAS = { claro: 'light', escuro: 'dark', sistema: '' };
// View escondido zera a viewport da página (0x0, independente dos bounds), e aí ela cai no
// layout de celular e não há quadro pra fotografar. A emulação de tamanho a desamarra do
// compositor. Tamanho fixo porque não há painel de onde tirar um: é o que o agente vê.
const VIEWPORT_OCULTO = { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false };
// Layout de celular: iPhone 14/15 em pontos CSS. `deviceScaleFactor: 2` porque site que serve
// imagem por densidade decide por ele, e é o que se quer ver ao testar em celular.
const VIEWPORT_MOVEL = { width: 390, height: 844, deviceScaleFactor: 2, mobile: true };
const UA_MOVEL = 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 '
  + '(KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1';
// Teto do print por CDP. 3000 era APERTADO DEMAIS e recusava quadro que existia: o caminho do
// `Page.captureScreenshot` é o ÚNICO de uma sessão fora do painel (`oculto`), e sem compositor
// ele é lento — medido 2071-2902ms nesta VM, com a primeira captura consumindo 97% do teto.
// Estourar aqui devolvia imagem vazia, que o preview_srv reporta como "não produziu quadro
// (janela minimizada?)" — culpa trocada, porque o quadro estava lá. Alinhado ao `tetoEspera`
// dos outros verbos: melhor um print que demora do que um print que mente.
const TETO_SHOT_CDP = 15000;

// Tecla nomeada precisa de `code` + virtual key code pra o Chromium reconhecer, e Enter precisa
// do caractere: keyDown sem `text: "\r"` não gera char e o form não submete.
const TECLAS = {
  Enter: { code: 'Enter', vk: 13, text: '\r' },
  Tab: { code: 'Tab', vk: 9 },
  Escape: { code: 'Escape', vk: 27 },
  Backspace: { code: 'Backspace', vk: 8 },
  Delete: { code: 'Delete', vk: 46 },
  ArrowUp: { code: 'ArrowUp', vk: 38 },
  ArrowDown: { code: 'ArrowDown', vk: 40 },
  ArrowLeft: { code: 'ArrowLeft', vk: 37 },
  ArrowRight: { code: 'ArrowRight', vk: 39 },
  Home: { code: 'Home', vk: 36 },
  End: { code: 'End', vk: 35 },
  PageUp: { code: 'PageUp', vk: 33 },
  PageDown: { code: 'PageDown', vk: 34 },
  Space: { key: ' ', code: 'Space', vk: 32, text: ' ' },
};
function eventoTecla(tecla) {
  const t = TECLAS[tecla];
  if (t) {
    const ev = { key: t.key || tecla, code: t.code, windowsVirtualKeyCode: t.vk };
    if (t.text) { ev.text = t.text; ev.unmodifiedText = t.text; }
    return ev;
  }
  if (tecla.length === 1) {
    return { key: tecla, text: tecla, unmodifiedText: tecla, windowsVirtualKeyCode: tecla.toUpperCase().charCodeAt(0) };
  }
  return { key: tecla };   // ponytail: tecla nomeada fora da tabela vai só com `key`, como antes
}

// `insertText` só entra em campo editável: sem foco num deles o texto some calado — e depois de
// um clique o foco pode ter caído num botão ou no body. Devolve '' quando dá pra digitar.
const FOCO = `(()=>{const a=document.activeElement;if(!a||a===document.body)return 'nada';const t=a.tagName.toLowerCase();`
  + `const ok=a.isContentEditable||t==='textarea'||(t==='input'&&!/^(button|submit|reset|checkbox|radio|file|image|range|color)$/i.test(a.type||''));`
  + `return ok?'':t+(a.type?'['+a.type+']':'')})()`;

// tetoEspera vale pra TODO comando que pode não voltar (wait e eval): view escondido suspende
// requestAnimationFrame, e um comando pendurado trava o agente sem erro nenhum.
function criarControlador({ dbg, capturarPagina, aoNavegar, tetoEspera = 15000 }) {
  let fila = Promise.resolve();
  let refs = new Map();
  let temaAtual = 'sistema';
  let oculto = false;
  let layoutMovel = false;
  let ultimaRede = Date.now();
  let requisicoesEmVoo = 0;
  const console_ = [];
  const rede = [];

  const guardar = (lista, linha, teto) => {
    lista.push(linha);
    if (lista.length > teto) lista.shift();
  };

  // O depurador do Electron emite UM evento, 'message', com o método CDP como argumento — não
  // um evento por método. `dbg.on('Network.responseReceived')` registra e nunca dispara.
  const eventos = {
    'Runtime.consoleAPICalled': (p) => {
      const texto = (p.args || []).map((a) => (a.value !== undefined ? a.value : a.description || a.type)).join(' ');
      guardar(console_, `${p.type}: ${texto}`, TETO_CONSOLE);
    },
    'Log.entryAdded': (p) => guardar(console_, `${p.entry.level}: ${p.entry.text}`, TETO_CONSOLE),
    'Network.requestWillBeSent': () => { requisicoesEmVoo++; },
    'Network.responseReceived': (p) => {
      requisicoesEmVoo = Math.max(0, requisicoesEmVoo - 1);
      ultimaRede = Date.now();
      guardar(rede, `${p.response.status} ${p.response.url}`, TETO_REDE);
    },
    'Network.loadingFailed': () => { requisicoesEmVoo = Math.max(0, requisicoesEmVoo - 1); },
  };
  dbg.on('message', (_e, metodo, p) => { if (Object.hasOwn(eventos, metodo)) eventos[metodo](p); });

  // Network e Accessibility custam o tempo todo (toda resposta de rede vai pro processo principal;
  // a árvore de acessibilidade é mantida viva), então só entram na primeira vez que um verbo pede.
  // Falha NÃO fica lembrada: a próxima chamada tenta de novo em vez de responder vazio pra sempre.
  const ligados = new Map();
  function ligar(dominio) {
    if (!ligados.has(dominio)) {
      const p = dbg.sendCommand(`${dominio}.enable`).then(() => {
        if (dominio === 'Network') ultimaRede = Date.now();   // o silêncio do --idle conta daqui
      }, (err) => {
        ligados.delete(dominio);
        throw new Error(`${dominio}.enable falhou: ${err && err.message ? err.message : err}`);
      });
      ligados.set(dominio, p);
    }
    return ligados.get(dominio);
  }

  // Emular tamanho numa página que ainda não carregou (o `about:blank` de um view recém-criado)
  // derruba o processo com SIGSEGV. Quem chama espera o load.
  async function aplicarViewport() {
    if (layoutMovel) {
      // Layout de celular pedido por quem está olhando de fora (acesso remoto). Vale MAIS que o
      // `oculto`: quem escolheu ver em celular quer o site servindo mobile, com ou sem painel.
      // Sem a emulação de toque a página não recebe touchstart e um carrossel que só escuta toque
      // fica morto — que é justamente o que se quer testar num layout de celular.
      await dbg.sendCommand('Emulation.setDeviceMetricsOverride', VIEWPORT_MOVEL);
      await dbg.sendCommand('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
      await dbg.sendCommand('Emulation.setUserAgentOverride', { userAgent: UA_MOVEL });
      return;
    }
    await dbg.sendCommand('Emulation.setTouchEmulationEnabled', { enabled: false });
    await dbg.sendCommand('Emulation.setUserAgentOverride', { userAgent: '' });
    if (oculto) await dbg.sendCommand('Emulation.setDeviceMetricsOverride', VIEWPORT_OCULTO);
    else await dbg.sendCommand('Emulation.clearDeviceMetricsOverride');
  }

  async function aplicarTema() {
    const valor = TEMAS[temaAtual] ?? '';
    await dbg.sendCommand('Emulation.setEmulatedMedia',
      valor ? { features: [{ name: 'prefers-color-scheme', value: valor }] } : { features: [] });
  }

  // A emulação se perde ao navegar (crbug 1180104, fechado como "not planned" no Electron), e as
  // refs apontam pra nós que já não existem. Os dois se resolvem no mesmo gancho.
  aoNavegar(async () => {
    refs = new Map();
    // Requisição cancelada pela navegação nem sempre vira loadingFailed; sem zerar, o --idle
    // ficaria preso até o teto na página nova.
    requisicoesEmVoo = 0;
    if (temaAtual !== 'sistema') await aplicarTema();
    // A emulação de tamanho sobrevive à navegação, mas a do tema também deveria e não sobrevive;
    // reaplicar custa um comando e o preço de errar é a página inteira em 0x0, calada.
    // O `oculto` aceita falhar calado (o comentário acima diz por quê). O layout de celular, não:
    // ele foi PEDIDO por alguém que está olhando, e se a emulação não voltar depois de navegar a
    // página vira desktop com a pill ainda marcando celular.
    if (oculto || layoutMovel) {
      await aplicarViewport().catch((err) => {
        if (layoutMovel) console.error(`[nav] layout de celular nao voltou apos navegar: ${err && err.message ? err.message : err}`);
      });
    }
  });

  // Dois frames, não um: o primeiro rAF roda ANTES da pintura do quadro seguinte; só o segundo
  // garante que o que a ação mudou já está pintado — um `shot` logo após `click` saía com o
  // quadro anterior (botão sem a marca de selecionado). O timer de dentro cobre view escondido,
  // onde rAF nunca dispara; o de fora, renderer travado. Falha (contexto destruído por navegação
  // que o próprio clique causou) não é erro da ação.
  const QUADRO = 'new Promise(r=>{requestAnimationFrame(()=>requestAnimationFrame(r));setTimeout(r,100)})';
  const quadro = () => Promise.race([
    dbg.sendCommand('Runtime.evaluate', { expression: QUADRO, awaitPromise: true }).catch(() => {}),
    new Promise((r) => setTimeout(r, 500)),
  ]);

  async function focoNaoEditavel() {
    const r = await dbg.sendCommand('Runtime.evaluate', { expression: FOCO, returnByValue: true });
    const v = r.result && r.result.value;
    return v === '' ? null : (v || 'desconhecido');
  }

  function enfileirar(fn) {
    const resultado = fila.then(fn, fn);
    fila = resultado.then(() => {}, () => {});
    return resultado;
  }

  return {
    enfileirar,
    refDe: (ref) => (refs.has(ref) ? refs.get(ref) : null),
    ligar,
    async snapshot() {
      await ligar('Accessibility');
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
    // Layout do navegador de verdade — não de uma cópia: quem está vendo de fora escolhe, e o
    // agente que dirige esta mesma sessão passa a ver a mesma coisa. Mora junto do tema porque os
    // dois são emulação que `aplicarViewport`/`aoNavegar` precisam repor depois de navegar.
    async layout(modo) {
      if (modo !== 'mobile' && modo !== 'desktop') return `erro: layout desconhecido: ${modo}`;
      layoutMovel = modo === 'mobile';
      await aplicarViewport();
      return `layout: ${modo}`;
    },
    layoutAtual: () => (layoutMovel ? 'mobile' : 'desktop'),
    console(limpar) {
      const saida = console_.join('\n');
      if (limpar) console_.length = 0;
      return saida;
    },
    async rede() { await ligar('Network'); return rede.join('\n'); },
    // Texto visível da página, cru: ler uma mensagem de erro ou uma lista não precisa da árvore
    // de acessibilidade inteira.
    async texto() {
      const r = await dbg.sendCommand('Runtime.evaluate', { expression: 'document.body.innerText', returnByValue: true });
      return String((r.result && r.result.value) ?? '');
    },
    // Estado de exibição do view, dito pelo main quando ele esconde ou mostra. Guardado aqui
    // porque é o mesmo lugar que já sabe reaplicar emulação depois de navegar.
    async definirOculto(valor) {
      oculto = !!valor;
      await aplicarViewport();
    },
    // Dois caminhos, e qual serve depende de o view estar na tela. Visível: `capturePage` do
    // Electron. Escondido: ele REJEITA com UnknownVizError (não devolve imagem vazia), e quem
    // responde é o `Page.captureScreenshot` do CDP — mas só com a emulação de tamanho ligada,
    // senão pendura. O teto é a rede pra esse caso.
    async capturarPagina() {
      await quadro();
      if (!oculto) {
        const img = await capturarPagina().catch(() => null);
        if (img && !img.isEmpty()) return img;
      }
      const pedido = dbg.sendCommand('Page.captureScreenshot', { format: 'png' });
      pedido.catch(() => {});   // rejeição atrasada não pode virar rejeição solta no processo
      const r = await Promise.race([
        pedido.catch(() => null),
        new Promise((res) => setTimeout(() => res(null), TETO_SHOT_CDP)),
      ]);
      if (!r || !r.data) {
        // Distinguir no LOG o que o retorno não distingue: o preview_srv só vê "imagem vazia" e
        // atribui à janela minimizada. Quem for investigar precisa saber se foi o teto.
        console.error(`[nav] captureScreenshot sem dado apos ${TETO_SHOT_CDP}ms (oculto=${oculto})`);
        return { isEmpty: () => true, toPNG: () => Buffer.alloc(0) };
      }
      const png = Buffer.from(r.data, 'base64');
      return { isEmpty: () => png.length === 0, toPNG: () => png };
    },
    fechar() { console_.length = 0; rede.length = 0; refs = new Map(); },
    // Ref VELHA e ref INEXISTENTE dão no mesmo lugar de propósito: o `backendDOMNodeId` morre em
    // qualquer re-render que desmonte o nó, não só em navegação — e é justo o caso de uma lista
    // React que muda depois de um clique. Sem o try, isso voltaria como erro genérico de CDP e o
    // agente não saberia que a saída é tirar snapshot de novo.
    async centroDe(ref) {
      const id = refs.get(ref);
      if (id == null) return null;
      // O clique é por coordenada de tela: elemento abaixo da dobra dava coordenada fora da
      // viewport e o evento não acertava nada, sem erro. Falha aqui não é "ref não existe" —
      // quem decide isso é o getBoxModel logo abaixo.
      try { await dbg.sendCommand('DOM.scrollIntoViewIfNeeded', { backendNodeId: id }); } catch { /* segue */ }
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
      await quadro();
      return `ok: click ${ref}`;
    },
    async preencher(ref, texto) {
      const p = await this.centroDe(ref);
      if (!p) return `erro: ref ${ref} nao existe (rode snapshot de novo)`;
      await this.clicar(ref);
      const foco = await focoNaoEditavel();
      if (foco) return `erro: fill ${ref}: o clique nao deixou um campo de texto com foco (foco em ${foco}) — a ref e mesmo um campo? rode snapshot`;
      // SUBSTITUI: sem seleção o insertText gruda no que já estava. Quem seleciona é o campo
      // `commands` — a tecla sozinha (Ctrl+A) não seleciona nada, porque o Chromium mapeia
      // tecla→comando de edição pelo virtual key code, que o evento sintético não carrega.
      // `commands` não vale em keyUp, não depende de Ctrl vs Cmd, e pega o campo inteiro
      // (triplo clique pegaria só uma linha de um textarea).
      await dbg.sendCommand('Input.dispatchKeyEvent', { type: 'keyDown', commands: ['selectAll'] });
      // insertText substitui a seleção, texto vazio incluído — daí não haver passo de apagar.
      await dbg.sendCommand('Input.insertText', { text: String(texto) });
      await quadro();
      return `ok: fill ${ref}`;
    },
    async digitar(texto) {
      const foco = await focoNaoEditavel();
      if (foco) return `erro: type: nenhum campo de texto com foco (foco em ${foco}) — use fill @eN, ou click no campo antes`;
      await dbg.sendCommand('Input.insertText', { text: String(texto) });
      await quadro();
      return 'ok: type';
    },
    async teclar(tecla) {
      const { text, unmodifiedText, ...soltar } = eventoTecla(String(tecla));
      await dbg.sendCommand('Input.dispatchKeyEvent', { type: 'keyDown', ...soltar, ...(text ? { text, unmodifiedText } : {}) });
      await dbg.sendCommand('Input.dispatchKeyEvent', { type: 'keyUp', ...soltar });
      await quadro();
      return `ok: press ${tecla}`;
    },
    async pairar(ref) {
      const p = await this.centroDe(ref);
      if (!p) return `erro: ref ${ref} nao existe (rode snapshot de novo)`;
      await dbg.sendCommand('Input.dispatchMouseEvent', { type: 'mouseMoved', x: p.x, y: p.y });
      await quadro();
      return `ok: hover ${ref}`;
    },
    async avaliar(js) {
      const rodar = async (params) => {
        const chamada = dbg.sendCommand('Runtime.evaluate', { awaitPromise: true, returnByValue: true, ...params });
        // Rejeição que chega DEPOIS de o teto vencer a corrida não pode virar rejeição solta no
        // processo principal (um `location.reload()` no eval derruba o contexto no meio).
        chamada.catch(() => {});
        const estouro = new Promise((r) => setTimeout(() => r('__teto__'), tetoEspera));
        return Promise.race([chamada, estouro]);
      };
      const erroDe = (r) => r.exceptionDetails && (r.exceptionDetails.exception?.description || r.exceptionDetails.text);
      // Um frame antes de ler: `eval` que clica e lê no mesmo comando devolvia o DOM de ANTES do
      // React re-renderizar, e isso já foi lido como bug de aplicação. O teto de fora cobre
      // script do usuário que nunca resolve.
      let r = await rodar({ expression: `(async()=>{const v=(${js});await ${QUADRO};return v})()` });
      // Declaração (`const a=1; a+1`, `location.reload(); "ok"`) não cabe entre parênteses e vira
      // SyntaxError. Aí roda cru, como no console do DevTools: o valor é o da última expressão e
      // `replMode` deixa `const`/`let` serem redeclarados numa segunda chamada.
      if (r !== '__teto__' && /^SyntaxError/.test(erroDe(r) || '')) {
        r = await rodar({ expression: js, replMode: true });
        if (r !== '__teto__' && !r.exceptionDetails) await quadro();
      }
      if (r === '__teto__') return `erro: eval nao respondeu em ${tetoEspera}ms`;
      if (r.exceptionDetails) return `erro: ${erroDe(r)}`;
      return `ok: ${JSON.stringify(r.result && r.result.value)}`;
    },
    async esperar(args) {
      const alvo = args[0];
      if (/^\d+$/.test(String(alvo))) {
        await new Promise((r) => setTimeout(r, Number(alvo)));
        return `ok: wait ${alvo}ms`;
      }
      // Antes do laço, não dentro do cheque: o cheque tem teto de 100ms e um enable lento
      // viraria "não aconteceu" sem a rede nunca ter sido observada.
      if (alvo === '--idle') await ligar('Network');
      const limite = Date.now() + tetoEspera;
      const cheque = async () => {
        if (String(alvo).startsWith('@')) { await this.snapshot(); return refs.has(alvo); }
        if (alvo === '--url') {
          const r = await dbg.sendCommand('Runtime.evaluate', { expression: 'location.href', returnByValue: true });
          return String(r.result && r.result.value).includes(args[1]);
        }
        if (alvo === '--text') {
          const r = await dbg.sendCommand('Runtime.evaluate', { expression: 'document.body.innerText', returnByValue: true });
          return String(r.result && r.result.value).includes(args[1]);
        }
        // `--idle` é REDE parada (contador zero + 500ms sem resposta) + readyState complete.
        // readyState vira 'complete' quase no ato da navegação e não diz nada sobre fetch que
        // ainda vai popular a tela. Contador de requisição detecta SPA com fetch novo em voo.
        if (alvo === '--idle') {
          if (requisicoesEmVoo > 0) return false;
          if (Date.now() - ultimaRede <= 500) return false;
          const r = await dbg.sendCommand('Runtime.evaluate', { expression: 'document.readyState', returnByValue: true });
          return r.result && r.result.value === 'complete';
        }
        return false;
      };
      while (Date.now() < limite) {
        const tempoRestante = limite - Date.now();
        if (tempoRestante <= 0) break;
        // Protege `cheque()` contra penduração: teto é o menor entre 100ms e tempo restante
        const tetoChecagem = Math.min(100, tempoRestante);
        const checagemComTeto = new Promise((r) => setTimeout(() => r(false), tetoChecagem));
        const resultado = await Promise.race([cheque(), checagemComTeto]);
        if (resultado) return `ok: wait ${args.join(' ')}`;
        // Intervalo entre checagens: usar tempo restante se for menor
        const intervalo = Math.min(120, limite - Date.now());
        if (intervalo > 0) await new Promise((r) => setTimeout(r, intervalo));
      }
      return `erro: wait ${args.join(' ')} nao aconteceu em ${tetoEspera}ms`;
    },
  };
}

module.exports = { criarControlador };
