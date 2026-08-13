// Preferencia de idioma do app. O Paraglide resolve o locale sozinho pela cadeia
// localStorage -> preferredLanguage -> baseLocale (vite.config.ts); este modulo so cuida da
// TERCEIRA opcao da tela, "Seguir o sistema", que nao e um valor: e a AUSENCIA do override.
// A chave abaixo foi lida de src/paraglide/runtime.js (gerado) — nao adivinhar.
import { getLocale, setLocale, overwriteSetLocale, baseLocale } from '../paraglide/runtime';

export const LOCALES = ['en', 'pt'] as const; // ingles primeiro: e o baseLocale
export type Locale = (typeof LOCALES)[number];
export type Preferencia = Locale | 'sistema';

const CHAVE = 'PARAGLIDE_LOCALE'; // valor confirmado no Step 5

// O getLocale() do Paraglide chama setLocale(resolved, { reload: false }) na PRIMEIRA
// resolucao (runtime.js:184, issue opral/inlang-paraglide-js#455) e isso GRAVA a chave no
// localStorage — apagando a diferenca entre "o usuario escolheu" e "o app resolveu sozinho".
// So persistimos quando a escrita partiu daqui. O discriminador e a NOSSA intencao, nao o
// formato da chamada da biblioteca: olhar `opcoes.reload` quebra calado no dia em que o
// Paraglide mudar a assinatura da chamada interna.
let escritaManual = false;
const setLocaleOriginal = setLocale;
overwriteSetLocale((novo, opcoes) => {
  if (!escritaManual) return;
  return setLocaleOriginal(novo, opcoes);
});

export function localeAtual(): Locale {
  const l = getLocale();
  return (LOCALES as readonly string[]).includes(l) ? (l as Locale) : (baseLocale as Locale);
}

// Valor fora da lista conta como "sem escolha": storage e editavel pelo usuario e por extensao,
// e um "klingon" ali nao pode deixar a tela sem nenhuma opcao marcada.
export function preferenciaSalva(): Preferencia {
  const v = localStorage.getItem(CHAVE);
  return v && (LOCALES as readonly string[]).includes(v) ? (v as Locale) : 'sistema';
}

// `recarregar` e injetado pra o teste conseguir observar a chamada; em producao e o reload real.
// O reload existe porque as mensagens do Paraglide sao funcoes compiladas, nao valores reativos:
// sem recarregar, a tela continuaria mostrando o idioma anterior.
export function aplicarPreferencia(p: Preferencia, recarregar: () => void = () => location.reload()): void {
  if (p === 'sistema') {
    localStorage.removeItem(CHAVE);
    recarregar();
    return;
  }
  // try/finally: se o setLocale estourar, a flag nao pode ficar ligada — a proxima chamada
  // interna do getLocale passaria e a auto-gravacao voltaria.
  escritaManual = true;
  try {
    setLocale(p); // o proprio setLocale ja recarrega por padrao
  } finally {
    escritaManual = false;
  }
}
