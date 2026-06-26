import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { applyTheme } from './lib/theme';

// Resolve o tema (escolha do usuario ou prefers-color-scheme) ANTES de montar -> sem flash do default.
applyTheme();

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
