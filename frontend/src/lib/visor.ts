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
    // Erro ou midia sem dimensao (audio): um retangulo razoavel e melhor que zero.
    img.onerror = () => resolve({ width: 1600, height: 1200 });
    img.src = url;
  });
}

function paraItem(midia: MidiaVisor, tamanho: { width: number; height: number }) {
  const base = {
    thumb: midia.thumb,
    alt: midia.nome,
    element: midia.element,
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
  // entao o vinculo de volta e pelo nome do arquivo (unico dentro de uma sessao).
  const indiceDe = (item: { nome?: string }) => midias.findIndex((x) => x.nome === item?.nome);

  bp.open({
    items: midias.map((x, i) => paraItem(x, tamanhos[i])),
    position: inicio,
    onOpen(container) {
      // Esc fecha o VISOR, nao a folha atras dele. Precisa ser na CAPTURA: o BottomSheet tambem
      // escuta Escape na window e chama stopImmediatePropagation, e como ele monta antes, o Esc
      // fechava a folha e a foto ficava por cima — o mesmo bug que o AttachmentsSheet ja tinha
      // resolvido assim antes de o visor virar compartilhado.
      window.addEventListener('keydown', escapa, true);
      faixa = document.createElement('div');
      faixa.className = 'visor-faixa';
      faixa.innerHTML =
        '<span class="visor-nome"></span><span class="visor-meta"></span>' +
        '<span class="visor-acoes">' +
        `<a class="visor-btn visor-baixar" title="${m.visor_baixar()}" download>⤓</a>` +
        (acao ? `<button class="visor-btn visor-acao">${acao.rotulo}</button>` : '') +
        '</span>';
      container.appendChild(faixa);
      if (acao) {
        faixa.querySelector('.visor-acao')?.addEventListener('click', () => {
          const escolhida = midias[atual];
          bp.close();
          if (escolhida) acao.acao(escolhida);
        });
      }
      pintar(inicio);
    },
    onUpdate(_container, item) {
      const i = indiceDe(item as { nome?: string });
      if (i >= 0) pintar(i);
    },
    onClosed() {
      window.removeEventListener('keydown', escapa, true);
      faixa = null;
    },
  });
}
