// Mostrar (ou não) a lista de tarefas do agente como cápsulas no fluxo da conversa.
//
// Chave SEPARADA da pele das ferramentas (`toolLook`): são duas decisões diferentes e o usuário
// pediu explicitamente pra poder ligar uma sem a outra. Default desligado — com ela desligada os
// TaskCreate/TaskUpdate seguem aparecendo como linha de ferramenta normal, como sempre foi.
//
// Mesmo padrão do navMode/toolLook: chave no localStorage + $state, reage na hora, sem reload.
const TASKS_KEY = 'cp_task_rows';

export type TaskRowsPref = 'off' | 'on';

function loadPref(): TaskRowsPref {
  try { return localStorage.getItem(TASKS_KEY) === 'on' ? 'on' : 'off'; } catch { return 'off'; }
}

let pref = $state<TaskRowsPref>(loadPref());

export const taskRows = {
  get pref() { return pref; },
  get ativo() { return pref === 'on'; },
  set pref(v: TaskRowsPref) {
    pref = v;
    try {
      if (v === 'on') localStorage.setItem(TASKS_KEY, 'on');
      else localStorage.removeItem(TASKS_KEY);
    } catch { /* modo privado: vale pela sessão */ }
  },
};
