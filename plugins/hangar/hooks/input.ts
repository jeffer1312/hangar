import type { EngineInterface, On } from "claude-code";
import { type Bridge, askResend, clearBridge, instance, lastState, setBridge } from "./bridge";

// A largada divide o `session.start` com o state.ts por MATCHER — dois hooks no
// mesmo evento sem matcher o engine recusa. O filtro não é enfeite: sem prompt
// box (`-p`, SDK) não há o que entregar.
const REARM_MS = 50;
// Sem o long-poll a espera cairia num `$.clock.sleep`, o único `$` que CONSOME
// o orçamento de 10 s do hook; a chamada HTTP em voo não consome nada.
const BACKOFF_MS = 2000;
const BACKOFF_DONO_MS = 30000;

/** Entrada sem `tmux send-keys` digitando: o backend segura a resposta até ter
 *  texto na fila e diz COMO entregar.
 *
 *  `fill` põe o rascunho no composer e o Hangar manda só o Enter — a mensagem
 *  chega como a fala do usuário. `submit` entrega inteiro por `$.prompt.submit`,
 *  sem tecla nenhuma, ao custo da moldura de "prompt de plugin". `user` é o
 *  `submit` com `asUser`: sem tecla e sem moldura, só com a sessão parada. */
export function registerInput(on: On) {
  on("session.start", { isInteractive: true }, async ($, e, next) => {
    const url = await $.env.get("HANGAR_PLUGIN_URL");
    const token = await $.env.get("HANGAR_PLUGIN_TOKEN");
    // O nome da sessão no Hangar, carimbado no pane pelo `new_session`: é a
    // chave da fila lá, e não o uuid do transcript.
    const sessao = await $.env.get("CP_SESSION_NAME");
    if (url && token && sessao) {
      // Otimista: o dono não pode ficar mudo até o 1º long-poll voltar; o 409 do 2º processo é imediato.
      setBridge({ url, token, sessao });
      $.clock.after(REARM_MS, () => void pull($, { url, token, sessao }));
    } else {
      $.clock.after(REARM_MS, () => void discover($));
    }
    // Recarregado (atualização do plugin), o módulo começa sem a faixa e os painéis, e o engine não
    // redesenha o que já está na tela: o ui.ts só os vê de novo com o desenho pedido.
    $.ui.invalidate("ui.render");
    return next(e);
  });
}

// `{sessao: null}` nem sempre é definitivo: o marcador do state hook ou o vínculo da conversa podem
// chegar depois da largada. Tenta de novo poucas vezes e para.
const DISCOVER_RETRY_MS = [2000, 10000, 30000];

async function discover($: EngineInterface, attempt = 0) {
  try {
    // USERPROFILE antes: no Git Bash do Windows o HOME vem como /c/Users/...
    const home = (await $.env.get("USERPROFILE")) ?? (await $.env.get("HOME"));
    if (!home) return;
    const { url, chave } = JSON.parse(await $.fs.read(`${home}/.hangar/plugin.json`)) as { url: string; chave: string };
    const r = await $.http.fetch(`${url}/whoami`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        chave,
        pane: await $.env.get("TMUX_PANE"),
        nome: await $.env.get("CP_SESSION_NAME"),
        tmux: await $.env.get("TMUX"),
        session_id: await $.session.id(),
      }),
    });
    if (r.status !== 200) return;
    const { sessao, token } = JSON.parse(r.text) as { sessao: string | null; token?: string };
    if (!sessao || !token) {
      const wait = DISCOVER_RETRY_MS[attempt];
      if (sessao === null && wait !== undefined) $.clock.after(wait, () => void discover($, attempt + 1));
      return;
    }
    // Otimista pelo mesmo motivo do `session.start`.
    setBridge({ url, token, sessao });
    void pull($, { url, token, sessao });
  } catch {
    // Sem arquivo, backend fora ou resposta estranha: o plugin fica parado e o tmux segue.
  }
}

/** Devolve o teclado ao prompt, de onde estiver nos mods: um `prompt.fill` com texto tira o foco da faixa
 *  ou do painel sem trocar a aba na frente, e o vazio não mexe nele (medido). O espaço entra no fim, sem
 *  apagar o que a pessoa digite no meio, e só sai se o rascunho ainda for o nosso; o cursor vai ao fim. */
