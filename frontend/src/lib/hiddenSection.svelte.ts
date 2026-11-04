// Aberto/fechado da seção "Ocultas" é do aparelho. Módulo, não instância: o atalho de
// Configurações › Sessões abre a seção com a lista já montada atrás do painel.
const KEY = 'cp_hidden_section_open';

function read(): boolean {
  try { return localStorage.getItem(KEY) === '1'; } catch { return false; }
}

let open = $state(read());

export const hiddenSection = {
  get open() { return open; },
  set open(v: boolean) {
    open = v;
    try { localStorage.setItem(KEY, v ? '1' : '0'); } catch { /* modo privado */ }
  },
};
