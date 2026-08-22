import type { AskQuestionPayload, ChatEvent, Provider } from './types';

// Porte de Chat.svelte:659-724. Pendente = último tool_use 'question' (Pi) / 'AskUserQuestion'
// (Kimi) sem tool_result com o mesmo tool_use_id. Claude não entra aqui: ele chega pelo SSE
// `ask_question`, via hook (askq_capture.py).
export function pendingAskFromEvents(events: ChatEvent[], provider: Provider | null | undefined): ChatEvent | null {
  const toolName = provider === 'pi' ? 'question' : provider === 'kimi' ? 'AskUserQuestion' : null;
  if (!toolName) return null;
  const answered = new Set<string>();
  let last: ChatEvent | null = null;
  for (const ev of events) {
    if (ev.kind === 'tool_result' && ev.tool_use_id) answered.add(ev.tool_use_id);
    else if (ev.kind === 'tool_use' && ev.tool_name === toolName && ev.tool_use_id) last = ev;
  }
  return last && !answered.has(last.tool_use_id ?? '') ? last : null;
}

const mapOpts = (opts: unknown) =>
  (Array.isArray(opts) ? opts : [])
    .map((o) => ({
      label: String((o as Record<string, unknown> | null)?.label ?? ''),
      description: String((o as Record<string, unknown> | null)?.description ?? ''),
    }))
    .filter((o) => o.label);

// null = shape inesperado (quem chama loga e deixa o OptionButtons cru como saída).
export function askPayloadFromToolUse(ev: ChatEvent, provider: Provider): AskQuestionPayload | null {
  const args = (ev.tool_input ?? {}) as Record<string, unknown>;
  if (provider === 'kimi') {
    const qs = (Array.isArray(args.questions) ? args.questions : [])
      .map((item) => {
        const it = item as Record<string, unknown> | null;
        return {
          header: String(it?.header ?? ''),
          question: String(it?.question ?? ''),
          multiSelect: it?.multi_select === true,
          options: mapOpts(it?.options),
        };
      })
      .filter((item) => item.question && item.options.length);
    return qs.length ? { questions: qs } : null;
  }
  const options = mapOpts(args.options);
  if (!options.length || !args.question) return null;
  return {
    questions: [{
      header: String(args.header ?? ''),
      question: String(args.question),
      multiSelect: args.multiSelect === true,
      options,
    }],
  };
}
