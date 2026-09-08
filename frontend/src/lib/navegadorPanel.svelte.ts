// Largura do navegador embutido: arrastável pela divisória esquerda e guardada (cp_nav_w).
// Mesma forma do ctxPanel — o Chat reserva a faixa no conteúdo (--cp-ctx-w usa a largura dele
// quando a aba Navegador está ativa) e o NavegadorPane se posiciona nela, então os dois leem
// daqui e nunca divergem.

import { navegadorNativo } from './navegadorNativo';

const CHAVE = 'cp_nav_w';

// Abaixo do MIN um browser não serve pra nada; o teto deixa uma coluna de chat legível mais o
// trilho da sidebar (que o navegador força ao abrir, via sidebarPin.setForced no Chat).
export const NAV_MIN = 400;
export const RESERVA_CHAT = 520;
export const RESERVA_TRILHO = 52;

function tetoLargura(): number {
  if (typeof window === 'undefined') return 920;
  return Math.max(NAV_MIN, window.innerWidth - RESERVA_CHAT - RESERVA_TRILHO);
}

function clampLargura(w: number): number {
  return Math.max(NAV_MIN, Math.min(tetoLargura(), w));
}

// Nasce grande: browser quer espaço (a coluna de contexto, em comparação, nasce em 264-340).
function larguraDefault(): number {
  if (typeof window === 'undefined') return 640;
  return clampLargura(Math.round(window.innerWidth * 0.42));
}

function carregarLargura(): number {
  try {
    const salva = Number(localStorage.getItem(CHAVE));
    if (Number.isFinite(salva) && salva > 0) return clampLargura(salva);
  } catch {
    /* sem storage: default */
  }
  return clampLargura(larguraDefault());
}

// `resizing` aqui e não num $state do NavegadorPane: o flag precisa sobreviver à desmontagem no
// meio do arrasto (trocar de sessão remonta o Chat) — mesmo motivo do ctxPanel.
// `abertos`: sessões com navegador aberto (chave workspaceSessionKey → url atual, '' = ainda não
// navegou). Vive aqui porque o Chat remonta por key a cada troca de sessão — um $state local
// esqueceria que a sessão X tem navegador aberto. Persistido pra um reload da página reexibir
// (o view continua vivo no main; se o SHELL reiniciou, o front reabre com esta url).
//
// ORDEM IMPORTA: as consts e funções de carga ficam ANTES do $state — o inicializador roda na
// linha dele, e uma const declarada depois está no TDZ (o try/catch do carregarAbertos engolia o
// ReferenceError e o store nascia vazio mesmo com localStorage cheio — o painel não remontava
// depois de reload).
const CHAVE_ABERTOS = 'cp_nav_abertos';
// Dona de cada marca: o transcript (`jsonl`) da sessão que tinha o navegador quando a lista foi
// lida. A marca é por NOME, e nome se repete: sessão morta e recriada com o app FECHADO chega na
// lista com o mesmo nome e outro transcript — sem a dona, a poda por nome a tomava pela mesma.
const CHAVE_DONOS = 'cp_nav_donos';

function carregarJson(chave: string): Record<string, string> {
  try {
    const j = JSON.parse(localStorage.getItem(chave) || '{}');
    return j && typeof j === 'object' && !Array.isArray(j) ? j : {};
  } catch {
    return {};
  }
}

const donos: Record<string, string> = carregarJson(CHAVE_DONOS);

export const navegadorPanel = $state({
  largura: carregarLargura(),
  resizing: false,
  abertos: carregarJson(CHAVE_ABERTOS),
});

function salvarAbertos(): void {
  try {
    localStorage.setItem(CHAVE_ABERTOS, JSON.stringify(navegadorPanel.abertos));
    localStorage.setItem(CHAVE_DONOS, JSON.stringify(donos));
  } catch {
    /* sem storage: vale só nesta sessão */
  }
}

export function marcarNavAberto(chave: string): void {
  if (!(chave in navegadorPanel.abertos)) {
    navegadorPanel.abertos[chave] = '';
    salvarAbertos();
  }
}

export function atualizarNavUrl(chave: string, url: string): void {
  if (chave in navegadorPanel.abertos) {
    navegadorPanel.abertos[chave] = url;
    salvarAbertos();
  }
}

export function fecharNav(chave: string): void {
  delete navegadorPanel.abertos[chave];
  delete donos[chave];
  salvarAbertos();
}

// Sessão que sumiu da lista — ou que está lá com outro transcript — leva o navegador junto:
// a marca sobrevivia à morte da sessão e uma sessão nova com o mesmo nome no mesmo repo nascia
// com o view e a URL de uma morta, e o agente dela era avisado de um navegador que nunca abriu.
// `vivos` só traz os servidores cuja lista foi lida agora (nome → jsonl; null = sessão ainda sem
// transcript, que nem adota nem condena); servidor offline não decide.
//
// Transcript diferente da dona só condena na PRIMEIRA vez que a sessão aparece desde que a página
// abriu: aí ela morreu e foi recriada com o app fechado. Presente desde antes e o transcript
// trocou, é `/clear` (mesma sessão, arquivo novo) — a dona só é atualizada.
const presentes = new Set<string>();

export function podarNavMortos(vivos: Map<string, Map<string, string | null>>): void {
  let mudou = false;
  for (const chave of Object.keys(navegadorPanel.abertos)) {
    const i = chave.indexOf('::');
    const sessoes = vivos.get(chave.slice(0, i));
    if (!sessoes) continue;
    const nome = chave.slice(i + 2);
    const jsonl = sessoes.has(nome) ? sessoes.get(nome) ?? null : undefined;
    const trocou = !!jsonl && !!donos[chave] && donos[chave] !== jsonl;
    if (jsonl === undefined || (trocou && !presentes.has(chave))) {
      fecharNav(chave);
      navegadorNativo()?.close(chave);
      presentes.delete(chave);
      continue;
    }
    presentes.add(chave);
    if (jsonl && (!donos[chave] || trocou)) {
      donos[chave] = jsonl;
      mudou = true;
    }
  }
  if (mudou) salvarAbertos();
}

// O painel cola na direita: largura = janela - clientX (espelho do arrastarLargura do ctxPanel).
export function arrastarNav(clientX: number): void {
  if (typeof window === 'undefined') return;
  navegadorPanel.largura = clampLargura(window.innerWidth - clientX);
}

export function salvarNav(): void {
  try {
    localStorage.setItem(CHAVE, String(navegadorPanel.largura));
  } catch {
    /* sem storage: vale só nesta sessão */
  }
}
