// scripts/pi/cp-state.ts
// Publica o estado da sessao Pi no MESMO marcador que o hook do Claude escreve
// (<config>/.claude-pocket-state/<session_id>.json), entao o HookState do backend le os dois sem
// saber a diferenca. Escrita atomica (tmp + rename) pelo mesmo motivo do state_hook.py: o watcher
// pode ler no meio da escrita e um JSON pela metade viraria marcador ignorado.
//
// `agent_settled` e nao `agent_end`: o `agent_end` dispara antes de o Pi poder auto-retry ou
// auto-compactar e seguir rodando, entao marcar idle nele deixaria a sessao "ociosa" enquanto ela
// ainda trabalha.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { EventEmitter, once } from "node:events";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const base = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), ".claude");
const dir = path.join(base, ".claude-pocket-state");
const paneDir = path.join(base, ".claude-pocket-pi");

// O arquivo da sessao vem do ctx do handler, NAO do ambiente. Medido na Task 0 (fato 6): o processo
// do pi nao tem NENHUMA var PI_* no /proc/<pid>/environ e `process.env.PI_SESSION_FILE` e undefined
// dentro da extensao. Quem sabe e `ctx.sessionManager.getSessionFile()`
// (ReadonlySessionManager, dist/core/session-manager.d.ts:140).
function sessionFile(ctx: any): string | null {
  return ctx?.sessionManager?.getSessionFile?.() ?? null;   // --no-session: nada pra rastrear
}

