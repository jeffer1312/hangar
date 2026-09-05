import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { applyTheme, getThemePref, getTextoDoDesktop } from './lib/theme';
import { buscarPaleta, aplicarPaleta, ligarAtualizacaoAoFocar } from './lib/desktopTheme';
import { applyBg, applyAppearance } from './lib/background';
import { ensureCookie, getBaseUrl, getToken, dropActiveServer } from './lib/auth';
import { localeAtual } from './lib/locale';
import { configureApi, configureLocale, configureDiag } from '@hangar/core';
import { registrar as registrarDiag, novoReq } from './lib/diag';

configureLocale({ getLocale: localeAtual });
// O diário de uso é da web (usa window/navigator e o auth daqui). O core só tem a tomada: sem este
// registro, o `apiFetch` segue funcionando e não grava nada.
configureDiag({ registrar: registrarDiag, novoReq });
configureApi({
  getBaseUrl,
  getToken,
  onUnauthorized: () => { dropActiveServer(); window.location.reload(); },
  origin: window.location.origin,
  createEventSource: (url, { withCredentials }) =>
    new EventSource(url, { withCredentials }) as unknown as import('@hangar/core').EventSourceLike,
});

// Pedaco que nao existe mais no servidor -> recarrega. A aba aberta ANTES de um deploy guarda um
// index.html que aponta pra hashes trocados; quando ela enfim pede um pedaco sob demanda (o realce
// de sintaxe, o visor de midia, o terminal), a resposta e 404 e a promise do import rejeita. Sem
// isto o toque simplesmente nao faz nada, e o unico rastro e um erro no console que ninguem abre.
// Enquanto o app era UM arquivo so, este caso nao existia: ou tudo carregava, ou nada carregava.
// O `skipWaiting`/`cleanupOutdatedCaches` do service worker nao cobre isto — ele serve a proxima
// carga, nao a aba que ja esta na tela.
// UMA vez por aba, e nao a cada evento: se o pedaco continuar faltando depois de recarregar, o
// problema nao e aba velha — e deploy incompleto —, e recarregar de novo vira a pagina piscando
// contra o servidor sem nada na tela dizendo por que. A flag NAO e limpa depois: o preco de nao
// recarregar de novo e ficar sem a cor do codigo ate a pessoa recarregar na mao, o que e degradado,
// nao quebrado.
const _CHAVE_RELOAD = 'cp_preload_reload';
window.addEventListener('vite:preloadError', (e) => {
  let jaTentou = false;
  try {
    jaTentou = sessionStorage.getItem(_CHAVE_RELOAD) !== null;
    if (!jaTentou) sessionStorage.setItem(_CHAVE_RELOAD, '1');
  } catch {
    // Aba anonima / storage bloqueado: sem memoria pra travar o laco, entao nem recarrega.
    console.error('pedaco do app nao carregou e nao da pra marcar a tentativa; recarregue a pagina', e);
    return;
  }
  if (jaTentou) {
    console.error('pedaco do app segue faltando depois de recarregar — deploy incompleto?', e);
    return;
  }
  location.reload();
});

// Resolve o tema (escolha do usuario ou prefers-color-scheme) ANTES de montar -> sem flash do default.
applyTheme();
// Fundo escolhido (chapado por padrao): antes de montar, pelo mesmo motivo do tema.
applyBg();
applyAppearance();
// Cookie do SSE: reescrito a cada boot (é por host e morria ao fechar o navegador).
ensureCookie();

// Rede do lang: o script inline do index.html ja escreveu o lang antes do primeiro paint; aqui
// corrigimos pro idioma que o Paraglide REALMENTE resolveu (caso o script nao tenha rodado) —
// leitor de tela e a hifenizacao do browser leem daqui.
document.documentElement.lang = localeAtual() === 'pt' ? 'pt-BR' : 'en';

// Tema do desktop: assincrono de proposito. O boot NAO espera a rede — a tela sobe com a paleta do
// app e repinta quando a resposta chega. Bloquear aqui prenderia o app a um backend fora do ar.
if (getThemePref() === 'desktop') {
  buscarPaleta().then((p) => { if (p) aplicarPaleta(p, getTextoDoDesktop()); });
}
// Rebusca quando a janela volta ao foco: o papel de parede e trocado no Control Center, fora daqui.
// Foco custa zero conexao persistente (SSE ja usa ~2 das ~6 por host); EventSource/poller nao entram.
ligarAtualizacaoAoFocar(() => getThemePref() === 'desktop', getTextoDoDesktop);

// Liquid glass (refracao SVG real) so funciona em Chromium: Safari/Firefox NAO suportam filtro SVG
// dentro de backdrop-filter (restricao WebKit). userAgentData existe SO em Chromium -> usa como gate.
// Onde nao tem (iOS/Safari), o glass fica no frosted (blur), que e o maximo possivel la.
if ((navigator as unknown as { userAgentData?: unknown }).userAgentData) {
  document.documentElement.setAttribute('data-liquid', '');
}

const app = mount(App, {
  target: document.getElementById('app')!,
});

// Register service worker (PWA)
if ('serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js').catch(() => {
      // SW registration fails silently in dev
    });
  });
}

export default app;
