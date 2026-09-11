// Chip da conta de uma sessão, a partir do id que a lista já traz (`conta`: "claude:<config dir>",
// "chave:<motor>", "codex:<dir>"…). Conta de ASSINATURA vira chip — Anthropic e ChatGPT/Codex, que
// desde as contas Codex adicionais também são várias e mudam a cota de quem paga. Motor (`chave:`)
// fica de fora: ele já tem o ⚙ e o consumo é do provedor, não de uma conta nomeada. O nome é o
// sufixo da pasta — o mesmo que `hangar-conta` e `--conta` usam.
import * as m from '../paraglide/messages';

// `nome` é o que `--conta` aceita; `label` tira um `claude-` repetido do início (a pasta já é
// `.claude-…`), porque o chip disputa a barra do celular com o nome da sessão.
export interface ContaChip { label: string; nome: string; cor: string }

// Seis tintas distinguíveis entre si e do estado (âmbar/verde/roxo já são estado). Dessaturadas em
// 45% mantendo a luminância: a cor aqui é identidade, não aviso, e pintava o chip inteiro de rosa ou
// ciano no meio de uma lista que já tem estado colorido. Hoje ela só tinge o ponto de 5px do chip
// (o texto é --text-secondary), então distinguir importa mais que saltar aos olhos.
const CORES = ['#80bbc3', '#d3a781', '#97c69d', '#cd99b9', '#9ba9d9', '#ccbf87'];

export function chipDaConta(conta: string | null | undefined): ContaChip | null {
  const harness = ['claude', 'codex'].find((h) => conta?.startsWith(`${h}:`));
  if (!conta || !harness) return null;
  const base = conta.slice(harness.length + 1).replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? '';
  const nome = base === `.${harness}` ? '' : base.replace(new RegExp(`^\\.${harness}-`), '');
  const label = nome.replace(/^claude-(?=.)/, '') || m.conta_padrao();
  let h = 0;
  for (const ch of base) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return { label, nome: nome || m.conta_padrao(), cor: CORES[h % CORES.length] };
}