export async function returnFocus(read: () => Promise<{ text: string }>,
  fill: (a: { text: string; mode: "append" | "replace" }) => Promise<{ isFilled: boolean }>): Promise<{ moved: boolean; restored: boolean }> {
  const { text } = await read();
  let moved = false;
  let restored = true;
  try {
    moved = (await fill({ text: " ", mode: "append" })).isFilled;
  } finally {
    if ((await read()).text === `${text} `) {
      restored = (await fill({ text, mode: "replace" }).catch(() => ({ isFilled: false }))).isFilled;
    }
  }
  return { moved, restored };
}

async function pull($: EngineInterface, ponte: Bridge) {
  let espera = REARM_MS;
  try {
    const r = await $.http.fetch(`${ponte.url}/pull`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      // A conversa vai a cada poll: o `/clear` troca o id sem `session.start`, e o backend só
      // entrega à conversa que ele acompanha (um segundo `claude` no mesmo pane recebe 409).
      body: JSON.stringify({
        sessao: ponte.sessao, token: ponte.token, instance: instance(), modos: ["fill", "user", "receipt_v2", "focus"], estado: lastState(),
        session_id: await $.session.id(),
      }),
    });
    // 409 cala os outros hooks; depois dele a ponte só volta com um `/pull` aceito. Erro de rede
    // ou outro status não tira a ponte de quem já é dono, nem a devolve a quem a perdeu.
    if (r.status === 409) clearBridge();
    else if (r.status === 200) setBridge(ponte);
    if (r.status === 200) {
      const { text, modo, faixa, publication_id, generation, session_id } = JSON.parse(r.text) as {
        text?: string | null; modo?: string; faixa?: boolean; publication_id?: string; generation?: number; session_id?: string;
      };
      if (faixa === false) askResend();
      const receipt = { publication_id, generation };
      if (modo === "focus") {
        let ok = false;
        try {
          if (!session_id || await $.session.id() === session_id) {
            const r = await returnFocus(() => $.prompt.read(), (a) => $.prompt.fill(a));
            ok = r.moved;
            if (!r.restored) $.ui.log("hangar: o rascunho ficou com um espaço a mais ao devolver o foco", { to: "debug" });
          }
        } catch (err) {
          $.ui.log(`hangar: devolver o foco falhou: ${String(err)}`, { to: "debug" });
        }
        await $.http.fetch(`${ponte.url}/filled`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ sessao: ponte.sessao, token: ponte.token, ok,
            ...receipt, session_id: await $.session.id() }),
        });
      } else if (text && modo === "fill") {
        let isFilled = false;
        try {
          if (!session_id || await $.session.id() === session_id) {
            ({ isFilled } = await $.prompt.fill({ text, mode: "replace" }));
          }
        } catch (err) {
          // Engine recusou o rascunho: avisa `ok: false` na hora, em vez de deixar o backend
          // esperar o prazo inteiro sem saber se foi a rede ou o composer.
          $.ui.log(`hangar: prompt.fill falhou: ${String(err)}`, { to: "debug" });
        }
        await $.http.fetch(`${ponte.url}/filled`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ sessao: ponte.sessao, token: ponte.token, ok: isFilled,
            ...receipt, session_id: await $.session.id() }),
        });
      } else if (text && modo === "user") {
        let ok = false;
        try {
          if (!session_id || await $.session.id() === session_id) {
            await $.prompt.submit({ text, asUser: true });
            ok = true;
          }
        } catch (err) {
          $.ui.log(`hangar: prompt.submit falhou: ${String(err)}`, { to: "debug" });
        }
        await $.http.fetch(`${ponte.url}/submitted`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ sessao: ponte.sessao, token: ponte.token, ok,
            ...receipt, session_id: await $.session.id() }),
        });
      } else if (text) {
        if (!session_id || await $.session.id() === session_id) await $.prompt.submit({ text });
      }
    } else {
      // 403 é token de outra vida da sessão: insistir de 50 ms bateria no
      // backend para sempre. 409 é outra instância dona da sessão: espera mais.
      espera = r.status === 409 ? BACKOFF_DONO_MS : BACKOFF_MS;
    }
  } catch {
    espera = BACKOFF_MS;
  }
  $.clock.after(espera, () => void pull($, ponte));
}
