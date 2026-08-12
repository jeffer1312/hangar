// Monta o gráfico em cima de uma <table> já renderizada.
//
// Imperativo de propósito: a tabela nasce dentro do {@html} do renderMarkdown, então não há
// componente Svelte pra pendurar nela. Mesmo molde do highlightCodeBlocks — roda depois da
// montagem, é idempotente (marca o que já tratou) e não toca no pipeline do markdown.
//
// uPlot é série temporal por natureza: o eixo x é numérico. Aqui o x é o ÍNDICE da linha (0,1,2…)
// e os rótulos entram pelo formatador do eixo — é o que permite barra por categoria sem trazer uma
// segunda biblioteca só pra isso.
// O uPlot entra por import DINÂMICO: estático, os ~66 KB dele iam no pacote inicial de todo mundo,
// inclusive de quem nunca abre um gráfico (e o recurso nasce desligado). Assim o custo só é pago no
// primeiro clique em "Gráfico".
import type uPlotTipo from 'uplot';
import { lerTabela, formatarValor, type TabelaLida } from './tableChart';

const MARCA = 'cpChart';   // dataset.cpChart = já tratada

function cor(el: HTMLElement, nome: string, queda: string): string {
  const v = getComputedStyle(el).getPropertyValue(nome).trim();
  return v || queda;
}

async function desenhar(alvo: HTMLElement, t: TabelaLida, col: number): Promise<() => void> {
  const [{ default: uPlot }] = await Promise.all([
    import('uplot'),
    import('uplot/dist/uPlot.min.css'),
  ]);
  const c = t.colunas[col];
  const xs = t.rotulos.map((_, i) => i);
  // --chart-1: slot 1 da paleta categórica do tema (app.css). Existe justamente pra isto e já tem
  // o par do tema claro — chumbar o accent aqui ficaria de fora da troca de tema.
  const accent = cor(alvo, '--chart-1', '#7c87e8');
  const texto = cor(alvo, '--text-muted', '#8b8b8b');
  const linha = cor(alvo, '--border-subtle', 'rgba(255,255,255,0.07)');

  const largura = Math.max(240, alvo.clientWidth || 320);
  const u = new uPlot(
    {
      width: largura,
      height: 180,
      // Sem legenda e sem cursor: dentro de uma bolha de chat o valor cabe no eixo; a legenda do
      // uPlot rouba duas linhas de altura e repete o que o título da coluna já diz.
      legend: { show: false },
      cursor: { show: false },
      padding: [12, 8, 4, 8],
      scales: {
        x: {
          time: false,
          // Sem isto o uPlot encosta o 1o e o último ponto nas BORDAS: com duas linhas, uma barra
          // colava na esquerda e a outra na direita, com um vão enorme no meio. Meia categoria de
          // folga de cada lado é o que centra as barras.
          range: () => [-0.6, Math.max(1, xs.length - 1) + 0.6],
        },
        y: {
          // Barra SEMPRE parte do zero. O auto-range do uPlot começava em ~300 e fazia 327 parecer
          // um décimo de 769, quando a razão real é 1 pra 2,4 — a mentira clássica do eixo cortado.
          range: (_u, _min, max) => [0, max * 1.15 || 1],
        },
      },
      axes: [
        {
          stroke: texto, grid: { show: false }, ticks: { show: false },
          // O rótulo da linha no lugar do índice. splits inteiros só — meio índice não é categoria.
          splits: () => xs,
          values: (_u, vals) => vals.map((v) => t.rotulos[v] ?? ''),
          font: '11px system-ui, sans-serif',
        },
        {
          stroke: texto, grid: { stroke: linha, width: 1 }, ticks: { show: false },
          values: (_u, vals) => vals.map((v) => formatarValor(v)),
          font: '11px system-ui, sans-serif',
          size: 52,
        },
      ],
      series: [
        {},
        {
          label: c.titulo,
          stroke: accent,
          fill: accent + (accent.startsWith('#') ? '55' : ''),
          // Barra larga: com 2 ou 3 categorias o padrão do uPlot fica um palito. O teto de 72px
          // impede que UMA categoria sozinha vire um bloco atravessando a bolha.
          paths: uPlot.paths.bars!({ size: [0.7, 72] }),
          points: { show: false },
        },
      ],
    },
    [xs, c.valores],
    alvo,
  );

  // A bolha muda de largura (painel abre, janela redimensiona) e o uPlot não é fluido sozinho.
  const ro = new ResizeObserver(() => {
    // A bolha inteira é recriada quando a mensagem re-renderiza (replay do SSE reatribui o evento e
    // o {@html} refaz os filhos), e aí este palco vira nó solto — o uPlot e este observer ficariam
    // pendurados sem ninguém pra desmontá-los. Auto-desmonte quando o alvo sai do documento.
    if (!alvo.isConnected) { ro.disconnect(); u.destroy(); return; }
    const l = alvo.clientWidth;
    if (l > 0) u.setSize({ width: l, height: 180 });
  });
  ro.observe(alvo);
  return () => { ro.disconnect(); u.destroy(); };
}

