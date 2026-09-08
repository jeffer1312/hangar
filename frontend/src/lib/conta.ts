// Chip da conta de uma sessão, a partir do id que a lista já traz (`conta`: "claude:<config dir>",
// "chave:<motor>", "codex:<dir>"…). Só a conta Anthropic vira chip: motor e provider já têm os
// seus (⚙ e o glifo). O nome é o sufixo da pasta — o mesmo que `hangar-conta` e `--conta` usam.
import * as m from '../paraglide/messages';

// `nome` é o que `--conta` aceita; `label` tira um `claude-` repetido do início (a pasta já é
// `.claude-…`), porque o chip disputa a barra do celular com o nome da sessão.
export interface ContaChip { label: string; nome: string; cor: string }

// Seis tintas distinguíveis entre si e do estado (âmbar/verde/roxo já são estado).
const CORES = ['#5ec8d8', '#f0a05a', '#7fd68a', '#e78ac3', '#8fa8ff', '#d9c15b'];

export function chipDaConta(conta: string | null | undefined): ContaChip | null {
  if (!conta || !conta.startsWith('claude:')) return null;
  const base = conta.slice('claude:'.length).replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? '';
  const nome = base === '.claude' ? '' : base.replace(/^\.claude-/, '');
  const label = nome.replace(/^claude-(?=.)/, '') || m.conta_padrao();
  let h = 0;
  for (const ch of base) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return { label, nome: nome || m.conta_padrao(), cor: CORES[h % CORES.length] };
}
