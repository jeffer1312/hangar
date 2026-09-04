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
  console: (c, a) => c.console(a[0] === '--limpar'),
  network: (c) => c.rede(),
  url: (c) => c.avaliar('location.href'),
  shot: async (c, a) => {
    if (!a[0]) return 'erro: shot precisa de um caminho de arquivo';
    const img = await c.capturarPagina();
    fs.writeFileSync(a[0], img.toPNG());
    return `ok: shot ${a[0]}`;
  },
};

const TETO_CORPO = 128 * 1024; // folgado: o maior corpo real é um eval com trecho de JS

async function subirServidor({ controladorDe, escrever }) {
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
      let bruto = '';
      let tamanho = 0;
      for await (const p of req) {
        tamanho += p.length;
        if (tamanho > TETO_CORPO) { responder(413, 'erro: corpo grande demais'); req.destroy(); return; }
        bruto += p;
      }
      let pedido;
      try { pedido = JSON.parse(bruto); } catch { return responder(400, 'erro: corpo invalido'); }
      const ctl = controladorDe(pedido.chave);
      if (!ctl) return responder(404, `erro: a sessao ${pedido.chave} nao tem navegador aberto`);
      const fn = VERBOS[pedido.verbo];
      if (!fn) return responder(400, `erro: verbo desconhecido: ${pedido.verbo}`);
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