/** Idempotente: pode rodar a cada nova versão do html. */
export function enhanceTables(raiz: HTMLElement): void {
  for (const tabela of raiz.querySelectorAll('table')) {
    const el = tabela as HTMLTableElement;
    if (el.dataset[MARCA]) continue;
    el.dataset[MARCA] = '1';

    const lida = lerTabela(el);
    if (!lida) continue;   // sem coluna numérica: a tabela fica como está, sem botão nenhum

    const barra = document.createElement('div');
    barra.className = 'cp-chart-bar';

    const botao = document.createElement('button');
    botao.type = 'button';
    botao.className = 'cp-chart-btn';
    botao.textContent = 'Gráfico';

    const seletor = document.createElement('select');
    seletor.className = 'cp-chart-sel';
    seletor.hidden = true;
    for (const [i, c] of lida.colunas.entries()) {
      const o = document.createElement('option');
      o.value = String(i);
      o.textContent = c.titulo;
      seletor.appendChild(o);
    }
    // Uma coluna numérica só: o seletor não tem escolha a oferecer.
    const temEscolha = lida.colunas.length > 1;

    const palco = document.createElement('div');
    palco.className = 'cp-chart-palco';
    palco.hidden = true;

    barra.append(botao);
    if (temEscolha) barra.append(seletor);
    el.parentNode?.insertBefore(barra, el);
    el.parentNode?.insertBefore(palco, el.nextSibling);

    let limpar: (() => void) | null = null;
    let aberto = false;
    // Contador de geração: o import do uPlot é assíncrono, então entre o clique e o desenho o
    // usuário pode fechar ou trocar de coluna. Sem isto, o gráfico de uma escolha antiga chegaria
    // atrasado e se somaria ao novo dentro do mesmo palco.
    let geracao = 0;

    const render = () => {
      const minha = ++geracao;
      limpar?.();
      limpar = null;
      palco.textContent = '';
      if (!aberto) return;
      const voltarPraTabela = () => {
        aberto = false;
        palco.hidden = true;
        el.hidden = false;
        seletor.hidden = true;
        botao.textContent = 'Gráfico';
        botao.classList.remove('ativo');
      };
      void desenhar(palco, lida, Number(seletor.value || 0))
        .then((fim) => {
          if (minha !== geracao) { fim(); return; }   // chegou tarde: desfaz em vez de pintar
          limpar = fim;
        })
        .catch((err) => {
          // O import do uPlot pode falhar (offline, chunk 404). Sem isto o clique já tinha
          // ESCONDIDO a tabela e mostrado o palco vazio: o usuário ficava sem tabela e sem
          // gráfico, olhando um retângulo, com o botão dizendo "Tabela" como se tivesse dado certo.
          console.error('gráfico da tabela: falhou ao desenhar', err);
          if (minha === geracao) voltarPraTabela();
        });
    };

    botao.addEventListener('click', () => {
      aberto = !aberto;
      palco.hidden = !aberto;
      el.hidden = aberto;              // tabela e gráfico são a MESMA informação: um de cada vez
      seletor.hidden = !aberto || !temEscolha;
      botao.textContent = aberto ? 'Tabela' : 'Gráfico';
      botao.classList.toggle('ativo', aberto);
      render();
    });
    seletor.addEventListener('change', render);
  }
}
