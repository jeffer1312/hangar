import type { EngineInterface, On } from "claude-code";
import { agentDone, bridge, setLastState } from "./bridge";

const MEASURE_TIMEOUT_MS = 2_000;

// O scanner do engine não segue `$` através de um import: o envio fica aqui, e
// do bridge.ts vem só o endereço.
async function send($: EngineInterface, estado: string, extra: Record<string, unknown> = {}, measure = false) {
  setLastState(estado);
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

/** Estado por EVENTO, no lugar da leitura do pane.
 *
 * O par é `turn.start`/`turn.complete`, não `prompt.submit`/`classic.Stop`: o
 * turno é o que o rótulo descreve, e a submissão do próprio plugin não passa
 * pelos hooks dele (medido — o `working` sumia justo na mensagem vinda do app).
 *
 * Só avisa a transição; quem decide o rótulo continua sendo o backend, que já
 * tem a máquina de estados e as outras fontes (Codex, Pi, Kimi). */
export function registerState(on: On) {
  on("session.start", async ($, e, next) => {
    await send($, "idle", { cwd: await $.session.cwd(), model: await $.session.model() });
    return next(e);
  });

  on("turn.start", async ($, e, next) => {
    await send($, "working");
    return next(e);
  });

  on("turn.complete", async ($, e, next) => {
    // Fim de subagente não é a sessão parada: o turno principal pode seguir trabalhando.
    if (e.agentId) {
      agentDone(e.agentId, { answer: e.answer, isAborted: e.isAborted });
      return next(e);
    }
    await send($, "idle", { motivo: e.reason }, true);
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
