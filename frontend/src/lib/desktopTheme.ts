// Tema vindo do desktop: o app pinta neutros, vidro e destaque com a paleta Material You que o rice
// (quickshell/end-4) gera do papel de parede. Ver
// docs/superpowers/specs/2026-08-05-tema-do-sistema-design.md.
import { getBaseUrl, getToken } from './auth';

export type Paleta = { escuro: boolean; cores: Record<string, string> };

// Todo token que `mapear()` le de `p.cores` — a mesma lista que decide o que valida em
// `buscarPaleta()`. Um so lugar: se `mapear` passar a ler mais uma chave, o validador segue ela
// sem precisar lembrar de atualizar duas listas (a outra fonte, `TOKENS`, e do BACKEND).
const CHAVES_CORES = [
  'background', 'surfaceContainerLow', 'surfaceContainer', 'surfaceContainerHigh',
  'primary', 'outlineVariant', 'outline', 'onPrimary', 'onSurface', 'onSurfaceVariant',
] as const;

// A paleta e da MAQUINA onde o backend roda. Nao basta a pagina ter vindo do localhost: o app troca
// de SERVIDOR ativo (o mesmo front fala com varias maquinas), e um servidor remoto devolveria 403 —
// ou, pior, a paleta do papel de parede DELE. Entao o gate compara a origem do pedido com a da
// pagina. `getBaseUrl()` vazio significa mesma origem.
export function ehLocal(): boolean {
  if (typeof location === 'undefined') return false;
  const base = getBaseUrl();
  return base === '' || base === location.origin;
}

function hexPraRgb(hex: string): string {
  const n = parseInt(hex.slice(1), 16);
  return `${(n >> 16) & 255} ${(n >> 8) & 255} ${n & 255}`;
}

function hexPraRgba(hex: string, alfa: number): string {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alfa})`;
}

// Escurece um hex — o `--accent-press` precisa ser visivelmente diferente do `--accent`, senao o
// botao nao da retorno nenhum ao ser apertado.
function escurecer(hex: string, fator = 0.82): string {
  const n = parseInt(hex.slice(1), 16);
  const canais = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  let c = canais.map((v) => Math.round(v * fator));
  // Um `$primary` quase preto nao tem pra onde escurecer — ai o retorno de toque vem CLAREANDO.
  // Sem isto, --accent-press sairia igual a --accent e o botao nao responderia ao toque.
  if (c.every((v, i) => v === canais[i])) {
    c = canais.map((v) => Math.round(v + (255 - v) * 0.18));
  }
  return `#${c.map((v) => v.toString(16).padStart(2, '0')).join('')}`;
}

// Toda chave que o tema escreve. Existe como constante (e nao derivada na hora) porque `limpar`
// precisa saber o que tirar mesmo tendo sido chamada num boot diferente do que aplicou.
export const CHAVES = [
  '--bg-base', '--bg-surface', '--bg-elevated', '--bg-hover',
  '--veu-rgb', '--veu-amostra-rgb',
  '--glass-panel-rgb', '--glass-rgb', '--glass-solid-rgb',
  '--accent', '--accent-press', '--accent-dim',
  '--border-subtle', '--border-default', '--border-strong',
  '--text-inverse', '--bubble-user',
  '--text-primary', '--text-secondary', '--text-muted',
  '--text-primary-base', '--text-secondary-base', '--text-muted-base',
] as const;

// Material You -> tokens do app.css. O que nao esta aqui NAO muda: `--success`/`--error`/`--warning`
// sao semantica (verde e verde), os `--pill-*` sao ESTADO da sessao, e `--chart-2..4` passaram por
// validador de daltonismo, compromisso que uma paleta tirada de foto nao tem.
export function mapear(p: Paleta, textoDoDesktop: boolean): Record<string, string> {
  const c = p.cores;
  const m: Record<string, string> = {
    '--bg-base': c.background,
    '--bg-surface': c.surfaceContainerLow,
    '--bg-elevated': c.surfaceContainer,
    '--bg-hover': c.surfaceContainerHigh,
    // Veu do body::after (app.css): MESMA fonte do --bg-base (c.background) — o veu e a base sao a
    // mesma cor por natureza, so viraram tokens separados pro app.css poder ficar bit-a-bit igual
    // fora do tema Desktop mesmo quando o literal do veu tinha driftado do --bg-base de fabrica.
    '--veu-rgb': hexPraRgb(c.background),
    // Veu da AMOSTRA (AparenciaAmostra.svelte): MESMA fonte (c.background) — dois nomes, um valor.
    // Precisa ser um token separado do `--veu-rgb` de cima porque o fallback de fabrica da amostra
    // no claro (255 253 250) e diferente do de app.css (250 247 243); um nome so forcaria os dois a
    // convergir pro mesmo literal, que e exatamente a mudanca visual que esta tarefa nao autoriza.
    '--veu-amostra-rgb': hexPraRgb(c.background),
    // Vidro: so a COR. A alfa continua vindo do slider Transparencia (background.ts aplicarScrim);
    // escrever `--glass-panel` inteiro aqui ganharia de tudo e mataria o slider.
    '--glass-panel-rgb': hexPraRgb(c.surfaceContainerLow),
    '--glass-rgb': hexPraRgb(c.surfaceContainerHigh),
    '--glass-solid-rgb': hexPraRgb(c.background),
    '--accent': c.primary,
    '--accent-press': escurecer(c.primary),
    '--accent-dim': hexPraRgba(c.primary, 0.18),
    // Bordas: cor do outline, ALFAS de hoje (app.css:16-18). Opaco aqui deixaria a tela riscada.
    '--border-subtle': hexPraRgba(c.outlineVariant, 0.07),
    '--border-default': hexPraRgba(c.outlineVariant, 0.12),
    '--border-strong': hexPraRgba(c.outline, 0.22),
    // Texto SOBRE o destaque: com `$primary` claro, branco sumiria.
    '--text-inverse': c.onPrimary,
    // Bolha do usuario: e superficie neutra, nao estado — sem ela, foto quente + bolha cinza-indigo.
    '--bubble-user': c.surfaceContainerHigh,
  };
  if (textoDoDesktop) {
    m['--text-primary'] = c.onSurface;
    m['--text-secondary'] = c.onSurfaceVariant;
    m['--text-muted'] = c.outline;
    // As copias `-base` sao de onde o modo "Texto sobre foto" mistura; sem elas, ligar papel de
    // parede clarearia a partir do tom ANTIGO.
    m['--text-primary-base'] = c.onSurface;
    m['--text-secondary-base'] = c.onSurfaceVariant;
    m['--text-muted-base'] = c.outline;
  }
  return m;
}

