import BiggerPicture from 'bigger-picture/vanilla';
import 'bigger-picture/css';
import * as m from '../paraglide/messages';

// Visor de midia do app inteiro: chat, anexo de arquivo e folha de Anexos abrem POR AQUI.
//
// Por que uma biblioteca, e nao o terceiro visor caseiro: zoom (roda/clique/pinca), passar entre as
// midias, arrastar pra fechar, teclado e foco preso sao trabalho que a bigger-picture ja faz em
// 8,4 KB gzip, MIT, sem dependencia. Ela e escrita em Svelte e distribuida COMPILADA — por isso o
// import e de `bigger-picture/vanilla` e nao do pacote raiz: a condicao `svelte` do package.json
// aponta pro fonte em Svelte 3/4, que o vite-plugin-svelte tentaria compilar com o compilador da 5.
//
// O que ela NAO tem e fica nosso: nome do arquivo, tamanho/prazo e os botoes de baixar e de acao
// (ex.: "usar no ditado"). Isso vira uma faixa montada no `onOpen` e atualizada no `onUpdate` — a
// lib nao expoe slot, mas expoe o container.

export type MidiaVisor = {
  url: string;
  nome: string;
  tipo: 'image' | 'video' | 'audio';
  /** Linha discreta ao lado do nome (tamanho, idade, prazo). Opcional. */
  meta?: string;
  /** Miniatura ja carregada, pra abrir sem esperar o original. */
  thumb?: string;
  /** No de origem: a animacao de abrir/fechar sai dele. */
  element?: HTMLElement;
};

export type AcaoVisor = {
  rotulo: string;
  /** Recebe a midia que estava aberta. Fecha o visor antes de rodar. */
  acao: (midia: MidiaVisor) => void;
};

let instancia: ReturnType<typeof BiggerPicture> | null = null;
// Listener de Escape da abertura VIVA. É de módulo, e não da chamada, porque `abrirVisor` é
// assíncrona (mede as mídias antes de abrir): dois toques rápidos em miniaturas diferentes chegam
// a `bp.open` duas vezes, e só o `onClosed` da segunda rodaria — o `escapa` da primeira ficava
// pendurado na window pra sempre, fechando visor alheio.
let escapaVivo: ((e: KeyboardEvent) => void) | null = null;

function trocarEscapa(novo: ((e: KeyboardEvent) => void) | null) {
  if (escapaVivo) window.removeEventListener('keydown', escapaVivo, true);
  escapaVivo = novo;
  if (novo) window.addEventListener('keydown', novo, true);
}

function obterInstancia() {
  if (!instancia) {
    // O alvo tem que ser o proprio <body>: a lib mede o container por `target.offsetWidth` e, so
    // pro body, usa `window.innerHeight` como altura. Num <div> criado a mao a altura e ZERO, e o
    // fator de escala vira 0 — o visor abre com a midia em `width: 0px; height: 0px`, sem erro
    // nenhum na tela (medido em 26/08/2026, foi assim que este arquivo nasceu errado).
    instancia = BiggerPicture({ target: document.body });
  }
  return instancia;
}

/**
 * Tamanho natural da midia, lido da MINIATURA que ja esta na tela. A lib precisa dele: sem width e
 * height o item nasce com `width: 0px; height: 0px` e o visor abre VAZIO — sem erro nenhum, o que
 * custou meia hora de "por que a imagem nao aparece".
 */
function medir(el?: HTMLElement) {
  const img = el?.querySelector('img') as HTMLImageElement | null;
  if (img?.naturalWidth) return { width: img.naturalWidth, height: img.naturalHeight };
  const video = el?.querySelector('video') as HTMLVideoElement | null;
  if (video?.videoWidth) return { width: video.videoWidth, height: video.videoHeight };
  return null;
}

/** Mede carregando de fato — plano B de quando a miniatura ainda nao tem tamanho. Cai no cache do
 *  browser (mesma url da miniatura), entao na pratica resolve na hora. */
function medirCarregando(url: string): Promise<{ width: number; height: number }> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
    // Erro ou midia sem dimensao (video/audio, que nao carregam como <img>): um retangulo razoavel
    // e melhor que zero — mas quem NAO carregar de verdade aparece dito na faixa, pelo onError da
    // lib; sem isso um anexo expirado abria como retangulo cinza e ninguem sabia por que.
    img.onerror = () => resolve({ width: 1600, height: 1200 });
    img.src = url;
  });
}

function paraItem(midia: MidiaVisor, tamanho: { width: number; height: number }, idx: number) {
  const base = {
    thumb: midia.thumb,
    alt: midia.nome,
    element: midia.element,
    // `idx` (e nao o nome) e o que liga o item de volta pra nossa lista no onUpdate: duas fotos com
    // o MESMO nome de arquivo na mesma mensagem existem, e por nome a faixa ficava presa na
    // primeira ao navegar.
    idx,
    nome: midia.nome,
    meta: midia.meta,
    ...tamanho,
  };
  if (midia.tipo === 'image') return { ...base, img: midia.url };
  // Video e audio usam o player nativo da lib; `type` fica de fora de proposito — o servidor manda
  // o Content-Type certo e adivinhar aqui pelo nome erraria justamente no .webm de ditado.
  return { ...base, sources: [{ src: midia.url }] };
}

