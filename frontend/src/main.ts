import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { applyTheme } from './lib/theme';

// Resolve o tema (escolha do usuario ou prefers-color-scheme) ANTES de montar -> sem flash do default.
applyTheme();

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
