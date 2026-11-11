import type { EngineInterface, On } from "claude-code";
import { agentDone, bridge, setLastState } from "./bridge";
import { parseNotifications } from "./verificarParse";

const MEASURE_TIMEOUT_MS = 2_000;

// O scanner do engine não segue `$` através de um import: o envio fica aqui, e
// do bridge.ts vem só o endereço.
// Fica no `$.state` da sessão, que sobrevive à recarga do plugin; a variável do bridge.ts recomeça.
const LAST = { plugin: "hangar", key: "lastState" } as const;

async function send($: EngineInterface, estado: string, extra: Record<string, unknown> = {}, measure = false) {
  setLastState(estado);
  // Dentro do `ui.render` (a pergunta desenhada) o engine recusa a escrita: ali fica o estado anterior.
  await $.state.set(LAST, estado).catch((err) => $.ui.log(`hangar: estado ${estado} não guardado para a recarga: ${String(err)}`, { to: "debug" }));
  const p = bridge();
  if (!p) return;
  try {
    if (measure) {
      try {
        // A medida atrasaria o `idle` e o `next(e)`: com prazo, o aviso sai sem ela.
        const usage = await Promise.race([
          $.session.usage(),
          $.clock.sleep(MEASURE_TIMEOUT_MS).then(() => null),
        ]);
        if (usage && usage.context.tokens > 0 && usage.context.window > 0) {
          extra = { ...extra, session_id: await $.session.id(),
            context: { used: usage.context.tokens, window: usage.context.window } };
        }
      } catch {
        // A medida indisponível não pode impedir o aviso de estado nem o próximo hook.
      }
    }
    await $.http.fetch(`${p.url}/state`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ sessao: p.sessao, token: p.token, estado, ...extra }),
    });
  } catch {
    // Backend fora do ar: o aviso se perde e o pane segue decidindo. Lançar aqui impediria o
    // `next(e)` de quem chamou.
  }
}

export type AgentEstado = "running" | "finished" | "failed";

/** `<status>` da task-notification no estado do subagente: só `completed` é fim normal. */
export function agentEnd(status: string): AgentEstado {
  return status === "completed" ? "finished" : "failed";
}

/** O texto de um `content` de linha: string ou blocos `text`. */
export function contentText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((b) => (b && typeof b === "object" && "text" in b && typeof b.text === "string" ? b.text : "")).join("\n");
}

// Subagente em segundo plano termina pela task-notification: o `turn.complete` dele pode ser só o fim de
// um turno, com mensagem na fila para o próximo.
const background = new Set<string>();
// Só um id que o `agent.spawn` devolveu é subagente: o `SendMessage` também fala com nomes, `*` e colegas.
// ponytail: sem teto; o conjunto morre com o processo do claude.
const spawned = new Set<string>();
// Fim já avisado: o `running` do `agent.spawn` sai depois do `next(e)` e não pode cobrir um fim rápido.
const ended = new Set<string>();
// Fila única: os avisos chegam ao backend na ordem em que aconteceram, mesmo com o hook já liberado.
let agentQueue: Promise<void> = Promise.resolve();
const AGENT_POST_MS = 1500;

async function postAgent($: EngineInterface, agentId: string, estado: AgentEstado, at: number) {
  const p = bridge();
  if (!p) return;
  try {
    const r = await $.http.fetch(`${p.url}/agent`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ sessao: p.sessao, token: p.token, agentId, estado, at }),
    });
    if (r.status !== 200) $.ui.log(`hangar: aviso do subagente ${agentId} (${estado}) recusado: ${r.status} ${r.text}`, { to: "debug" });
  } catch (err) {
    // Sem backend o subagente segue; a lista do app fica no último estado que chegou.
    $.ui.log(`hangar: aviso do subagente ${agentId} (${estado}) não chegou: ${String(err)}`, { to: "debug" });
  }
}

