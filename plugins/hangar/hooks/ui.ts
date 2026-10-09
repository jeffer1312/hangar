import type { EngineInterface, On } from "claude-code";
import { type Bridge, bridge, onBtwChange, onOwnPane, onResend, setSurfaceBridge, surfaceBridge } from "./bridge";
import { BTW_PANE, btwSupported, btwView } from "./btw";
import { focusElement, holding, HOLD_MS, type FocusTarget } from "./uiFocus";
import { openerUrl } from "./uiIntercept";
import { bandBody, scrollOffset, shownAfterClose, stampOwn, UNDRAWN, type PaneEntry } from "./uiPayload";

// A faixa redesenha a cada segundo enquanto um mod mostra relógio: o envio junta os quadros.
const SEND_DELAY_MS = 500;
// Um Raster cheio passa de 1 MB; acima disto saem os painéis e depois a faixa.
const MAX_BODY_CHARS = 256 * 1024;

let above: unknown = null;
let columns: number | null = null;
const panes = new Map<string, PaneEntry>();
// O painel cujo `ui.render` chegou por último: no terminal o painel escondido não é redesenhado, então o
// último desenhado é o da frente (T1). O backend confere pela linha de abas da tela.
let shown: string | null = null;
// Painéis que já tiveram desenho: um reaberto com as mesmas props volta sem `ui.render`.
const drawn = new Set<string>();
// Painéis fechados e não reabertos: um desenho que estava em curso no fechamento (o mod redesenha a cada
// segundo, ou o vizinho que veio à frente) termina depois dele e não pode devolver o painel à lista.
const closed = new Set<string>();
// Onde o último painel foi colocado; vale para o painel registrado antes do primeiro desenho.
let lastPlacement: "dock" | "inline" = "dock";
// Até quando o envio do composer fica segurado (reserva por teclado em curso).
let holdUntil: number | null = null;
let sent: string | null = null;
// O que o plugin atende nesta sessão (`bandBody`), lido uma vez.
let caps: string[] | null = null;
// Sem terminal: quem leva o estado do `/btw` ao app, trocado a cada `session.start` (só o último vale).
let surfaceSync: (() => void) | null = null;
let surfaceGen = 0;
const SURFACE_RETRY_MS = 5_000;
const SURFACE_RESYNC_MS = 30_000;
let scheduled = false;
// Reenvio pedido quando a ponte volta, fora de qualquer hook: o engine não deixa guardar o `$`
// numa variável, só usá-lo num closure, como nos timers.
let resend: (() => void) | null = null;

// JSON de objeto sem as chaves de fora, para juntar à ponte no corpo do POST.
const fields = (o: Record<string, unknown>) => JSON.stringify(o).slice(1, -1);