/**
 * Abre o visor na midia `inicio` da lista. A lista inteira vai junto: e ela que da o "12 / 34" e as
 * setas — passar so a url clicada era o que os tres visores de hoje faziam, e por isso nenhum deles
 * navegava.
 */
export async function abrirVisor(midias: MidiaVisor[], inicio: number, acao?: AcaoVisor) {
  try {
    await montarVisor(midias, inicio, acao);
  } catch (e) {
    // Os três chamadores disparam com `void` (é um onclick). Sem isto, uma falha aqui vira rejeição
    // não tratada e o toque na miniatura simplesmente não faz nada, sem rastro nenhum.
    console.error('visor: falhou ao abrir', e);
  }
}

async function montarVisor(midias: MidiaVisor[], inicio: number, acao?: AcaoVisor) {
  if (!midias.length) return;
  const tamanhos = await Promise.all(
    midias.map(async (x) => medir(x.element) ?? (await medirCarregando(x.url))),
  );
  const bp = obterInstancia();
  let faixa: HTMLElement | null = null;
  let atual = inicio;

  const escapa = (e: KeyboardEvent) => {
    if (e.key !== 'Escape') return;
    e.stopImmediatePropagation();
    bp.close();
  };

  const pintar = (indice: number) => {
    atual = indice;
    const midia = midias[indice];
    if (!faixa || !midia) return;
    const nome = faixa.querySelector('.visor-nome');
    const meta = faixa.querySelector('.visor-meta');
    const baixar = faixa.querySelector<HTMLAnchorElement>('.visor-baixar');
    if (nome) nome.textContent = midia.nome;
    if (meta) meta.textContent = midia.meta ?? '';
    if (baixar) { baixar.href = midia.url; baixar.download = midia.nome; }
  };

  // A lib nao diz o indice no onUpdate, so o item — e o item e o objeto que devolvemos em paraItem,
  // que carrega o `idx` de propria lavra pra este caminho de volta.
  const indiceDe = (item: { idx?: number }) => (typeof item?.idx === 'number' ? item.idx : -1);

  bp.open({
    items: midias.map((x, i) => paraItem(x, tamanhos[i], i)),
    position: inicio,
    onOpen(container) {
      // Esc fecha o VISOR, nao a folha atras dele. Precisa ser na CAPTURA: o BottomSheet tambem
      // escuta Escape na window e chama stopImmediatePropagation, e como ele monta antes, o Esc
      // fechava a folha e a foto ficava por cima — o mesmo bug que o AttachmentsSheet ja tinha
      // resolvido assim antes de o visor virar compartilhado.
      trocarEscapa(escapa);
      // Nós criados um a um, sem innerHTML: o rótulo da ação vem de fora (i18n hoje, quem sabe o
      // nome de um arquivo amanhã) e montar HTML por string é o caminho curto pra injetar marcação
      // sem querer.
      faixa = document.createElement('div');
      faixa.className = 'visor-faixa';
      const nome = document.createElement('span');
      nome.className = 'visor-nome';
      const meta = document.createElement('span');
      meta.className = 'visor-meta';
      const acoes = document.createElement('span');
      acoes.className = 'visor-acoes';
      const baixar = document.createElement('a');
      baixar.className = 'visor-btn visor-baixar';
      baixar.title = m.visor_baixar();
      baixar.textContent = '⤓';
      acoes.append(baixar);
      if (acao) {
        const botao = document.createElement('button');
        botao.className = 'visor-btn visor-acao';
        botao.textContent = acao.rotulo;
        botao.addEventListener('click', () => {
          const escolhida = midias[atual];
          bp.close();
          if (escolhida) acao.acao(escolhida);
        });
        acoes.append(botao);
      }
      faixa.append(nome, meta, acoes);
      container.appendChild(faixa);
      pintar(inicio);
    },
    onUpdate(_container, item) {
      const i = indiceDe(item as { idx?: number });
      if (i >= 0) pintar(i);
    },
    onError() {
      // A midia nao carregou (anexo expirado, servidor fora). Sem isto o visor abre um retangulo
      // vazio e o erro nao existe em lugar nenhum.
      const meta = faixa?.querySelector('.visor-meta');
      if (meta) meta.textContent = m.visor_nao_carregou();
    },
    onClosed() {
      // Só solta se ainda for o NOSSO: uma abertura mais nova já trocou o listener, e removê-lo
      // aqui deixaria o visor vivo sem Escape.
      if (escapaVivo === escapa) trocarEscapa(null);
      faixa = null;
    },
  });
}
