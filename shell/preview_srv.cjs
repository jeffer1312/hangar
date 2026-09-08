// Servidor do CLI, dentro do processo principal do Electron — é lá que vivem os controladores.
// Porta EFÊMERA e token sorteado a cada subida, publicados num sidecar que só o dono lê: assim o
// CLI descobre os dois sem o shell depender do .env do backend.
const http = require('node:http');
const crypto = require('node:crypto');
const fs = require('node:fs');

const VERBOS = {
  snapshot: (c) => c.snapshot(),
  click: (c, a) => c.clicar(a[0]),
  fill: (c, a) => c.preencher(a[0], a[1]),
  type: (c, a) => c.digitar(a[0]),
  press: (c, a) => c.teclar(a[0]),
  hover: (c, a) => c.pairar(a[0]),
  wait: (c, a) => c.esperar(a),
  eval: (c, a) => c.avaliar(a[0]),
  tema: (c, a) => c.tema(a[0]),
  layout: (c, a) => (a[0] ? c.layout(a[0]) : `layout: ${c.layoutAtual()}`),
  console: (c, a) => c.console(a[0] === '--limpar'),
  network: (c) => c.rede(),
  text: (c) => c.texto(),
  url: (c) => c.avaliar('location.href'),
  shot: async (c, a) => {
    if (!a[0]) return 'erro: shot precisa de um caminho de arquivo';
    const img = await c.capturarPagina();
    // O controlador já acordou o renderer e insistiu até o teto; imagem ainda vazia é view que
    // não compõe (janela minimizada, por exemplo). PNG de 0 bytes com "ok" mentiria.
    if (img.isEmpty()) return 'erro: o navegador desta sessao nao produziu quadro (janela minimizada?) — text/snapshot/click funcionam; o print sai com a janela do Hangar visivel';
    // Assincrona: writeFileSync roda na thread principal do Electron e travaria a interface
    // inteira enquanto um disco lento escreve o PNG.
    await fs.promises.writeFile(a[0], img.toPNG());
    return `ok: shot ${a[0]}`;
  },
};

const TETO_CORPO = 128 * 1024; // folgado: o maior corpo real é um eval com trecho de JS

// `fecharDe(chave)` fecha o navegador de verdade (view, controlador, sidecar) e avisa o painel;
// devolve false quando a chave não tem navegador. Fica fora de VERBOS porque não passa pela
// fila do controlador — o controlador é justamente o que morre.
async function subirServidor({ controladorDe, escrever, fecharDe }) {
  const token = crypto.randomBytes(24).toString('hex');
  const tokenBuf = Buffer.from(`Bearer ${token}`);
  const servidor = http.createServer(async (req, res) => {
    const responder = (codigo, texto) => {
      res.writeHead(codigo, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end(texto);
    };
    if (req.method !== 'POST' || req.url !== '/cmd') return responder(404, 'erro: rota desconhecida');
    // timingSafeEqual exige buffers do MESMO tamanho — checa o comprimento antes, senão ela lança.
    const auth = Buffer.from(req.headers.authorization || '');
    if (auth.length !== tokenBuf.length || !crypto.timingSafeEqual(auth, tokenBuf)) {
      return responder(401, 'erro: token invalido');
    }
    try {
      // Sem teto, um `for await` sem limite deixa quem tem o token derrubar por memória o
      // processo PRINCIPAL do Electron — não só a requisição, o app inteiro. A leitura mora
      // dentro do try pra uma desconexão do cliente no meio virar 500, não rejeição solta.
      // Concatenar `p` (Buffer) direto em string decodifica pedaço por pedaço: um multibyte
      // (acento, o caso comum aqui) cortado na fronteira entre dois pedaços chega partido duas
      // vezes. Guarda os Buffers e decodifica UMA vez no fim.
      const pedacos = [];
      let tamanho = 0;
      for await (const p of req) {
        tamanho += p.length;
        if (tamanho > TETO_CORPO) { responder(413, 'erro: corpo grande demais'); req.destroy(); return; }
        pedacos.push(p);
      }
      const bruto = Buffer.concat(pedacos).toString('utf8');
      let pedido;
      try { pedido = JSON.parse(bruto); } catch { return responder(400, 'erro: corpo invalido'); }
      if (pedido.verbo === 'close') {
        if (!fecharDe) return responder(500, 'erro: este shell nao sabe fechar navegador pelo CLI');
        return responder(200, fecharDe(pedido.chave) ? 'ok: close' : `erro: a sessao ${pedido.chave} nao tem navegador aberto`);
      }
      const ctl = controladorDe(pedido.chave);
      if (!ctl) return responder(404, `erro: a sessao ${pedido.chave} nao tem navegador aberto`);
      // Object.hasOwn, nao `VERBOS[pedido.verbo]` direto: um verbo tipo "constructor" alcancaria o
      // prototype (Object.prototype.constructor) e devolveria 200 com [object Object] em vez do
      // 400 de verbo desconhecido.
      if (!Object.hasOwn(VERBOS, pedido.verbo)) return responder(400, `erro: verbo desconhecido: ${pedido.verbo}`);
      const fn = VERBOS[pedido.verbo];
      responder(200, String(await ctl.enfileirar(() => fn(ctl, pedido.args || []))));
    } catch (err) {
      responder(500, `erro: ${err && err.message ? err.message : err}`);
    }
  });
  await new Promise((r) => servidor.listen(0, '127.0.0.1', r));
  const porta = servidor.address().port;
  escrever({ porta, token, pid: process.pid, ts: Date.now() });
  return { porta, token, endereco: '127.0.0.1', fechar: () => servidor.close() };
}

module.exports = { subirServidor };
