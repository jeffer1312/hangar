// Aparência das chamadas de ferramenta no chat: 'classico' (o bloco de duas linhas de sempre,
// "● Bash <arg>" / "└ Pronto (38 linhas) • clique para ver") ou 'chips' (a pele portada do
// beautiful-ui: uma linha por chamada, o argumento num chip, o detalhe abrindo embaixo, e a faixa
// de chips de diff no fim da rodada).
//
// É INTERRUPTOR, não migração: o default é 'classico', então quem não mexer não vê diferença
// nenhuma, e voltar é um clique. As duas peles leem exatamente os MESMOS dados derivados do
// ToolCard — nada de comportamento muda entre elas (o diff da edição, o erro em texto, o realce
// do Read e o anexo de imagem valem nas duas).
//
// Mesmo padrão do navMode/sidebarPrefs: chave no localStorage + $state, reage na hora, sem reload.
const LOOK_KEY = 'cp_tool_look';

export type ToolLook = 'classico' | 'chips';

// Ausência de chave = 'classico': ninguém é migrado por acidente.
function loadLook(): ToolLook {
  try { return localStorage.getItem(LOOK_KEY) === 'chips' ? 'chips' : 'classico'; } catch { return 'classico'; }
}

let look = $state<ToolLook>(loadLook());

export const toolLook = {
  get look() { return look; },
  set look(v: ToolLook) {
    look = v;
    try {
      if (v === 'chips') localStorage.setItem(LOOK_KEY, 'chips');
      else localStorage.removeItem(LOOK_KEY);
    } catch { /* modo privado: vale pela sessão */ }
  },
};
