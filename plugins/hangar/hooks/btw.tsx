import type { EngineInterface, ModelForkResult, On } from "claude-code";
import { btwChange, forgetAgent, ownPane, waitAgent } from "./bridge";

// Pergunta lateral: o `/btw` do Claude Code respondido por `$.model.fork` num painel de mod, que o
// `ui.ts` espelha no app. O fork lê a conversa e não grava nada nela; o overlay embutido não abre.
export const BTW_PANE = "hangar-btw";
export const BTW_MAX = 20;
export const BTW_RECENT = 5;
// O fork não aceita sinal: o prazo só desiste de esperar, a chamada segue até o fim e a resposta é descartada.
export const BTW_TIMEOUT_MS = 120_000;
// Subagente com ferramentas pode trabalhar bastante; passado isto, o painel desiste e diz.
export const BTW_FORK_TIMEOUT_MS = 15 * 60_000;

// Build mais antigo medido com `$.model.fork` e o mesmo contrato de resultado.
export const BTW_MIN_VERSION = "2.1.289";

/** O CLI desta sessão atende a pergunta lateral? `base` é o de `$.session.version()` (`2.1.294`, `2.1.294-dev`). */
export function btwSupported(base: string | undefined): boolean {
  const parts = (v: string) => v.split("-")[0]!.split(".").map(Number);
  if (!base) return false;
  const have = parts(base);
  const need = parts(BTW_MIN_VERSION);
  if (have.some(Number.isNaN)) return false;
  for (let i = 0; i < need.length; i++) {
    if ((have[i] ?? 0) !== need[i]) return (have[i] ?? 0) > need[i]!;
  }
  return true;
}

type Status = "pending" | "done" | "error";
type ForkState = "" | "running" | "done" | "failed";
// `fork`: o que a bifurcação disse (iniciada ou por que não), vazio enquanto não houve; `forkState` o mesmo
// para o app, que escreve o texto no idioma dele.
export type BtwEntry = {
  id: number; question: string; status: Status; answer: string; error: string; attempt: number;
  fork: string; forkState: ForkState; askedAt: number;
};

/** O painel como o app nativo o desenha com o tema dele: vai junto da árvore no `/ui`. */
export type BtwView = {
  current: number;
  entries: { id: number; question: string; status: Status; answer: string; error: string; fork: ForkState; forkNote: string; askedAt: number }[];
};

const TEXTS = {
  pt: {
    title: "Pergunta lateral",
    empty: "Nenhuma pergunta ainda. Mande /btw <pergunta> pelo campo de mensagem.",
    answering: "Respondendo…",
    position: (i: number, n: number) => `Pergunta ${i} de ${n}`,
    recent: "Recentes",
    prev: "‹ Anterior", next: "Próxima ›", copy: "Copiar", retry: "Repetir", clear: "Limpar", close: "Fechar",
    nothing: "Ainda não há resposta do Claude nesta sessão para consultar (sessão nova, retomada ou depois de /clear). Mande uma mensagem, espere a resposta e repita.",
    api: (status: number | null, error: string) => `A API recusou a pergunta${status ? ` (HTTP ${status})` : ""}: ${error}.`,
    emptyReply: "O modelo não devolveu texto. Tente de novo.",
    aborted: "A pergunta foi interrompida antes da resposta.",
    timeout: "Sem resposta em 2 minutos; o que chegar depois é descartado. Tente de novo.",
    failed: (why: string) => `A pergunta falhou: ${why}`,
    fork: "Bifurcar",
    forked: "Bifurcada: um subagente com ferramentas está trabalhando nesta pergunta.",
    forkDone: "Bifurcação concluída; a resposta foi para a conversa principal.",
    forkAborted: "A bifurcação foi interrompida antes da resposta.",
    forkNotSent: (why: string) => `A bifurcação terminou, mas a resposta não chegou à conversa principal: ${why}`,
    forkFailed: (why: string) => `Não deu para bifurcar: ${why}`,
    forkTimeout: "O subagente não terminou em 15 minutos; a resposta dele não vai mais para a conversa.",
    forkEmpty: "O subagente terminou sem texto; nada foi para a conversa.",
    unsupported: "A pergunta lateral precisa do Claude Code 2.1.289 ou mais novo.",
    actionFailed: (why: string) => `Não deu certo: ${why}`,
    forkMessage: (q: string, a: string) => `Resultado da pergunta lateral bifurcada (/btw ${q}):\n\n${a}`,
    dropped: "Pergunta lateral respondida no painel do Hangar.",
  },
  en: {
    title: "Side question",
    empty: "No questions yet. Send /btw <question> from the message field.",
    answering: "Answering…",
    position: (i: number, n: number) => `Question ${i} of ${n}`,
    recent: "Recent",
    prev: "‹ Previous", next: "Next ›", copy: "Copy", retry: "Retry", clear: "Clear", close: "Close",
    nothing: "Claude has not replied in this session yet (new, resumed or after /clear). Send a message, wait for the reply and retry.",
    api: (status: number | null, error: string) => `The API refused the question${status ? ` (HTTP ${status})` : ""}: ${error}.`,
    emptyReply: "The model returned no text. Try again.",
    aborted: "The question was interrupted before the answer.",
    timeout: "No answer within 2 minutes; anything that arrives later is discarded. Try again.",
    failed: (why: string) => `The question failed: ${why}`,
    fork: "Fork",
    forked: "Forked: a subagent with tools is working on this question.",
    forkDone: "Fork finished; its answer went to the main conversation.",
    forkAborted: "The fork was interrupted before answering.",
    forkNotSent: (why: string) => `The fork finished, but its answer did not reach the main conversation: ${why}`,
    forkFailed: (why: string) => `Could not fork: ${why}`,
    forkTimeout: "The subagent did not finish within 15 minutes; its answer will not reach the conversation.",
    forkEmpty: "The subagent finished without text; nothing went to the conversation.",
    unsupported: "The side question needs Claude Code 2.1.289 or newer.",
    actionFailed: (why: string) => `It did not work: ${why}`,
    forkMessage: (q: string, a: string) => `Result of the forked side question (/btw ${q}):\n\n${a}`,
    dropped: "Side question answered in the Hangar panel.",
  },
};
type Texts = (typeof TEXTS)["pt"];

