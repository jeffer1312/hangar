// scripts/pi/hangar-state.ts
// Publica o estado da sessao Pi no MESMO marcador que o hook do Claude escreve
// (<config>/.hangar-state/<session_id>.json), entao o HookState do backend le os dois sem
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
const dir = path.join(base, ".hangar-state");
const paneDir = path.join(base, ".hangar-pi");

// O arquivo da sessao vem do ctx do handler, NAO do ambiente. Medido na Task 0 (fato 6): o processo
// do pi nao tem NENHUMA var PI_* no /proc/<pid>/environ e `process.env.PI_SESSION_FILE` e undefined
// dentro da extensao. Quem sabe e `ctx.sessionManager.getSessionFile()`
// (ReadonlySessionManager, dist/core/session-manager.d.ts:140).
function sessionFile(ctx: any): string | null {
  return ctx?.sessionManager?.getSessionFile?.() ?? null;   // --no-session: nada pra rastrear
}

// ── subagente (pi-subagents) publica NADA ──────────────────────────────────────────────────────
// O subagente do pi-subagents e OUTRO PROCESSO, nao outra sessao no mesmo processo: o fork so
// ESCREVE o arquivo da sessao no disco (shared/fork-context.ts, `writeForkedSessionFile`) — nao
// emite evento nenhum no pai —, e o filho nasce com `--session <esse arquivo>` (runs/shared/
// pi-args.ts:519) e o MESMO TMUX_PANE herdado (runs/foreground/execution.ts:463, `spawnEnv =
// {...process.env, ...}`). Com a memoria do modulo zerada, o filho se declarava dono e reescrevia
// o bilhete pane->sessao com o arquivo DO FORK — a pill do app mostrava o modelo do subagente no
// lugar do da sessao (medido 12/08/2026 no pane %2612). O 24f1b75 tentou resolver com a trava
// `sessaoDoPane`, mas ela compara sessoes DENTRO de um processo, e pai e filho nunca dividem
// processo — o fork nao emite session_start no pai (grep por emit no fork-context.ts volta vazio).
//
// O sinal que atravessa o spawn: `PI_SUBAGENT_DEPTH`. O pai nao tem a var (vale 0); o filho
// recebe "1" (o neto "2") do getSubagentDepthEnv() — a unica var que o pi-subagents injeta no env
// do filho (runs/foreground/execution.ts:463 e runs/background/subagent-runner.ts:541). O backend
// do hangar nao seta esta var ao criar sessoes Pi (registry.py spawna `pi --session-id` cru).
const emSubagente = Number(process.env.PI_SUBAGENT_DEPTH ?? "0") > 0;

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
// ~/.claude/.hangar-state (ou disco cheio) quebra o rastreio de TODA sessao Pi pra sempre,
// indistinguivel de "extensao nao instalada" e sem rastro em lugar nenhum. Os erros de fs do Node ja
// carregam o caminho que falhou; o rotulo diz qual escrita era.
function guard(what: string, fn: () => void): void {
  try {
    fn();
  } catch (err) {
    console.error(`[hangar-state] ${what} falhou:`, err);
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
// A CHAVE do bilhete precisa ser unica no servidor do multiplexador. No tmux o `%N` e global e
// serve; no psmux (Windows) ele e por SESSAO — medido em 21/08/2026, quatro sessoes vivas ao mesmo
// tempo e TODAS com `TMUX_PANE=%1`. Com o pane como chave, a segunda sessao Pi sobrescrevia o
// bilhete da primeira e as duas passavam a apontar pro MESMO transcript: uma abria a conversa da
// outra (reproduzido — `pi_session_file` devolveu o mesmo arquivo pros dois panes).
// `PSMUX_SESSION` traz o nome da sessao, que e unico por construcao. No tmux ela nao existe e a
// chave continua sendo o pane, byte-identica a de sempre.
function paneKey(): string | null {
  const psmux = process.env.PSMUX_SESSION;
  if (psmux) return psmux.replace(/[^A-Za-z0-9._-]/g, "-");   // vira nome de arquivo
  const pane = process.env.TMUX_PANE;
  return pane ? pane.replace("%", "") : null;
}

// A MESMA unicidade, pra outra porta: a chave com que esta extensao se apresenta na LINHA (o
// WebSocket do pi_inbox). O bilhete acima ja usava `PSMUX_SESSION`, mas a linha continuava se
// identificando pelo pane — e no psmux o pane e `%1` em toda sessao, entao a segunda sessao Pi
// tomava o slot da primeira e recebia a mensagem endereçada a ela (medido 22/08/2026: um /input
// pra `pi-teste` apareceu na conversa da `pi-medir`, com `delivered: true` na resposta).
//
// SEM sanitizar, ao contrario do paneKey: aqui a chave nao vira nome de arquivo, e o backend
// procura pelo nome da sessao que ELE conhece — passar uma versao "limpa" faria as duas pontas
// discordarem justamente num nome com acento. E no tmux devolve o `TMUX_PANE` INTEIRO (com o `%`),
// que e o que o backend sempre usou: o Linux nao muda em nada.
function chaveDaLinha(): string | null {
  return process.env.PSMUX_SESSION || process.env.TMUX_PANE || null;
}

function publishPane(ctx: any): void {
  guard("publishPane", () => {
    const chave = paneKey();
    const file = sessionFile(ctx);
    if (!chave || !file) return;      // fora do tmux nao ha o que ligar
    writeAtomic(path.join(paneDir, `${chave}.json`),
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

// ── previa AO VIVO do texto do assistente (streaming) ──────────────────────────────────────────
// POR QUE: o backend lia a previa RASPANDO o pane do tmux (preview.py) — texto ja pintado, cortado
// pela largura da janela, misturado com o desenho da TUI. Toda regra de la ("isto e prosa ou e
// cabecalho de ferramenta? spinner? painel de Todos?") e heuristica que quebra quando o Pi muda um
// traco: em 03/08/2026 um quadro do spinner em `*` ASCII fez a previa engolir a linha de status E o
// painel de tarefas inteiro. Aqui o texto sai do proprio evento de streaming — sem adivinhacao.
//
// Publica o ULTIMO bloco de texto (nao a concatenacao): e a unidade que o transcript grava como um
// `assistant_msg` (adapters/pi/transcript.py:97), e e ela que a bolha real substitui quando o bloco
// fecha. Mandando a soma dos blocos, o supressor de "previa ja commitada" (sse.preview_is_committed)
// veria o commitado como PREFIXO da previa e engoliria tudo — previa nenhuma.
//
// O pane continua como plano B do lado do backend: sessao Pi aberta ANTES desta extensao existir
// (ou sem /reload) nao publica nada, e previa nenhuma seria pior que previa raspada.
const previewDir = path.join(base, ".hangar-preview");

// Coalescencia: `message_update` dispara por TOKEN. Escrever a cada um seria um write por caractere
// num arquivo que o backend le a cada 150ms — trabalho jogado fora dos dois lados. Guarda o ultimo
// texto e grava no maximo a cada PREVIEW_MS (o ultimo sempre vence; full-replace, igual ao slot do
// SSE). `unref()` porque um timer pendente NAO pode segurar o processo do Pi vivo na saida.
const PREVIEW_MS = 150;
let previewTimer: ReturnType<typeof setTimeout> | null = null;
let previewPendente: { file: string; text: string } | null = null;
let previewUltimo = "";

function textoEmVoo(message: any): string {
  if (message?.role !== "assistant") return "";
  const blocos = Array.isArray(message?.content) ? message.content : [];
  let ultimo = "";
  for (const b of blocos) {
    if (b?.type === "text" && typeof b.text === "string") ultimo = b.text;
  }
  return ultimo.trim();
}

function gravaPreview(file: string, text: string): void {
  guard("publishPreview", () => {
    writeAtomic(path.join(previewDir, `${path.basename(file, ".jsonl")}.json`),
                { text, ts: Date.now() / 1000 });
  });
}

function publishPreview(ctx: any, text: string, agora: boolean): void {
  const file = sessionFile(ctx);
  if (!file) return;                       // --no-session: nao ha chave pro backend casar
  if (text === previewUltimo && !agora) return;
  previewUltimo = text;
  if (agora) {
    if (previewTimer) { clearTimeout(previewTimer); previewTimer = null; }
    previewPendente = null;
    gravaPreview(file, text);
    return;
  }
  previewPendente = { file, text };
  if (previewTimer) return;                // ja ha uma escrita agendada: o ultimo texto vence nela
  previewTimer = setTimeout(() => {
    previewTimer = null;
    const p = previewPendente;
    previewPendente = null;
    if (p) gravaPreview(p.file, p.text);
  }, PREVIEW_MS);
  previewTimer.unref?.();
}

// ── entrega de mensagem vinda do app, sem passar pela TUI ──────────────────────────────────────
// POR QUE: o backend entregava digitando no tmux e conferindo pela tela. Medido em 02/08/2026, o
// aviso de subagente do Pi dentro da caixa do composer fazia o guarda adiar pra sempre, e a
// mensagem só saía quando o usuário apertava Enter no terminal. Aqui o texto entra por
// `pi.sendUserMessage`, a MESMA fila do Enter do TUI (interactive-mode.js:654 chama o mesmo
// session.prompt), então o usuário continua digitando no terminal normalmente.
//
// A CHAVE é o TMUX_PANE, não o arquivo de sessão: é o que o backend resolve de graça no envio.
// Os dois nomes, novo primeiro: no Windows a migração de nome (backend/app/migracao_sidecars.py)
// não consegue deixar link num ARQUIVO solto sem privilégio, então lá o `.hangar-conn.json`
// pode continuar sendo o único que existe. É resolvido a cada tentativa, não uma vez na carga: o
// arquivo pode nascer (ou ser migrado) depois que a extensão subiu.
const CONN_NOMES = [".hangar-conn.json", ".hangar-conn.json"];
const achaConn = (): string | null =>
  CONN_NOMES.map((n) => path.join(base, n)).find((p) => fs.existsSync(p)) ?? null;

let socket: WebSocket | null = null;
let tentativa = 0;
let desligando = false;

// Dedupe por id de mensagem (achado ALTA da revisao 02/08/2026 — "Porta A"/"Porta B"): o backend
// pode reentregar o MESMO id (retry apos ACK perdido/timeout em pi_inbox.py, ou reenvio pelo
// reconcile de um "sent" nao confirmado no transcript) sem saber se a tentativa ANTERIOR ja chegou
// a chamar sendUserMessage — chamar de novo repetiria a instrucao pro agente, que e o dano real.
// Guarda so os ids JA entregues (chamada bem-sucedida, sem throw sincrono); um id repetido so
// re-confirma. Set preserva ordem de insercao -> FIFO simples pra limitar memoria (o processo do Pi
// vive por horas): 500 cobre com folga qualquer sessao interativa (cada retry e raro — ACK perdido
// ou reconcile, nao trafego normal) e custa <20KB (ids de 32 hex chars).
const idsEntregues = new Set<string>();
const IDS_ENTREGUES_MAX = 500;

function foiEntregue(id: string): boolean {
  return idsEntregues.has(id);
}

function marcarEntregue(id: string): void {
  idsEntregues.add(id);
  if (idsEntregues.size > IDS_ENTREGUES_MAX) {
    const maisAntigo = idsEntregues.values().next().value;
    if (maisAntigo !== undefined) idsEntregues.delete(maisAntigo);
  }
}

// O ctx mais recente. Os handlers de evento recebem (event, ctx) e o socket NAO — mas quem sabe
// responder "tem rascunho na caixa?" e o `ctx.ui`. Guardar o ultimo e o unico jeito de a linha
// alcancar essa API; e legitimo porque a extensao so registra handler no processo do USUARIO (o
// retorno cedo de `emSubagente` cobre o resto) e todo ctx que passa por aqui e da mesma sessao —
// a troca de sessao (/new, /fork, /resume) emite session_start de novo e sobrescreve este valor.
let ctxAtual: any = null;

function lembrarCtx(ctx: any): void {
  if (ctx) ctxAtual = ctx;
}

// A pergunta que o backend faz antes de digitar. Vale a pena existir porque a TELA nao responde:
// o Pi desenha aviso de extensao (`console.error`) DENTRO da faixa do composer, com o mesmo ANSI
// do texto digitado, entao raspar o pane confunde aviso com rascunho — e era isso que recusava a
// troca de modelo pelo app com "composer do pi ja tem texto" (medido 22/08/2026).
//
// `null` = "nao sei" (sem ctx ainda, Pi sem essa API, ou erro): o backend cai na raspagem de
// sempre. String vazia e RESPOSTA — "nao ha rascunho" —, e por isso os dois nao podem se misturar.
function responderPergunta(o_que: string): string | null {
  if (o_que !== "editor") return null;
  try {
    const t = ctxAtual?.ui?.getEditorText?.();
    return typeof t === "string" ? t : null;
  } catch {
    return null;
  }
}

// Log de conectividade da linha (achado ALTA da revisao 02/08/2026): token girado / bind mudado /
// firewall no meio faziam a extensao voltar calada pro caminho de tecla, sem rastro nem no terminal
// do Pi nem no log do backend. Muda de estado só quando MUDA de estado — o retry roda em laço com
// recuo (reagendar abaixo), e logar toda tentativa inundaria o terminal do usuário (mesma politica
// de "aviso uma vez ate mudar" que terminal_input.py:_avisa_deferred usa do lado do backend).
// `null` = ainda não sabemos (antes da 1a tentativa desta sessão do Pi).
let linhaConectada: boolean | null = null;

function avisaConectividade(conectada: boolean, motivo?: string): void {
  if (linhaConectada === conectada) return;
  linhaConectada = conectada;
  if (conectada) {
    console.error("[hangar-state] linha do hangar conectada");
  } else {
    console.error(`[hangar-state] linha do hangar indisponivel${motivo ? ": " + motivo : ""} — ` +
                  "caindo pro envio por tecla ate reconectar");
  }
}

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
    const connFile = achaConn();
    if (!connFile) {
      avisaConectividade(false, "sidecar .hangar-conn.json ainda nao existe");
      reagendar(pi);
      return;   // backend não subiu ou não escreveu ainda
    }
    const { url, token } = JSON.parse(fs.readFileSync(connFile, "utf8"));
    const ws = new WebSocket(`${url}?token=${encodeURIComponent(token)}`);
    socket = ws;

    ws.addEventListener("open", () => {
      tentativa = 0;
      avisaConectividade(true);
      // `pane` continua indo: backend velho so entende ele. `chave` e o que um backend novo usa —
      // ver chaveDaLinha.
      ws.send(JSON.stringify({ pane, chave: chaveDaLinha() }));
    });

    ws.addEventListener("message", (ev: any) => {
      // O parse/ping continua SÍNCRONO dentro do guard (contrato original preservado): se virasse
      // async aqui, um JSON.parse malformado rejeitaria a Promise que o guard não aguarda, e o
      // erro sumiria como unhandled rejection em vez do `[hangar-state] entregar falhou: ...` de
      // sempre. Só a entrega em si (que precisa esperar corroboração) vai pra uma IIFE assíncrona
      // com o PRÓPRIO try/catch — nada escapa sem log.
      guard("entregar", () => {
        const msg = JSON.parse(String(ev.data));
        if (msg.ping) { ws.send(JSON.stringify({ pong: true })); return; }
        // Pergunta (leitura) antes da entrega: campo PROPRIO (`pedir`), e a resposta sai em
        // `resposta` — nunca em `ok` —, entao os dois caminhos nunca se confundem no backend.
        // Responde SEMPRE, inclusive `null`: calar faria o backend pagar o prazo inteiro pra
        // descobrir o que esta linha ja sabe.
        if (typeof msg.pedir === "string") {
          if (msg.id) ws.send(JSON.stringify({ id: msg.id, resposta: responderPergunta(msg.pedir) }));
          return;
        }
        if (!msg.id || typeof msg.text !== "string") return;
        (async () => {
          if (foiEntregue(msg.id)) {
            // Retry do backend pro MESMO id (ACK perdido/timeout, ou reconcile reenviando um
            // "sent" nao confirmado no transcript — ver pi_inbox.py) — ja chamamos sendUserMessage
            // pra este id antes. So confirma; repetir a chamada duplicaria a instrucao no agente.
            ws.send(JSON.stringify({ id: msg.id, ok: true }));
            return;
          }
          try {
            // deliverAs SEMPRE: com a sessão streamando o Pi LEVANTA erro se não vier
            // (agent-session.js:827-840) — recusa em vez de corromper estado, que é o certo.
            const estavaOciosa = !trabalhando;   // snapshot ANTES de chamar — ver bloco acima
            pi.sendUserMessage(msg.text, { deliverAs: msg.deliverAs ?? "steer" });
            // So marca DEPOIS da chamada nao lancar: e o sinal mais forte que a API de hoje da (ver
            // o comentario grande acima sobre sendUserMessage ser void). Um throw sincrono aqui
            // significa que a instrucao NAO foi enfileirada no agente — um retry com o mesmo id
            // precisa poder tentar de novo, entao nao marca `idsEntregues` nesse caminho.
            marcarEntregue(msg.id);
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
      // avisaConectividade dedupa por estado: se "error" (abaixo) ja rodou pra esta queda, esta
      // chamada e um no-op silencioso (mesmo estado) — nao duplica o log.
      avisaConectividade(false, "conexao encerrada");
      // Recuo exponencial com teto: o backend reinicia (systemd) e isso não pode virar tempestade
      // de reconexão. Sem linha, o backend digita no tmux como sempre fez — nada se perde.
      reagendar(pi);
    });

    // Achado ALTA da revisao 02/08/2026: handler vazio ate aqui — token girado, porta mudada ou
    // firewall no meio caiam calados pro retry, sem NENHUM rastro (nem terminal do Pi, nem log do
    // backend). "error" chega ANTES do "close" para uma conexao que falhou, entao loga o motivo
    // aqui (mais informativo: traz o erro real) e o "close" que segue vira no-op pela dedupe acima.
    ws.addEventListener("error", (ev: any) => {
      avisaConectividade(false, String(ev?.error ?? ev?.message ?? ev));
    });
  });
}

export default function (pi: ExtensionAPI) {
  // Subagente: NAO registra nada. Tudo o que esta abaixo publicaria no MESMO pane do pai (TMUX_PANE
  // herdado) com o arquivo DO FORK — o app le o pai e nao sabe que existem dois processos, entao a
  // copia dentro do filho so pode publicar nada: sem estado, sem pane, sem modelo, sem previa, sem
  // socket, sem comandos. O retorno cedo cobre todas as saidas de uma vez.
  if (emSubagente) return;

  // Handlers recebem (event, ctx) — types.d.ts:845. Sem o ctx nao ha arquivo de sessao.
  pi.on("session_start", async (_e: any, ctx: any) => {
    lembrarCtx(ctx);   // ANTES do return de --no-session: a pergunta do editor nao depende de sessao
    // Achado da revisão, confirmado no pacote instalado (agent-session-runtime.js:102-113,
    // `teardownCurrent`): "session_shutdown" NÃO é só "o processo vai morrer" — dispara com
    // reason "resume"/"new"/"fork"/"quit", chamado por switchSession/newSession/fork/
    // importFromJsonl. Essas trocas reusam a MESMA fábrica de extensão (extensions/loader.js:
    // 318-324), então `desligando` sobrevivia entre sessões: a shutdown da sessão VELHA travava
    // `conectar` (e o retry do close) da sessão NOVA pra sempre, sem nenhum log. Resetar aqui, no
    // início de toda sessão nova dentro do MESMO processo, é o caminho mais simples que cobre
    // /new, /fork e /resume sem precisar decidir com o `reason` do evento — só "quit" de verdade
    // não passa por aqui de novo (o processo morre, e o valor não importa mais).
    if (!sessionFile(ctx)) return;   // --no-session: nada pra rastrear
    desligando = false;
    publishPane(ctx); publishModels(pi, ctx); conectar(pi);
  });

  // NOVO: fecha de propósito ao morrer. Sem isto, um /reload deixaria o backend achando que ainda
  // tem alguém lendo até o prazo estourar, e a mensagem daquele intervalo esperaria à toa.
  pi.on("session_shutdown", async (_e: any, ctx: any) => {
    // Sem gate de dono: no processo do usuario (emSubagente falso), toda shutdown e da propria
    // sessao — trocas legitimas passam por teardownCurrent, que emite session_shutdown ANTES do
    // session_start da nova —, e a copia que roda no subagente nao registra handler nenhum.
    // Antes do 24f1b75, o shutdown de QUALQUER sessao ligava `desligando`; a trava sessaoDoPane
    // (removida — ver `emSubagente`) tapava isso comparando sessoes no mesmo processo, o que o
    // fork de subagente nunca faz: pai e filho nao compartilham evento.
    desligando = true;
    // Zera a previa da sessao que SAI. "session_shutdown" dispara em /new, /fork, /resume e /tree —
    // inclusive com o turno rodando —, e nesse caminho nem "message_end" nem "agent_settled" chegam:
    // o sidecar ficaria com o ultimo texto em voo, e retomar essa sessao dentro dos 10min do
    // _PREVIEW_MAX_AGE mostraria no app um texto "sendo digitado" por uma sessao parada.
    publishPreview(ctx, "", true);
    guard("fechar", () => socket?.close());
  });
  pi.on("agent_start", async (_e: any, ctx: any) => {
    lembrarCtx(ctx);
    // O gate de subagente vale tambem pro `trabalhando`/corroboracao: o turno do subagente nao e o
    // turno da sessao, e tratar como se fosse confirmaria entrega de mensagem que ninguem leu —
    // mas quem bloqueia isso e o `emSubagente` la em cima; aqui so o --no-session.
    if (!sessionFile(ctx)) return;
    publishPane(ctx); publishState("working", ctx);
    trabalhando = true;
    eventosAgente.emit("agent_start");   // corrobora entrega pendente — ver bloco da entrega acima
  });
  pi.on("agent_settled", async (_e: any, ctx: any) => {
    lembrarCtx(ctx);
    if (!sessionFile(ctx)) return;
    publishState("idle", ctx); trabalhando = false;
    publishPreview(ctx, "", true);   // turno fechou: nada em voo (rede pro message_end perdido)
  });

  // Previa ao vivo. `message_update` e o unico que traz o texto parcial; `message_end` fecha o bloco
  // (a bolha real vem do transcript, entao a previa TEM que zerar aqui — senao o texto ficaria
  // duplicado por um instante, uma vez na previa e outra na bolha).
  pi.on("message_update", async (e: any, ctx: any) => {
    lembrarCtx(ctx); publishPreview(ctx, textoEmVoo(e?.message), false);
  });
  pi.on("message_end", async (e: any, ctx: any) => {
    if (e?.message?.role !== "assistant") return;
    publishPreview(ctx, "", true);
  });
  // Republica tambem quando a troca vem do TUI (usuario no teclado, Ctrl+P, /model, /settings) —
  // senao o app mostraria o modelo velho ate a proxima sessao.
  pi.on("model_select", async (_e: any, ctx: any) => { publishModels(pi, ctx); });
  pi.on("thinking_level_select", async (_e: any, ctx: any) => { publishModels(pi, ctx); });

  // Argumento separado por ESPACO (`<provider> <id>`) e nao por "/": o id do modelo ja contem
  // barra (ex `cline-pass/glm-5.2` no provedor `clinepass`), entao "provider/id" seria ambiguo.
  // Nem provider nem id tem espaco.
  pi.registerCommand("cp-model", {
    description: "hangar: troca o modelo (<provider> <id>)",
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
    description: "hangar: nivel de raciocinio (off|minimal|low|medium|high|xhigh|max)",
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
