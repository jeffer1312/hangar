// Oferecer (ou não) o botão "Gráfico" nas tabelas do chat.
//
// Default DESLIGADO por decisão do usuário: o recurso fica no repo, testado, mas fora do caminho
// até ele decidir se quer. Com a chave desligada o enhanceTables nem roda, então a tabela sai
// exatamente como sempre saiu.
//
// Mesmo padrão do toolLook/taskRows: chave no localStorage + $state, reage na hora, sem reload.
const CHART_KEY = 'cp_table_chart';

export type TableChartPref = 'off' | 'on';

function loadPref(): TableChartPref {
  try { return localStorage.getItem(CHART_KEY) === 'on' ? 'on' : 'off'; } catch { return 'off'; }
}

let pref = $state<TableChartPref>(loadPref());

export const tableChartPref = {
  get pref() { return pref; },
  get ativo() { return pref === 'on'; },
  set pref(v: TableChartPref) {
    pref = v;
    try {
      if (v === 'on') localStorage.setItem(CHART_KEY, 'on');
      else localStorage.removeItem(CHART_KEY);
    } catch { /* modo privado: vale pela sessão */ }
  },
};