let entries: BtwEntry[] = [];
let current = -1;
let seq = 0;
let texts: Texts = TEXTS.pt;
let langRead = false;

// Redesenha o painel e avisa quem leva o estado ao app sem terminal (o `ui.ts`, pela ponte da superfície).
function changed($: EngineInterface): void {
  $.ui.invalidate("ui.render");
  btwChange();
}

/** Pergunta nova no fim; acima de `BTW_MAX` saem as mais antigas. Devolve a lista e o índice da nova. */
export function pushEntry(list: readonly BtwEntry[], entry: BtwEntry): { list: BtwEntry[]; index: number } {
  const next = [...list, entry].slice(-BTW_MAX);
  return { list: next, index: next.length - 1 };
}

/** As `BTW_RECENT` mais novas, da mais nova para a mais antiga, com o índice de cada uma na lista. */
export function recentEntries(list: readonly BtwEntry[]): { entry: BtwEntry; index: number }[] {
  return list.map((entry, index) => ({ entry, index })).slice(-BTW_RECENT).reverse();
}

export function btwView(): BtwView {
  return {
    current,
    entries: entries.map((e) => ({
      id: e.id, question: e.question, status: e.status, answer: e.answer, error: e.error,
      fork: e.forkState, forkNote: e.fork, askedAt: e.askedAt,
    })),
  };
}

/** O texto que o fork recebe: a pergunta, com a moldura que impede o modelo de retomar a tarefa. */
export function framed(question: string): string {
  return [
    "Side question from the user (/btw), asked while the main work goes on. Answer it from this conversation's",
    "context, in Markdown, in the language of the question. Text only: do not call tools and do not resume or",
    "continue the main task.",
    "",
    question,
  ].join("\n");
}

/** O erro mostrado para um fork sem resposta. */
export function failure(r: Exclude<ModelForkResult, { isAnswered: true }> | { reason: "timeout" }, t: Texts = TEXTS.pt): string {
  switch (r.reason) {
    case "nothing-to-fork": return t.nothing;
    case "api-error": return t.api(r.status, String(r.error));
    case "empty-reply": return t.emptyReply;
    case "aborted": return t.aborted;
    case "timeout": return t.timeout;
  }
}

const short = (s: string, max = 48) => (s.length > max ? `${s.slice(0, max - 1)}…` : s);

