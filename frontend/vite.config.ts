import { defineConfig, type PluginOption } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { VitePWA } from 'vite-plugin-pwa'
import { build as esbuild } from 'esbuild'
import { fileURLToPath } from 'url'
import type { IncomingMessage, ServerResponse } from 'http'

// Serve /sw.js DE VERDADE no dev. Sem isto, um PWA que instalou o SW de um build (preview/prod)
// fica PRESO nesse build pra sempre: o update check de /sw.js recebe o fallback HTML do SPA
// (text/html), o browser rejeita como script de SW, o SW velho continua servindo o app shell do
// PRECACHE antigo — e nenhum deploy chega no celular (foi o iPhone rodando o build de 2026-07-18
// enquanto o vite dev tinha código novo). Aqui compilamos o src/sw.ts com precache VAZIO: o SW
// novo instala (skipWaiting), o precache some, e o app volta a puxar tudo da rede (HMR incluso).
// Push continua funcionando — os handlers de push/notificationclick vêm do sw.ts real.
function devServiceWorker(): PluginOption {
  const swSrc = fileURLToPath(new URL('./src/sw.ts', import.meta.url));
  const serveSw = async (req: IncomingMessage, res: ServerResponse, next: (err?: unknown) => void) => {
    // Match exato (não startsWith cru): /sw.jsx ou /sw.json não podem cair aqui.
    const path = req.url?.split('?')[0];
    if (path !== '/sw.js') return next();
    // try/catch obrigatório: connect não espera nem captura a promise do handler — um esbuild
    // rejeitado viraria unhandled rejection (request pendurada, e Node pode derrubar o processo).
    try {
      const out = await esbuild({
        entryPoints: [swSrc],
        bundle: true,
        write: false,
        define: { 'self.__WB_MANIFEST': '[]' }, // dev: nada precacheado -> tudo vem da rede
      });
      res.setHeader('Content-Type', 'text/javascript');
      res.setHeader('Cache-Control', 'no-cache');
      res.end(out.outputFiles[0].text);
    } catch (err) {
      next(err);
    }
  };
  // Só no dev (apply: 'serve') — o preview já serve o dist/sw.js buildado de verdade.
  return { name: 'dev-service-worker', apply: 'serve', configureServer(s) { s.middlewares.use(serveSw); } };
}

// Multi-PC: quando o app (servido por uma origem, ex: casa) fala com o backend de OUTRA maquina
// (ex: trabalho), o browser faz preflight CORS (OPTIONS) por causa do header Authorization. O vite
// proxy repassa o ACAO=* do backend no GET/POST, MAS responde o OPTIONS sozinho (204 SEM CORS) ->
// o browser bloqueia. Este plugin responde o OPTIONS de /api com os headers CORS (token continua
// exigido pelo backend nas chamadas reais — preflight nao carrega credencial).
function apiCorsPreflight(): PluginOption {
  // Mesmo handler no dev (configureServer) e no BUILD servido (configurePreviewServer) -> o
  // `vite preview` se comporta igual ao dev pro preflight de /api.
  const preflight = (req: IncomingMessage, res: ServerResponse, next: () => void) => {
    if (req.method === 'OPTIONS' && req.url?.startsWith('/api')) {
      const reqHeaders = req.headers['access-control-request-headers'] ?? 'authorization,content-type';
      res.setHeader('Access-Control-Allow-Origin', '*');
      res.setHeader('Access-Control-Allow-Methods', 'GET,POST,DELETE,OPTIONS');
      res.setHeader('Access-Control-Allow-Headers', reqHeaders);
      res.setHeader('Access-Control-Max-Age', '86400');
      res.statusCode = 204;
      res.end();
      return;
    }
    next();
  };
  return {
    name: 'api-cors-preflight',
    configureServer(server) { server.middlewares.use(preflight); },
    configurePreviewServer(server) { server.middlewares.use(preflight); },
  };
}

export default defineConfig({
  server: {
    // Bind IPv4 loopback: vite default `localhost` resolve pra ::1 (IPv6-only) nesta maquina,
    // mas o `tailscale serve` proxia pra 127.0.0.1:5173 (IPv4) -> sem isto da 502. Forca IPv4.
    host: '127.0.0.1',
    // Desliga o CORS embutido do vite: ele só libera origens localhost e intercepta o preflight
    // OPTIONS de /api SEM setar Access-Control-Allow-Origin pra origens .ts.net (quebrava o
    // multi-PC cross-origin). Com false, o preflight cai no apiCorsPreflight (plugin) / no backend.
    cors: false,
    // Dev: proxy /api to the backend so the browser talks same-origin — no CORS, and
    // the cp_token cookie reaches the EventSource (SSE). Mirrors the prod reverse-proxy.
    // Leave the Login "URL do servidor" empty in dev so requests stay relative.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8765',
        changeOrigin: true,
      },
    },
    // Allow access via the Tailscale MagicDNS host (tailscale serve → this dev server),
    // so the phone can reach it over trusted HTTPS on the tailnet.
    allowedHosts: ['.ts.net'],
  },
  // `vite preview` serve o BUILD na MESMA URL/porta do dev (5173, atras do `tailscale serve`).
  // Espelha o server: proxy /api -> backend (same-origin -> cookie cp_token chega no SSE), hosts do
  // tailnet, porta fixa. Sem isto o preview nao herdaria o proxy (e o app nao falaria com o backend).
  preview: {
    port: 5173,
    strictPort: true,
    cors: false,
    proxy: {
      '/api': { target: 'http://127.0.0.1:8765', changeOrigin: true },
    },
    // .omniwise.com.br: a VPS (srv1633222) serve o BUILD via preview atrás do traefik do Coolify
    // (Host pocket.omniwise.com.br chega intacto no vite; sem isto o preview responde 403).
    allowedHosts: ['.ts.net', '.omniwise.com.br'],
  },
  plugins: [
    apiCorsPreflight(),
    devServiceWorker(),
    svelte(),
    VitePWA({
      registerType: 'autoUpdate',
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.ts',
      injectManifest: {
        swSrc: 'src/sw.ts',
        swDest: 'dist/sw.js',
      },
      manifest: {
        name: 'Claude Cockpit',
        short_name: 'Cockpit',
        display: 'standalone',
        background_color: '#100e11',
        theme_color: '#100e11',
        orientation: 'portrait',
        start_url: '/',
        icons: [
          { src: '/icons/icon-192.png', sizes: '192x192', type: 'image/png', purpose: 'any' },
          { src: '/icons/icon-512.png', sizes: '512x512', type: 'image/png', purpose: 'any' },
          { src: '/icons/icon-180.png', sizes: '180x180', type: 'image/png' },
          { src: '/icons/icon-maskable-512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
        ],
      },
    }),
  ],
})
