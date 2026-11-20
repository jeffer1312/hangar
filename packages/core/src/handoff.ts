// Troca de agente (context handoff): o nome curto de cada modelo no divisor da conversa e na faixa da troca
// pendente. A mesma regra do `model_display` do nativo (desktop-native/src/app/controls/picker.rs).

const capitalized = (word: string): string => (word ? word[0]!.toUpperCase() + word.slice(1) : word);

function providerLabel(provider: string): string {
  if (provider === 'claude') return 'Claude';
  if (provider === 'codex') return 'Codex';
  return provider === 'omp' ? 'omp' : capitalized(provider);
}

/** `claude-opus-5-5` vira "Opus 5.5" e `gpt-6.2-sol` vira "GPT-6.2-Sol"; sem modelo, o nome do agente. */
export function modelDisplay(provider: string | null | undefined, model: string | null | undefined): string {
  const agent = provider ?? '';
  const id = (model ?? '').trim();
  if (!id) return providerLabel(agent);
  const wide = id.endsWith('[1m]');
  const base = wide ? id.slice(0, -4) : id;
  let shown: string;
  if (agent === 'claude') {
    const rest = base.startsWith('claude-') ? base.slice('claude-'.length) : base;
    // A data no fim do id (`20250929`) não entra no nome.
    const parts = rest.split('-').filter((p) => !/^\d{8}$/.test(p));
    const family = capitalized(parts[0] ?? '');
    const version = parts.slice(1);
    if (!version.length) shown = family;
    else if (version.every((p) => /^\d+$/.test(p))) shown = `${family} ${version.join('.')}`;
    else shown = `${family} ${version.map(capitalized).join(' ')}`;
  } else {
    shown = base.split('-').map((p) => (p.toLowerCase() === 'gpt' ? 'GPT' : capitalized(p))).join('-');
  }
  return wide ? `${shown} 1M` : shown;
}
