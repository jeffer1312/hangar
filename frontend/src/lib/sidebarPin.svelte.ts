// Pin do trilho da Sidebar: separa a PREFERÊNCIA persistida (o que o usuário clicou) do
// override TEMPORÁRIO (auto-recolher do Board/Canvas). O board/canvas forçam o recolhido
// enquanto estão abertos, mas não podem virar preferência gravada — ao sair, volta o pin.
const KEY = 'cp_sidebar_collapsed';
function load(): boolean {
  try { return localStorage.getItem(KEY) === '1'; } catch { return false; }
}
let preferred = $state(load());
let forced = $state<boolean | null>(null);

export const sidebarPin = {
  get collapsed() { return forced ?? preferred; },
  get preferred() { return preferred; },
  // Override ativo (Board/Canvas segurando o recolhido). Quem precisa distinguir "recolhido por
  // preferência" (expandir é ação legítima) de "recolhido por override" (expandir é clique morto)
  // lê isto — `collapsed` sozinho não separa os dois (round 7).
  get forcedOverride() { return forced; },
  setUser(value: boolean) {
    preferred = value;
    try { localStorage.setItem(KEY, value ? '1' : '0'); } catch { /* modo privado */ }
  },
  toggleUser() { this.setUser(!preferred); },
  setForced(value: boolean | null) { forced = value; },
};