// Mesmo motivo do state.ts: `$` não atravessa import, então o POST é local. `ponte`: a do terminal, ou a
// da superfície `desktop` no clique do app dentro do `claude -p`. Sem ponte, null.
async function post($: EngineInterface, path: string, extra: string, ponte: Bridge | null = bridge()): Promise<{ status: number; text: string } | null> {
  if (!ponte) return null;
  try {
    return await $.http.fetch(`${ponte.url}/${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: `{"sessao":${JSON.stringify(ponte.sessao)},"token":${JSON.stringify(ponte.token)},${extra}}`,
    });
  } catch {
    return null;
  }
}

// Serializa só aqui, no máximo a cada SEND_DELAY_MS: o hook de render só marca que mudou. Sem
// ponte ou com recusa, a faixa sai de novo quando a ponte aparece ou o backend diz que não a tem.
async function flush($: EngineInterface) {
  scheduled = false;
  // Só uma versão lida fica guardada: erro de leitura manda vazio agora e tenta de novo no próximo envio.
  caps ??= await $.session.version().then((v) => (btwSupported(v.base) ? ["btw"] : []), () => null);
  const body = bandBody(above, columns, [...panes.values()], shown, MAX_BODY_CHARS, caps ?? []);
  if (body === sent || !bridge()) return;
  sent = body;
  if ((await post($, "ui", body))?.status !== 200) sent = null;
}

function schedule($: EngineInterface) {
  resend = () => {
    sent = null;
    schedule($);
  };
  if (!scheduled) {
    scheduled = true;
    $.clock.after(SEND_DELAY_MS, () => void flush($));
  }
}

onResend(() => resend?.());
onBtwChange(() => surfaceSync?.());

// Fechar sai da lista do app como no hook de `ui.close`; abrir só libera o espelho, que o desenho preenche.
function forget(id: string): boolean {
  closed.add(id);
  if (!panes.has(id)) return false;
  shown = shownAfterClose([...panes.keys()], shown, id);
  panes.delete(id);
  drawn.delete(id);
  return true;
}

onOwnPane((id, open) => {
  if (open) closed.delete(id);
  else if (forget(id)) resend?.();
});

// Janela do clique que o app pediu: abrir URL e copiar vão para o aparelho de quem clicou. Não acaba
// no fim do `next`: o `onPress` do mod costuma disparar a cópia sem `await`. Só vale para chamadas do
// mod dono do botão, e clique feito no próprio terminal fecha a janela na hora.
const APP_PRESS_MS = 1500;
// `surface`: só no terminal a cópia vai pela ponte; na superfície `desktop` o Hangar já recebe o
// `ui_copy` do engine. `ponte`: a do aparelho do clique (a do terminal ou a da superfície).
type AppPress = { until: number; plugin: string; attempt: string; surface: string; ponte: Bridge };
let appPress: AppPress | null = null;

// O clique do app a que esta chamada pertence, ou null para seguir onde o mod roda.
async function appAttempt($: EngineInterface, origin: string | undefined): Promise<AppPress | null> {
  if (!appPress || origin !== appPress.plugin) return null;
  return (await $.clock.now()) < appPress.until ? appPress : null;
}

// Quem fez a chamada do `$` que está passando por este hook.
function originOf(next: unknown): string | undefined {
  return (next as { origin?: { plugin?: string } }).origin?.plugin;
}

// Clique, cópia e abertura confirmam ao backend o clique que o app pediu. Devolve se o backend
// aceitou: recusado, a cópia ou a abertura acontece onde o mod roda.
async function tell($: EngineInterface, path: "pressed" | "copied" | "opened" | "focused" | "scroll", o: Record<string, unknown>, ponte: Bridge | null = bridge()): Promise<boolean> {
  return (await post($, path, fields(o), ponte))?.status === 200;
}

// O press que começou aqui é o clique que o app pediu? O backend responde com a tentativa, uma vez só. O mod
// vai junto: a `key` só é única dentro dele.
async function fromApp($: EngineInterface, requestId: string, plugin: string, element: string, ponte: Bridge | null = bridge()): Promise<string | null> {
  const r = await post($, "press-start", fields({ requestId, plugin, element }), ponte);
  if (r?.status !== 200) return null;
  const { attempt } = JSON.parse(r.text) as { attempt?: string | null };
  return typeof attempt === "string" && attempt ? attempt : null;
}

// O alvo de foco armado pelo backend; null sem alvo armado; undefined sem resposta que sirva (sem ponte,
// recusa, corpo ilegível, ou a ponte Python da reserva sem Rust, que não tem a rota e responde 404).
async function askFocus($: EngineInterface, requestId: string, plugin: string | null, element: string | null): Promise<FocusTarget | null | undefined> {
  const r = await post($, "focus-target", fields({ requestId, plugin, element }));
  if (r?.status !== 200) return undefined;
  let v: { armed?: boolean; attempt?: unknown; rewrite?: unknown } | null;
  try {
    v = JSON.parse(r.text) as typeof v;
  } catch {
    return undefined;
  }
  if (!v?.armed || typeof v.attempt !== "string") return null;
  return { attempt: v.attempt, rewrite: typeof v.rewrite === "string" ? v.rewrite : null };
}

// Prazo da pergunta do envio segurado: a rota da ponte responde em ~1 s no pior caso (sessão renomeada).
const ARMED_ASK_MS = 2000;

/** O backend ainda tem um alvo de foco armado? `null` sem resposta no prazo. */
async function armedNow($: EngineInterface): Promise<boolean | null> {
  // `requestId` que não é de painel, sem mod nem elemento: a rota só lê, sem reescrita nem registro.
  const ask = askFocus($, "prompt", null, null).then((alvo) => (alvo === undefined ? null : alvo !== null), () => null);
  const late = $.clock.sleep(ARMED_ASK_MS).then(() => null, () => null);
  return Promise.race([ask, late]);
}

/** Espelha no Hangar a faixa acima do prompt, os painéis e os avisos, os de TODOS os mods.
 *
 * `next(e)` devolve a árvore que os plugins abaixo deste e o engine desenharam; ela segue
 * intacta para a tela, e uma cópia vai ao backend. Só a superfície do terminal: com o app da
 * Anthropic aberto pelo Remote Control a mesma faixa também é pedida para `mobile`, e as duas
 * versões se alternariam no Hangar. */
export function registerUi(on: On) {
  // Sem terminal (`claude -p`) o Hangar é a superfície `desktop` dos mods. Aqui só nasce a ponte do
  // clique; a do input.ts fica nula e os outros hooks seguem calados nessa sessão. Com matcher, como o
  // `{ isInteractive: true }` do input.ts: o state.ts tem o único `session.start` sem matcher.
  on("session.start", { isInteractive: false }, async ($, e, next) => {
    const url = await $.env.get("HANGAR_PLUGIN_URL");
    const token = await $.env.get("HANGAR_PLUGIN_TOKEN");
    const sessao = await $.env.get("CP_SESSION_NAME");
    setSurfaceBridge(url && token && sessao ? { url, token, sessao } : null);
    // Sem terminal a árvore chega ao app pela superfície; pela ponte vão só o que o plugin atende e o estado
    // do `/btw`, que o Rust junta à vista dela: agora, a cada mudança do painel e a cada SURFACE_RESYNC_MS,
    // porque um `hangar-server` reiniciado começa sem eles e não pede de novo.
    // Só com a versão confirmada: anunciar sem ela faria o app mandar `/hangar-btw` a um CLI que não o atende.
    if (surfaceBridge() && (await $.session.version().then((v) => btwSupported(v.base), () => false))) {
      const gen = ++surfaceGen;
      let busy = false;
      let again = false;
      // Em fila: um envio velho que chegasse depois do novo deixaria o app em "respondendo".
      const send = async () => {
        if (busy) {
          again = true;
          return;
        }
        busy = true;
        try {
          do {
            again = false;
            const r = await post($, "ui", fields({ caps: ["btw"], panes: [{ id: BTW_PANE, data: btwView() }] }), surfaceBridge());
            if (r?.status !== 200) {
              $.clock.after(SURFACE_RETRY_MS, () => void (gen === surfaceGen && send()));
              break;
            }
          } while (again);
        } finally {
          busy = false;
        }
      };
      surfaceSync = () => void send();
      const tick = () => {
        if (gen !== surfaceGen) return;
        void send();
        $.clock.after(SURFACE_RESYNC_MS, tick);
      };
      tick();
    }
    return next(e);
  });

  on("ui.render", { component: "AbovePrompt" }, async ($, e, next) => {
    const tree = await next(e);
    if (e.surface === "terminal") {
      above = tree;
      // O `bodyColumns` que o mod recebeu; o `bandBody` manda ele (para o evento) e ele mais as 5 colunas
      // do `[-]` (o `columns` de hoje, que o Python lê para cortar a prévia).
      columns = e.props.bodyColumns;
      schedule($);
    }
    return tree;
  });

  on("ui.render", { component: "Pane" }, async ($, e, next) => {
    const tree = await next(e);
    if (e.surface === "terminal" && !closed.has(e.requestId)) {
      lastPlacement = e.props.placement;
      panes.set(e.requestId, {
        id: e.requestId, title: e.props.title, placement: e.props.placement, columns: e.props.bodyColumns,
        tree: e.requestId === BTW_PANE ? stampOwn(tree, "hangar") : tree,
        ...(e.requestId === BTW_PANE ? { data: btwView() } : {}),
      });
      drawn.add(e.requestId);
      shown = e.requestId;
      schedule($);
    }
    return tree;
  });

  on("ui.open", async ($, e, next) => {
    // Antes do `next`: o primeiro desenho do reaberto pode chegar antes de o `ui.open` responder.
    closed.delete(e.id);
    const r = await next(e);
    // Colocado e ainda sem desenho: entra já na lista, no fim, como a aba no terminal. Não colocado (aberto
    // sem pedido abaixo de 144 colunas) não aparece no terminal e não vai ao app (T7). O `ui.open` é uma
    // chamada do `$`: o resultado vem embrulhado em `{ value }` (ou `{ deny }`). Fechado enquanto abria,
    // fica de fora.
    if (r.value?.isPlaced && !panes.has(e.id) && !closed.has(e.id)) {
      panes.set(e.id, { id: e.id, title: e.title ?? e.id, placement: lastPlacement, columns: null, tree: UNDRAWN });
      schedule($);
    }
    // Reaberto com as mesmas props, o painel volta com o desenho guardado, sem `ui.render`.
    if (!drawn.has(e.id)) $.ui.invalidate("ui.render");
    return r;
  });

  on("ui.close", async ($, e, next) => {
    const r = await next(e);
    if ((r as { deny?: unknown } | undefined)?.deny) return r;
    if (forget(e.id)) schedule($);
    return r;
  });

  // O aviso só é desenhado no terminal e não entra no transcript: sem a cópia, quem acompanha a
  // sessão pelo app não o vê. Leva o nome do mod que o emitiu, que é o título da caixa no terminal.
  // No `claude -p` a ponte do terminal é nula e o `post` não sai: o aviso chega pelo `ui_toast`.
  on("ui.toast", async ($, e, next) => {
    void post($, "toast", fields({ text: e.text, timeoutMs: e.timeoutMs, plugin: originOf(next) }));
    return next(e);
  });

  on("ui.press", async ($, e, next) => {
    // A ponte do aparelho do clique: a do terminal, ou a da superfície `desktop` que o Hangar liga no
    // `claude -p`. Outra superfície (o app da Anthropic pelo Remote Control) segue sem janela.
    const ponte = e.surface === "terminal" ? bridge() : e.surface === "desktop" ? surfaceBridge() : null;
    if (e.surface === "terminal" || ponte) {
      const attempt = ponte ? await fromApp($, e.requestId, e.plugin, e.element, ponte) : null;
      appPress = attempt && ponte ? { until: (await $.clock.now()) + APP_PRESS_MS, plugin: e.plugin, attempt, surface: e.surface, ponte } : null;
    }
    try {
      return await next(e);
    } finally {
      if (e.surface === "terminal") void tell($, "pressed", { requestId: e.requestId, plugin: e.plugin, element: e.element });
    }
  });

  on("ui.copy", async ($, e, next) => {
    const clique = await appAttempt($, originOf(next));
    // Na superfície `desktop` a cópia já vai ao Hangar pelo `ui_copy`: desviá-la aqui a mandaria duas vezes.
    if (!clique || clique.surface !== "terminal" || !(await tell($, "copied", { attempt: clique.attempt, text: e.text }, clique.ponte))) return next(e);
    return { value: { isCopied: true } };
  });

  on("process.run", async ($, e, next) => {
    const url = openerUrl(e.argv);
    const clique = url ? await appAttempt($, originOf(next)) : null;
    if (!url || !clique || !(await tell($, "opened", { attempt: clique.attempt, url }, clique.ponte))) return next(e);
    return { value: { exitCode: 0, stdout: "", stderr: "", isStdoutTruncated: false, isStderrTruncated: false } };
  });

  // A rolagem de um painel (roda ou teclas): o backend acompanha o `offset` para alcançar pelo mouse um
  // botão fora da área visível (T4). Só com a ponte do terminal.
  on("ui.scroll", async ($, e, next) => {
    const r = await next(e);
    if (bridge() && !(r as { deny?: unknown } | undefined)?.deny) {
      void tell($, "scroll", { requestId: e.requestId, offset: scrollOffset(e), bodyRows: e.bodyRows, contentRows: e.contentRows });
    }
    return r;
  });

  // Reserva por teclado do clique do app (T5): com um alvo armado, a `key` dele entra no evento e o
  // backend fica sabendo onde o anel pousou; sem alvo, o foco segue como veio.
  on("ui.focus", async ($, e, next) => {
    if (!bridge()) return next(e);
    const alvo = await askFocus($, e.requestId, e.plugin ?? null, e.element ?? null);
    if (!alvo) return next(e);
    holdUntil = (await $.clock.now()) + HOLD_MS;
    const element = focusElement(e, alvo);
    const r = await next(element === e.element ? e : { ...e, element });
    // O mod vai junto: dois mods podem ter a mesma `key` no lugar, e o backend só confirma o do alvo.
    void tell($, "focused", { attempt: alvo.attempt, requestId: e.requestId, plugin: e.plugin ?? null, element: element ?? null, denied: Boolean((r as { deny?: unknown } | undefined)?.deny) });
    return r;
  });

  // Enquanto a reserva por teclado corre, o envio do composer fica segurado: uma letra digitada no meio
  // devolveria o teclado ao prompt e o `Enter` do backend mandaria o rascunho ao modelo ((aa)). Dentro da
  // janela, só segura com o alvo confirmado como armado no backend: ele desarma antes de soltar a fila, e a
  // mensagem que a fila entrega logo depois (o `Enter` do executor também é um envio do composer) passa. Sem
  // resposta, passa também: derrubar faria a mensagem da fila sumir como entregue, e a reserva já recusa com
  // rascunho no prompt, então deixar passar não manda nada indevido.
  on("prompt.submit", { origin: { kind: "composer" } }, async ($, e, next) => {
    if (!holding(await $.clock.now(), holdUntil)) return next(e);
    const armed = await armedNow($);
    if (armed !== true) {
      if (armed === false) holdUntil = null;
      return next(e);
    }
    return { drop: "Hangar: envio segurado durante um clique do app pelo teclado; a seta para cima traz o texto de volta. / Hangar: send held during an app click by keyboard; the Up arrow brings the text back." };
  });
}
