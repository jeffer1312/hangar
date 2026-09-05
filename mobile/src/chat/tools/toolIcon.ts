import type { IconName } from '../../ui/Icon';

// Ícone do VERBO da ferramenta: é ele que dá a leitura rápida de uma pilha de cards. Nome
// desconhecido (ferramenta de MCP, provider novo) cai na chave inglesa em vez de sumir.
const MAPA: Record<string, IconName> = {
  Bash: 'Terminal', Read: 'Eye', Edit: 'Pencil', MultiEdit: 'Pencil', Write: 'FilePlus', NotebookEdit: 'Pencil',
  Grep: 'Search', Glob: 'FolderSearch', WebSearch: 'Globe', WebFetch: 'Globe', ToolSearch: 'Puzzle',
  Agent: 'Bot', Task: 'Bot', TaskCreate: 'ListTodo', TaskUpdate: 'ListTodo', AskUserQuestion: 'CircleHelp',
  Skill: 'Sparkles', Workflow: 'Workflow',
};

export function toolIcon(nome?: string | null): IconName {
  return (nome && MAPA[nome]) || 'Wrench';
}
