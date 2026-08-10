// Modo da navegação com a sidebar RECOLHIDA (follow-up visual round 2): trilho vertical de
// iniciais ('rail', o PADRÃO — decisão do usuário, referência do rail completo) ou faixa
// horizontal de abas ('tabs', a SessionTabs desta branch). Mesmo padrão do sidebarPrefs:
// chave localStorage + $state + persistência sem reload — a escolha reage na hora, sem reload.
const MODE_KEY = 'cp_nav_mode';

export type NavMode = 'rail' | 'tabs';

// Ausência de chave = 'rail': quem nunca configurou recebe o trilho (default decidido pelo usuário).
function loadMode(): NavMode {
  try { return localStorage.getItem(MODE_KEY) === 'tabs' ? 'tabs' : 'rail'; } catch { return 'rail'; }
}

let mode = $state<NavMode>(loadMode());

export const navMode = {
  get mode() { return mode; },
  set mode(v: NavMode) {
    mode = v;
    try {
      if (v === 'tabs') localStorage.setItem(MODE_KEY, 'tabs');
      else localStorage.removeItem(MODE_KEY);
    } catch { /* modo privado: vale pela sessão */ }
  },
};
