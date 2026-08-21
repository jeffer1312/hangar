// Visibilidade da faixa de cota do rodapé (a "barra de baixo"). Desde a pílula do topo
// (QuotaPill) a faixa nasce RECOLHIDA: o popover é o detalhe sob demanda, e a faixa fixa vira o
// modo expandido ("Mostrar na barra" no rodapé do popover). Persiste em localStorage.
const KEY = 'cp_quota_barra';

let aberta = $state(typeof localStorage !== 'undefined' && localStorage.getItem(KEY) === '1');

export const quotaBarra = {
  get aberta() { return aberta; },
  alternar() {
    aberta = !aberta;
    if (typeof localStorage === 'undefined') return;
    if (aberta) localStorage.setItem(KEY, '1');
    else localStorage.removeItem(KEY);
  },
};