export function aplicarPaleta(p: Paleta, textoDoDesktop: boolean): void {
  if (typeof document === 'undefined') return;
  // Computa ANTES de tocar no DOM: um throw dentro de `mapear` (paleta malformada que passou pela
  // validacao de `buscarPaleta` por algum outro caminho) nao pode deixar o tema flipado com as
  // cores do estado anterior — nada parcial pode ficar observavel.
  const m = mapear(p, textoDoDesktop);
  const raiz = document.documentElement;
  // A VARIANTE vem do arquivo, nao da escolha do app: pedir "Desktop" e "Claro" ao mesmo tempo
  // daria paleta escura com o bloco claro do CSS por cima.
  raiz.dataset.theme = p.escuro ? 'dark' : 'light';
  // Limpa antes de escrever: alternar "cor do texto" de desktop pra app deixaria as chaves de texto
  // presas no valor anterior, porque o mapa novo simplesmente nao as traz.
  for (const k of CHAVES) raiz.style.removeProperty(k);
  for (const [k, v] of Object.entries(m)) raiz.style.setProperty(k, v);
  sincronizarMeta();
}

export function limparPaleta(): void {
  if (typeof document === 'undefined') return;
  for (const k of CHAVES) document.documentElement.style.removeProperty(k);
  sincronizarMeta();
}

// A barra do Safari/PWA e tingida pelo <meta theme-color>; `theme.ts` a sincroniza no applyTheme,
// mas quem troca a cor aqui somos nos, depois dele.
function sincronizarMeta(): void {
  if (typeof getComputedStyle === 'undefined') return;
  const bg = getComputedStyle(document.documentElement).getPropertyValue('--bg-base').trim();
  const meta = document.querySelector('meta[name="theme-color"]') as HTMLMetaElement | null;
  if (bg && meta) meta.content = bg;
}

// Ultima paleta que respondeu com sucesso, pro app ler SEM esperar rede: gate do ThemeToggle e
// reaplicar() do AppearanceSettings usam isto pra nao ficar mudos quando o fetch falha (backend
// piscou) nem pagar um GET por toque. So anda pra frente — uma falha depois de um sucesso NAO apaga
// o cache, ela so nao atualiza.
let ultima: Paleta | null = null;

export function paletaEmCache(): Paleta | null {
  return ultima;
}

// Confere a forma antes de confiar no JSON: hoje SO o backend deste repo responde este endpoint,
// mas o app fala com VARIOS servidores (troca de servidor ativo, malha cp-send) — um deles rodando
// versao velha, ou qualquer coisa respondendo 200 na mesma rota, e o front nao tem defesa propria
// nenhuma sem isto. `unknown` -> checagem manual, nao cast: um cast (`as Paleta`) so engana o
// compilador, nao troca o dado que chegou.
function paletaValida(v: unknown): v is Paleta {
  if (typeof v !== 'object' || v === null) return false;
  const cores = (v as Record<string, unknown>).cores;
  if (typeof cores !== 'object' || cores === null) return false;
  return CHAVES_CORES.every((k) => typeof (cores as Record<string, unknown>)[k] === 'string');
}

export async function buscarPaleta(): Promise<Paleta | null> {
  if (!ehLocal()) return null;
  try {
    const r = await fetch(`${getBaseUrl()}/api/desktop/palette`, {
      headers: { Authorization: `Bearer ${getToken() ?? ''}` },
    });
    if (!r.ok) return null;   // 404 = maquina sem rice; 403 = nao e a maquina. Os dois: sem opcao.
    const bruto: unknown = await r.json();
    if (!paletaValida(bruto)) {
      console.warn('desktopTheme: paleta com formato inesperado, ignorando', bruto);
      return null;            // mesma resposta de um 404: "sem paleta", nao erro
    }
    ultima = bruto;
    return bruto;
  } catch {
    return null;              // backend fora do ar nao pode impedir o app de abrir
  }
}

// Sem SSE, de proposito (decisao de 05/08/2026): o papel de parede e trocado no Control Center, ou
// seja FORA da janela do app. Voltar o foco e exatamente o instante em que repintar importa — e
// custa zero conexao persistente, num navegador que so permite ~6 por host e num app que ja usa duas.
let ligado = false;

export function ligarAtualizacaoAoFocar(quer: () => boolean, texto: () => boolean): void {
  if (ligado || typeof window === 'undefined') return;
  ligado = true;
  const rebuscar = () => {
    if (!quer() || !ehLocal()) return;
    buscarPaleta().then((p) => { if (p) aplicarPaleta(p, texto()); });
  };
  window.addEventListener('focus', rebuscar);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) rebuscar(); });
}