// A CHAVE do marcador de estado e o stem do arquivo de sessao, nao o session-id: o backend procura
// por `Path(jsonl).stem` (sse.py:305, registry.py:604). No Claude os dois coincidem (<uuid>.jsonl);
// no Pi o arquivo e <ts>_<uuid>.jsonl, entao gravar por session-id cria um marcador que ninguem le
// — e o sintoma seria "sessao Pi sempre ociosa", sem erro em lugar nenhum.
function writeAtomic(target: string, data: unknown): void {
  fs.mkdirSync(path.dirname(target), { recursive: true });
  const tmp = `${target}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(data));
  fs.renameSync(tmp, target);   // atomico: o watcher pode ler no meio da escrita
}

// Nada aqui pode escapar pro loop de eventos do Pi: marcador e otimizacao, nao correcao, e falhar
// nunca pode derrubar o turno do usuario. O corpo INTEIRO entra no try, nao so a escrita:
// `getSessionFile`/`getSessionId` foram lidos de um `ctx: any` (nao ha tipo publico), entao sao o
// candidato numero 1 a sumir ou lancar num upgrade do Pi — e `?.` protege contra o elo ausente, nao
// contra a chamada que lanca. Mas calado tambem nao: sem o console.error, uma permissao errada em
// ~/.claude/.claude-pocket-state (ou disco cheio) quebra o rastreio de TODA sessao Pi pra sempre,
// indistinguivel de "extensao nao instalada" e sem rastro em lugar nenhum. Os erros de fs do Node ja
// carregam o caminho que falhou; o rotulo diz qual escrita era.
function guard(what: string, fn: () => void): void {
  try {
    fn();
  } catch (err) {
    console.error(`[cp-state] ${what} falhou:`, err);
  }
}

function publishState(state: "working" | "idle", ctx: any): void {
  guard(`publishState(${state})`, () => {
    const file = sessionFile(ctx);
    if (!file) return;
    writeAtomic(path.join(dir, `${path.basename(file, ".jsonl")}.json`),
                { state, ts: Date.now() / 1000 });
  });
}

// Bilhete pane -> arquivo de sessao. E a UNICA forma de o backend ligar um pane Pi ao transcript
// dele: o pi reescreve o proprio argv (Task 0, fato 7), entao o `--session-id` some do
// /proc/<pid>/cmdline e nao ha o que casar. `TMUX_PANE` (ex `%123`) o pi HERDA — medido.
function publishPane(ctx: any): void {
  guard("publishPane", () => {
    const pane = process.env.TMUX_PANE;
    const file = sessionFile(ctx);
    if (!pane || !file) return;      // fora do tmux nao ha o que ligar
    writeAtomic(path.join(paneDir, `${pane.replace("%", "")}.json`),
                { file, id: ctx?.sessionManager?.getSessionId?.() ?? null, ts: Date.now() / 1000 });
  });
}

// ── modelo + nivel de raciocinio pelo celular ──────────────────────────────────────────────────
// O `/model` do Pi NAO e dirigivel por send-keys como o do Claude: e uma lista com CAMPO DE BUSCA
// de 301 modelos (`(1/301)` no rodape, so 10 visiveis), entao nem da pra enumerar do pane nem
// navegar contando Down. E o nivel de raciocinio esta enterrado em `/settings` -> "Thinking level"
// (submenu), com o conjunto de niveis variando por modelo. Aqui a extensao pergunta pro proprio Pi:
// o catalogo vai pra um sidecar que o backend le, e a troca entra pela API (`pi.setModel` /
// `pi.setThinkingLevel`) via dois comandos que o app dispara. Zero raspagem de TUI.
const modelDir = path.join(paneDir, "models");

// Ordem canonica (@earendil-works/pi-ai/dist/models.js:391, EXTENDED_THINKING_LEVELS).
const LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const;

// Copia de getSupportedThinkingLevels (pi-ai/dist/models.js:392): quais niveis ESTE modelo aceita.
// Copiado em vez de importado de proposito — a extensao roda por symlink de fora do node_modules
// do pi, entao um `import "@earendil-works/pi-ai/compat"` aqui depende de resolucao que nao
// controlamos; sao 6 linhas e o formato esta congelado no .d.ts publico (ThinkingLevelMap).
// ponytail: calibration knob — se o Pi mudar a regra, e AQUI que ajusta.
function supportedLevels(model: any): string[] {
  if (!model) return [...LEVELS];
  if (!model.reasoning) return ["off"];
  return LEVELS.filter((level) => {
    const mapped = model.thinkingLevelMap?.[level];
    if (mapped === null) return false;                       // explicitamente sem suporte
    if (level === "xhigh" || level === "max") return mapped !== undefined;  // so se mapeado
    return true;
  });
}

// Sidecar do catalogo, chaveado pelo stem do arquivo de sessao (mesma chave do marcador de estado —
// o backend ja resolve nome da sessao -> .jsonl). Nao entra no bilhete do pane porque aquele e
// reescrito a cada agent_start e o catalogo tem ~300 entradas: seria um write de dezenas de KB por
// turno pra um dado que so muda quando o usuario troca de modelo.
function publishModels(pi: ExtensionAPI, ctx: any): void {
  guard("publishModels", () => {
    const file = sessionFile(ctx);
    if (!file) return;
    const model = ctx?.model;
    const all = ctx?.modelRegistry?.getAvailable?.() ?? [];   // so provedores com auth configurada
    writeAtomic(path.join(modelDir, `${path.basename(file, ".jsonl")}.json`), {
      current: model ? { provider: model.provider, id: model.id, name: model.name } : null,
      thinking: pi.getThinkingLevel?.() ?? ctx?.thinkingLevel ?? null,
      levels: supportedLevels(model),
      models: all.map((m: any) => ({
        provider: m.provider, id: m.id, name: m.name, reasoning: !!m.reasoning,
      })),
      ts: Date.now() / 1000,
    });
  });
}

// ── entrega de mensagem vinda do app, sem passar pela TUI ──────────────────────────────────────
// POR QUE: o backend entregava digitando no tmux e conferindo pela tela. Medido em 02/08/2026, o
// aviso de subagente do Pi dentro da caixa do composer fazia o guarda adiar pra sempre, e a
// mensagem só saía quando o usuário apertava Enter no terminal. Aqui o texto entra por
// `pi.sendUserMessage`, a MESMA fila do Enter do TUI (interactive-mode.js:654 chama o mesmo
// session.prompt), então o usuário continua digitando no terminal normalmente.
//
// A CHAVE é o TMUX_PANE, não o arquivo de sessão: é o que o backend resolve de graça no envio.
const connFile = path.join(base, ".claude-pocket-conn.json");

let socket: WebSocket | null = null;
let tentativa = 0;
let desligando = false;

// Corroboração real de entrega (achado da revisão, confirmado no pacote instalado 0.83.0):
// `pi.sendUserMessage()` NUNCA lança nem devolve a Promise pro lado da extensão — a ponte interna
// (agent-session.js:1855-1862) faz `this.sendUserMessage(...).catch(err => runner.emitError(...))`,
// e a lista pública de eventos (extensions/types.d.ts:855-889) não tem "error" nem "queue_update"
// pra extensão ouvir esse emitError. Então um try/catch em volta da chamada nunca pega falha de
// verdade — só protegeria contra um throw síncrono que a API também não dá.
// O único sinal corroborável hoje: se a sessão estava OCIOSA, sendUserMessage() cai no caminho de
// prompt() que dispara "agent_start" antes de rodar o turno (agent-session.js:900-916) — dá pra
// esperar esse evento. Com a sessão OCUPADA (deliverAs="steer" entrando no meio de um turno já
// rodando), o enfileiramento (_queueSteer, agent-session.js:1013) não emite NADA que a extensão
// possa ouvir — pra esse caso não existe corroboração possível com a API de hoje, ponto.
let trabalhando = false;
const eventosAgente = new EventEmitter();

// `once(emitter, evento, {signal})` é a espera com prazo do próprio node:events — sem isso seria
// reinventar timeout+listener+cleanup na mão. Resolve `true` se "agent_start" chegou a tempo,
// `false` no timeout (o `once` já remove o listener sozinho nos dois casos, sem vazar).
async function aguardarAgentStart(prazoMs: number): Promise<boolean> {
  try {
    await once(eventosAgente, "agent_start", { signal: AbortSignal.timeout(prazoMs) });
    return true;
  } catch {
    return false;   // estourou o prazo — sem corroboração (ver "na dúvida, confirma" abaixo)
  }
}

// Mesmo recuo exponencial do `close` abaixo — reaproveitado, nao um segundo mecanismo. Falta do
// sidecar tem a MESMA causa (backend fora do ar / ainda subindo) que a linha cair, entao merece o
// mesmo retry; sem isto, uma sessao Pi aberta ANTES da primeira escrita do arquivo (achado da
// revisao final) nunca teria outra chance de conectar — nem socket, nem "close", nem log.
function reagendar(pi: ExtensionAPI): void {
  tentativa = Math.min(tentativa + 1, 6);
  setTimeout(() => conectar(pi), Math.min(1000 * 2 ** tentativa, 30000));
}

function conectar(pi: ExtensionAPI): void {
  guard("conectar", () => {
    if (desligando || socket) return;
    const pane = process.env.TMUX_PANE;
    if (!pane) return;                       // fora do tmux o backend não tem como nos achar
    if (!fs.existsSync(connFile)) { reagendar(pi); return; }   // backend não subiu ou não escreveu ainda
    const { url, token } = JSON.parse(fs.readFileSync(connFile, "utf8"));
    const ws = new WebSocket(`${url}?token=${encodeURIComponent(token)}`);
    socket = ws;

    ws.addEventListener("open", () => {
      tentativa = 0;
      ws.send(JSON.stringify({ pane }));
    });

    ws.addEventListener("message", (ev: any) => {
      // O parse/ping continua SÍNCRONO dentro do guard (contrato original preservado): se virasse
      // async aqui, um JSON.parse malformado rejeitaria a Promise que o guard não aguarda, e o
      // erro sumiria como unhandled rejection em vez do `[cp-state] entregar falhou: ...` de
      // sempre. Só a entrega em si (que precisa esperar corroboração) vai pra uma IIFE assíncrona
      // com o PRÓPRIO try/catch — nada escapa sem log.
      guard("entregar", () => {
        const msg = JSON.parse(String(ev.data));
        if (msg.ping) { ws.send(JSON.stringify({ pong: true })); return; }
        if (!msg.id || typeof msg.text !== "string") return;
        (async () => {
          try {
            // deliverAs SEMPRE: com a sessão streamando o Pi LEVANTA erro se não vier
            // (agent-session.js:827-840) — recusa em vez de corromper estado, que é o certo.
            const estavaOciosa = !trabalhando;   // snapshot ANTES de chamar — ver bloco acima
            pi.sendUserMessage(msg.text, { deliverAs: msg.deliverAs ?? "steer" });
            // Orçamento curto de propósito: o backend desiste da confirmação em PRAZO_ACK=3s
            // (pi_inbox.py) — esperar demais aqui transforma uma entrega que deu certo em
            // "deferred" à toa. Só espera quando dá pra corroborar (sessão estava ociosa).
            if (estavaOciosa) await aguardarAgentStart(1200);
            // Mesmo sem corroborar (timeout acima, ou sessão ocupada = sem sinal possível),
            // ainda confirma ok:true: falso NEGATIVO é PIOR aqui — o backend reenvia a mensagem
            // pra fila e duplica instrução no agente, que é o dano real (decisão do usuário).
            ws.send(JSON.stringify({ id: msg.id, ok: true }));
          } catch (err) {
            // Confirmação NEGATIVA é informação: o backend deixa a mensagem na fila e re-tenta.
            // Ficar calado faria o backend esperar o prazo inteiro à toa.
            ws.send(JSON.stringify({ id: msg.id, ok: false, erro: String(err) }));
          }
        })();
      });
    });

    ws.addEventListener("close", () => {
      socket = null;
      if (desligando) return;
      // Recuo exponencial com teto: o backend reinicia (systemd) e isso não pode virar tempestade
      // de reconexão. Sem linha, o backend digita no tmux como sempre fez — nada se perde.
      reagendar(pi);
    });

    ws.addEventListener("error", () => { /* o close vem em seguida e cuida do retry */ });
  });
}

export default function (pi: ExtensionAPI) {
  // Handlers recebem (event, ctx) — types.d.ts:845. Sem o ctx nao ha arquivo de sessao.
  pi.on("session_start", async (_e: any, ctx: any) => {
    // Achado da revisão, confirmado no pacote instalado (agent-session-runtime.js:102-113,
    // `teardownCurrent`): "session_shutdown" NÃO é só "o processo vai morrer" — dispara com
    // reason "resume"/"new"/"fork"/"quit", chamado por switchSession/newSession/fork/
    // importFromJsonl. Essas trocas reusam a MESMA fábrica de extensão (extensions/loader.js:
    // 318-324), então `desligando` sobrevivia entre sessões: a shutdown da sessão VELHA travava
    // `conectar` (e o retry do close) da sessão NOVA pra sempre, sem nenhum log. Resetar aqui, no
    // início de toda sessão nova dentro do MESMO processo, é o caminho mais simples que cobre
    // /new, /fork e /resume sem precisar decidir com o `reason` do evento — só "quit" de verdade
    // não passa por aqui de novo (o processo morre, e o valor não importa mais).
    desligando = false;
    publishPane(ctx); publishModels(pi, ctx); conectar(pi);
  });

  // NOVO: fecha de propósito ao morrer. Sem isto, um /reload deixaria o backend achando que ainda
  // tem alguém lendo até o prazo estourar, e a mensagem daquele intervalo esperaria à toa.
  pi.on("session_shutdown", async () => {
    desligando = true;
    guard("fechar", () => socket?.close());
  });
  pi.on("agent_start", async (_e: any, ctx: any) => {
    publishPane(ctx); publishState("working", ctx);
    trabalhando = true;
    eventosAgente.emit("agent_start");   // corrobora entrega pendente — ver bloco da entrega acima
  });
  pi.on("agent_settled", async (_e: any, ctx: any) => { publishState("idle", ctx); trabalhando = false; });
  // Republica tambem quando a troca vem do TUI (usuario no teclado, Ctrl+P, /model, /settings) —
  // senao o app mostraria o modelo velho ate a proxima sessao.
  pi.on("model_select", async (_e: any, ctx: any) => { publishModels(pi, ctx); });
  pi.on("thinking_level_select", async (_e: any, ctx: any) => { publishModels(pi, ctx); });

  // Argumento separado por ESPACO (`<provider> <id>`) e nao por "/": o id do modelo ja contem
  // barra (ex `cline-pass/glm-5.2` no provedor `clinepass`), entao "provider/id" seria ambiguo.
  // Nem provider nem id tem espaco.
  pi.registerCommand("cp-model", {
    description: "claude-cockpit: troca o modelo (<provider> <id>)",
    handler: async (args: string, ctx: any) => {
      const [provider, ...rest] = args.trim().split(/\s+/);
      const id = rest.join(" ");
      const model = provider && id ? ctx?.modelRegistry?.find?.(provider, id) : undefined;
      if (!model) {
        ctx?.ui?.notify?.(`[cp] modelo desconhecido: ${args.trim()}`, "error");
        return;
      }
      // setModel devolve false quando o provedor nao tem chave — falha visivel, nunca calada.
      const ok = await pi.setModel(model);
      ctx?.ui?.notify?.(ok ? `[cp] modelo: ${provider}/${id}` : `[cp] sem chave pra ${provider}`,
                        ok ? "info" : "error");
      publishModels(pi, ctx);   // rede: model_select nao dispara quando o modelo ja era esse
    },
  });

  pi.registerCommand("cp-think", {
    description: "claude-cockpit: nivel de raciocinio (off|minimal|low|medium|high|xhigh|max)",
    handler: async (args: string, ctx: any) => {
      const level = args.trim().toLowerCase();
      if (!(LEVELS as readonly string[]).includes(level)) {
        ctx?.ui?.notify?.(`[cp] nivel desconhecido: ${level}`, "error");
        return;
      }
      // O Pi CLAMPA pro que o modelo suporta (agent-session.js:1277) — pedir xhigh num modelo que
      // so vai ate high aterrissa em high, sem erro. Por isso o app le o sidecar de volta.
      pi.setThinkingLevel(level as any);
      ctx?.ui?.notify?.(`[cp] raciocinio: ${pi.getThinkingLevel?.() ?? level}`, "info");
      publishModels(pi, ctx);
    },
  });
}

// publishPane roda tambem no agent_start de proposito: /tree, /fork e troca de sessao mudam o
// arquivo com a sessao ja rodando, e o session_start sozinho deixaria o bilhete apontando pro
// arquivo velho.