function ask($: EngineInterface, entry: BtwEntry): void {
  const attempt = ++entry.attempt;
  entry.status = "pending";
  entry.answer = "";
  entry.error = "";
  // Só a tentativa da vez escreve: uma resposta que chega depois do prazo, de um Repetir ou de um Limpar some.
  const settle = (patch: Partial<BtwEntry>) => {
    const live = entries.find((x) => x.id === entry.id);
    if (!live || live.attempt !== attempt || live.status !== "pending") return;
    Object.assign(live, patch);
    changed($);
  };
  // Repetir depois de uma bifurcação que falhou devolve o botão.
  if (entry.forkState === "failed") {
    entry.fork = "";
    entry.forkState = "";
  }
  const late = $.clock.sleep(BTW_TIMEOUT_MS).then(() => ({ reason: "timeout" }) as const);
  // `then` em volta do fork: um erro síncrono dele também vira erro no painel, não pergunta parada.
  void Promise.race([Promise.resolve().then(() => $.model.fork({ prompt: framed(entry.question) })), late]).then(
    (r) => settle("isAnswered" in r && r.isAnswered ? { status: "done", answer: r.text } : { status: "error", error: failure(r as never, texts) }),
    (err: unknown) => settle({ status: "error", error: texts.failed(err instanceof Error ? err.message : String(err)) }),
  );
}

// O "f" do `/btw` do Claude Code: a pergunta vira um subagente `fork` (herda a conversa e as ferramentas),
// que roda em segundo plano e devolve a resposta na conversa principal.
async function fork($: EngineInterface, entry: BtwEntry): Promise<void> {
  if (entry.forkState === "running" || entry.forkState === "done") return;
  const t = texts;
  // Limpar no meio solta a entrada: daí em diante nada mais é mostrado nem vai à conversa.
  const live = () => entries.includes(entry);
  const show = (note: string, state: ForkState) => {
    if (!live()) return;
    entry.fork = note;
    entry.forkState = state;
    changed($);
  };
  // Antes do `spawn`: um segundo clique no meio não cria outro subagente.
  show(t.forked, "running");
  try {
    const r = await $.agent.spawn({ prompt: entry.question, description: short(entry.question, 40), subagentType: "fork" });
    if (r.deny || !r.agentId) return show(t.forkFailed(String(r.deny ?? "sem agentId")), "failed");
    // Agente criado por plugin responde ao plugin: a resposta vai à conversa principal como no "f" do Claude Code.
    const agentId = r.agentId;
    const done = await Promise.race([waitAgent(agentId), $.clock.sleep(BTW_FORK_TIMEOUT_MS).then(() => null)]);
    if (!done) {
      forgetAgent(agentId);
      return show(t.forkTimeout, "failed");
    }
    if (done.isAborted) return show(t.forkAborted, "failed");
    if (!done.answer.trim()) return show(t.forkEmpty, "failed");
    if (!live()) return;
    // Prompt do plugin espera a sessão ficar livre: não corta o turno em curso. O `session.send` não aceita
    // a própria sessão como destino.
    const sent = await $.prompt.submit({ text: t.forkMessage(entry.question, done.answer) });
    show(sent.drop ? t.forkNotSent(sent.drop) : t.forkDone, sent.drop ? "failed" : "done");
  } catch (err) {
    show(t.forkFailed(err instanceof Error ? err.message : String(err)), "failed");
  }
}

/** A pergunta de `/btw` ou `/hangar-btw` (o que o app manda sem terminal), ou null quando não é a lateral. */
export function sideQuestion(text: string): string | null {
  const m = /^\s*\/(?:hangar-)?btw(?:\s+([\s\S]*))?$/.exec(text);
  return m ? (m[1] ?? "").trim() : null;
}

// Erro ao ler a versão não vira "sem suporte": cairia no overlay embutido, que o app não mostra; seguindo, a
// falha do fork, se houver, aparece no painel.
async function supported($: EngineInterface): Promise<boolean> {
  return $.session.version().then((v) => btwSupported(v.base), () => true);
}

// A pergunta nova (ou, vazia, a última de volta) e o painel aberto na hora.
async function openBtw($: EngineInterface, question: string): Promise<void> {
  if (!langRead) {
    const lang = (await $.env.get("LC_ALL")) || (await $.env.get("LC_MESSAGES")) || (await $.env.get("LANG")) || "";
    texts = /^en/i.test(lang) ? TEXTS.en : TEXTS.pt;
    langRead = true;
  }
  if (question) {
    const entry: BtwEntry = { id: ++seq, question, status: "pending", answer: "", error: "", attempt: 0, fork: "", forkState: "", askedAt: await $.clock.now() };
    ({ list: entries, index: current } = pushEntry(entries, entry));
    ask($, entry);
  } else if (entries.length) {
    current = entries.length - 1;
  }
  ownPane(BTW_PANE, true);
  await $.ui.open({ id: BTW_PANE, title: texts.title });
  changed($);
}