/** Avisa sem segurar o hook além do prazo: backend pendurado não atrasa o subagente nem o turno. */
function agentLife($: EngineInterface, agentId: string, estado: AgentEstado): Promise<unknown> {
  if (!agentId || !bridge()) return Promise.resolve();
  if (estado === "running") ended.delete(agentId);
  else ended.add(agentId);
  const at = Date.now();
  // Cada passo também tem prazo: um POST pendurado não prende os avisos seguintes.
  const step = agentQueue.then(() => Promise.race([postAgent($, agentId, estado, at), $.clock.sleep(AGENT_POST_MS)]));
  agentQueue = step.then(() => {});
  return Promise.race([step, $.clock.sleep(AGENT_POST_MS)]);
}

/** Estado por EVENTO, no lugar da leitura do pane.
 *
 * O par é `turn.start`/`turn.complete`, não `prompt.submit`/`classic.Stop`: o
 * turno é o que o rótulo descreve, e a submissão do próprio plugin não passa
 * pelos hooks dele (medido — o `working` sumia justo na mensagem vinda do app).
 *
 * Só avisa a transição; quem decide o rótulo continua sendo o backend, que já
 * tem a máquina de estados e as outras fontes (Codex, Pi, Kimi). */
export function registerState(on: On) {
  // A atualização do Hangar recarrega o plugin no meio do turno, e a recarga roda o `session.start` de novo:
  // o turno que corria segue `working`. O resto começa `idle`: depois do `/clear` o último é `fim`.
  on("session.start", async ($, e, next) => {
    const { value } = await $.state.get(LAST);
    await send($, value === "working" ? "working" : "idle", { cwd: await $.session.cwd(), model: await $.session.model() });
    return next(e);
  });

  on("turn.start", async ($, e, next) => {
    // Turno de subagente (o de outro plugin, os de fundo) roda com a sessão parada: o fim dele já não conta.
    if (!e.agentId) await send($, "working");
    return next(e);
  });

  on("turn.complete", async ($, e, next) => {
    // Fim de subagente não é a sessão parada: o turno principal pode seguir trabalhando.
    if (e.agentId) {
      agentDone(e.agentId, { answer: e.answer, isAborted: e.isAborted });
      if (!background.has(e.agentId)) await agentLife($, e.agentId, e.isAborted ? "failed" : "finished");
      return next(e);
    }
    await send($, "idle", { motivo: e.reason }, true);
    return next(e);
  });

  // Ciclo de vida dos subagentes para a lista do app: nasce aqui, termina no `turn.complete` (primeiro plano)
  // ou na task-notification (segundo plano), e volta a rodar quando um SendMessage o retoma.
  on("agent.spawn", async ($, e, next) => {
    const r = await next(e);
    if ("agentId" in r && r.agentId) {
      spawned.add(r.agentId);
      if (e.background) background.add(r.agentId);
      // O de primeiro plano pode ter acabado antes de o `next` voltar.
      if (!ended.has(r.agentId)) await agentLife($, r.agentId, "running");
    }
    return r;
  });

  // Retomada: só conta a chamada que passou e falou com um subagente conhecido. Ela roda até o
  // `turn.complete` dela; se vier task-notification, também encerra.
  on("tool.call", { tool: "SendMessage" }, async ($, e, next) => {
    const r = await next(e);
    const to = (e as { to?: unknown }).to;
    if (typeof to === "string" && spawned.has(to) && !r.deny && !r.isError) await agentLife($, to, "running");
    return r;
  });

  on("session.append", async ($, e, next) => {
    if (e.origin.kind === "task-notification") {
      for (const { id, status } of parseNotifications(contentText(e.message.content))) {
        background.delete(id);
        await agentLife($, id, agentEnd(status));
      }
    }
    return next(e);
  });

  // A pergunta de múltipla escolha é DESENHADA: é o instante exato em que a
  // sessão passa a esperar o usuário, sem depender de achar o menu no pane.
  on("ui.render", { component: "AskUserQuestion" }, async ($, e, next) => {
    await send($, "awaiting_input", { motivo: "pergunta" });
    return next(e);
  });

  on("classic.Notification", async ($, e, next) => {
    await send($, "awaiting_input", { motivo: e.message });
    return next(e);
  });

  // Só ACORDA o backend: quem declara a sessão morta continua sendo o tmux, porque
  // `kill -9` não dispara este evento e `/clear` dispara sem ninguém ter morrido.
  on("session.end", async ($, e, next) => {
    await send($, "fim", { motivo: e.reason });
    return next(e);
  });
}
