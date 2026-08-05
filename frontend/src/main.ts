import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { applyTheme, getThemePref, getTextoDoDesktop } from './lib/theme';
import { buscarPaleta, aplicarPaleta, ligarAtualizacaoAoFocar } from './lib/desktopTheme';
import { applyBg, applyAppearance } from './lib/background';
import { ensureCookie } from './lib/auth';

// Resolve o tema (escolha do usuario ou prefers-color-scheme) ANTES de montar -> sem flash do default.
applyTheme();
// Fundo escolhido (chapado por padrao): antes de montar, pelo mesmo motivo do tema.
applyBg();
applyAppearance();
// Cookie do SSE: reescrito a cada boot (é por host e morria ao fechar o navegador).
ensureCookie();

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