export function registerBtw(on: On) {
  on("command.run", { command: "btw" }, async ($, e, next) => {
    // CLI anterior ao fork: segue o `/btw` do próprio Claude Code (o app não manda: o `caps` do `/ui` vem vazio).
    if (!(await supported($))) return next(e);
    await openBtw($, e.args.trim());
    // Sem `text`: nada vai ao transcript, nem a pergunta nem uma linha de saída do comando.
    return {};
  });

  // Sem terminal o Claude Code responde o `/btw` embutido como comando local antes de qualquer hook, e grava
  // no transcript comando de plugin registrado: o app manda `/hangar-btw`, que nenhum comando atende, e o
  // texto é pego aqui e descartado. Só o aviso do descarte fica (linha que o modelo não lê).
  on("prompt.submit", { origin: { kind: "sdk" } }, async ($, e, next) => {
    const question = sideQuestion(e.text);
    if (question === null) return next(e);
    // O `/hangar-btw` nunca segue ao modelo: sem suporte, cai com o motivo.
    if (!(await supported($))) return /^\s*\/hangar-btw/.test(e.text) ? { drop: texts.unsupported } : next(e);
    await openBtw($, question);
    return { drop: texts.dropped };
  });

  on("ui.render", { component: "Pane", requestId: BTW_PANE }, ($, e) => {
    const { Box, Text, Markdown, Button } = $.ui.resolve(e);
    const t = texts;
    const redraw = () => changed($);
    const close = <Button key="fechar" label={t.close} role="dismiss" onPress={() => void $.ui.close({ id: BTW_PANE }).then(() => ownPane(BTW_PANE, false), (err: unknown) => $.ui.toast(t.actionFailed(String(err))))} />;
    const entry = entries[current];
    if (!entry) {
      return (
        <Box flexDirection="column" gap={1}>
          <Text dimColor>{t.empty}</Text>
          <Box>{close}</Box>
        </Box>
      );
    }
    const go = (index: number) => () => {
      current = index;
      redraw();
    };
    const recent = entries.length > 1 ? recentEntries(entries) : [];
    return (
      <Box flexDirection="column" gap={1}>
        <Box flexDirection="row" justifyContent="space-between" columnGap={1}>
          <Text dimColor>{t.position(current + 1, entries.length)}</Text>
          <Box flexDirection="row" columnGap={1}>
            {current > 0 ? <Button key="anterior" label={t.prev} onPress={go(current - 1)} /> : null}
            {current < entries.length - 1 ? <Button key="proxima" label={t.next} onPress={go(current + 1)} /> : null}
          </Box>
        </Box>
        <Text bold>{`› ${entry.question}`}</Text>
        {entry.status === "pending" ? <Text color="cyan">{t.answering}</Text>
          : entry.status === "error" ? <Text color="red">{entry.error}</Text>
          : <Markdown key="resposta" text={entry.answer} />}
        {entry.fork ? <Text dimColor>{entry.fork}</Text> : null}
        <Box flexDirection="row" flexWrap="wrap" columnGap={1}>
          {entry.status === "done" ? <Button key="copiar" label={t.copy} onPress={(press) => void $.ui.copy({ text: entry.answer, surface: press.surface }).catch((err: unknown) => $.ui.toast(t.actionFailed(String(err))))} /> : null}
          {entry.status !== "pending" ? <Button key="repetir" label={t.retry} onPress={() => { ask($, entry); redraw(); }} /> : null}
          {entry.status !== "pending" && (entry.forkState === "" || entry.forkState === "failed") ? <Button key="bifurcar" label={t.fork} onPress={() => void fork($, entry)} /> : null}
          <Button key="limpar" label={t.clear} onPress={() => { entries = []; current = -1; redraw(); }} />
          {close}
        </Box>
        {recent.length ? (
          <Box flexDirection="column">
            <Text dimColor>{t.recent}</Text>
            {recent.map(({ entry: r, index }) => (
              <Button key={`recente-${r.id}`} plain label={`${index === current ? "▸ " : "  "}${short(r.question)}`} onPress={go(index)} />
            ))}
          </Box>
        ) : null}
      </Box>
    );
  });
}
