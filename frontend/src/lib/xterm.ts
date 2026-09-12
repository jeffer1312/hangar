// Montagem do xterm.js compartilhada pelo painel do desktop (TerminalPanel) e pelo terminal do
// celular (TerminalMobile). Aqui moram os dois detalhes que ja custaram caro uma vez — o fundo
// transparente e a fonte resolvida — pra nao existirem em duas copias que divergem.
import type { Terminal } from '@xterm/xterm';
import type { FitAddon } from '@xterm/addon-fit';

// Cores do xterm a partir dos tokens do app. `color`, propriedade REAL (nao custom property): o
// browser sempre entrega ela RESOLVIDA, mesmo quando --text-primary e um color-mix() com var()
// aninhado (app.css:323, o boost de texto sobre papel de parede) -- ler a CUSTOM PROPERTY direto
// devolveria a string CRUA com var() por dentro (e assim que a spec de CSS Custom Properties define
// o computed value delas: sem substituir var() aninhado), o xterm rejeitava calado e caia no branco
// padrao. `body` ja seta `color: var(--text-primary)` (app.css) e `host` herda -- de graca, sem
// elemento nem estilo extra. --accent, ao contrario, e hex LITERAL nas duas paletas do app.css (sem
// var() aninhado), entao ler a custom property direto e seguro ali.
// A chave e `foreground`, NAO `fg`: o xterm ignora chave desconhecida em silencio e cai no branco
// padrao dele. Ficou errado por semanas sem ninguem ver porque o objeto entra no `theme` por
// ESPALHAMENTO (`{ background: ..., ...lerTema(el) }`), e propriedade vinda de spread escapa da
// checagem de excesso do TypeScript -- `svelte-check` passava limpo com o tema nunca sendo aplicado.
export function lerTema(el: HTMLElement): { foreground: string; cursor: string } {
  const cs = getComputedStyle(el);
  const foreground = cs.color || '#d2cbcd';
  const cursor = cs.getPropertyValue('--accent').trim() || foreground;
  return { foreground, cursor };
}

// 'rgba(0, 0, 0, 0)', NAO 'transparent': o parser de cor do xterm 6.0.0 (Color.ts) so casa
// hex/rgb()/rgba() -- 'transparent' cai no caminho do canvas e LANCA (alfa != 255 e rejeitado), o
// ThemeService ENGOLE a excecao calado e devolve o fallback #000000 opaco por cima do
// --surface-inset do painel (regra de vidro do CLAUDE.md). Medido no pacote instalado -- nao
// aparece nenhum erro no console, so o retangulo preto.
export const FUNDO_TRANSPARENTE = 'rgba(0, 0, 0, 0)';

export function temaDe(el: HTMLElement) {
  return { background: FUNDO_TRANSPARENTE, ...lerTema(el) };
}

export function novoTerminal(
  hostEl: HTMLDivElement,
  TerminalCls: typeof Terminal,
  FitAddonCls: typeof FitAddon,
  fontSize = 12,
) {
  // getComputedStyle, nao `var(--font-mono)` cru: o renderer canvas monta
  // `ctx.font = \`${size}px ${family}\``, onde var() e invalido e ignorado calado -> metrica de
  // glifo errada e grade desalinhada.
  const mono = getComputedStyle(hostEl).getPropertyValue('--font-mono').trim() || 'monospace';
  const t = new TerminalCls({
    fontFamily: mono, fontSize, convertEol: false,
    allowTransparency: true,
    theme: temaDe(hostEl),
  });
  // Cala a RESPOSTA do xterm as perguntas de cor (OSC 4 indexada, 10 frente, 11 fundo, 12 cursor).
  // Nao e o xterm falando demais: no Windows quem pergunta e o psmux ao anexar um cliente, e ele nao
  // consome a resposta — ela desce pro pane e a TUI a engole como DIGITACAO (os "434"/"33" que
  // apareciam na frente do texto no composer sao pedacos de `]4;3;rgb:c4c4/...`). Handler
  // registrado depois do mount vem antes do embutido e, devolvendo true, a resposta nunca e gerada.
  // Custo: TUI que pergunte cor recebe silencio e usa o padrao dela — melhor que lixo no composer.
  // Engolir tambem a DEFINICAO de cor (o mesmo OSC sem `?`) e de brinde: OSC 10/11/12 pintariam por
  // cima do fundo transparente que o painel de vidro exige. Nao guardamos os IDisposable: quem monta
  // o terminal so o desfaz com `dispose()`, que leva os handlers junto.
  for (const osc of [4, 10, 11, 12]) t.parser.registerOscHandler(osc, () => true);
  const f = new FitAddonCls();
  t.loadAddon(f);
  t.open(hostEl);
  f.fit();
  return { term: t, fit: f };
}
