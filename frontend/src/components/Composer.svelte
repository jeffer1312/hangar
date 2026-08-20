<script module lang="ts">
  import type { CommandInfo } from '../lib/types';
  import { stateColors, abbrevNum } from '../lib/format';
  // Cache de comandos por sessao: sobrevive a remontagens do Composer (ex: voltar de
  // awaiting_input) pra buscar a lista so uma vez por sessao.
  const commandCache = new Map<string, CommandInfo[]>();

  // Cache de anexos por sessao: sobrevive ao remount do Composer/Chat ao trocar de sessao
  // (App.svelte usa {#key route.sessionName}). So em memoria -> File nao cabe no localStorage
  // (ao contrario do texto do draft). Nao revogamos o objectURL aqui: ele so morre ao enviar
  // ou remover o anexo (removeAttachment/clearAttachments), entao volta valido ao restaurar.
  // ponytail: sem limite/expiracao -> anexar e nunca enviar em varias sessoes acumula memoria;
  // adicionar TTL/limite se isso virar problema real.
  const attachmentCache = new Map<string, { file: File; url: string; isImage: boolean }[]>();
</script>

<script lang="ts">
  import { tick, onDestroy } from 'svelte';
  import * as m from '../paraglide/messages';
  import { novoEstadoVad, passoVad } from '../lib/vad';
  import type { EstadoVad } from '../lib/vad';
  import { lerMaosLivres } from '../lib/maosLivres';
  import { podeEnviarSozinho } from '../lib/autoEnvio';
  import type { MotivoFim } from '../lib/autoEnvio';
  import IconSend from './icons/IconSend.svelte';
  import IconInterrupt from './icons/IconInterrupt.svelte';
  import IconAttach from './icons/IconAttach.svelte';
  import IconMic from './icons/IconMic.svelte';
  import IconMonitor from './icons/IconMonitor.svelte';
  import IconFolder from './icons/IconFolder.svelte';
  import { prepareImage } from '../lib/imagePrep';
  import ContextRing from './ContextRing.svelte';
  import ClaudeModelPopover from './ClaudeModelPopover.svelte';
  import ClaudeEffortPopover from './ClaudeEffortPopover.svelte';
  import ClaudePermissionPopover from './ClaudePermissionPopover.svelte';
  import CodexModelSheet from './CodexModelSheet.svelte';
  import PiModelPopover from './PiModelPopover.svelte';
  import KimiModelPopover from './KimiModelPopover.svelte';
  import KimiEffortPopover from './KimiEffortPopover.svelte';
  import PiEffortPopover from './PiEffortPopover.svelte';
  import SlashSuggest from './SlashSuggest.svelte';
  import CommandSheet from './CommandSheet.svelte';
  import ConfirmSheet from './ConfirmSheet.svelte';
  import DitadoEstiloPopover from './DitadoEstiloPopover.svelte';
  import { ditadoEstilo, estilosDitado } from '../lib/ditadoEstilo.svelte';
  import { getCommands, setModelEffort, uploadFile, transcribeFile, getCodexModels, getPiModels, getPermissionModes, setPermissionMode, type ModelEffortBody } from '../lib/api';
  import type { State, StatsEvent } from '../lib/types';
  import type { StatusFields } from '../lib/statusline';
  import { ttsPlayer } from '../lib/ttsPlayer.svelte';

  interface Props {
    sessionName: string;
    sessionState: State;
    status: StatusFields | null;
    onSend: (text: string) => Promise<void> | void;
    // ctrl-s avulso: promove o que JÁ está na fila da TUI do Kimi pro turno em curso.
    onSteer?: () => Promise<void> | void;
    // Quantas msgs estão esperando o turno atual (as bolhas translúcidas). 0 = sem chip de fila.
    filaCount?: number;
    onCommand: (cmd: string) => void;
    onInterrupt: () => void;
    // Cache de prompt do ultimo turno: { ts, ttl, read }. null = sem dado medido -> sem chip.
    lastCache?: { ts: number; ttl: number; read: number } | null;
    onOpenGit: () => void;
    onOpenPreview: () => void;
    // Grupo de trabalho: chip 🤝 na fileira de cima (1 par = nome; N = "grupo (N)"); tap abre o PairSheet.
    pairPeers?: string[] | null;
    pairedState?: string | null;  // estado vivo do par ÚNICO (working/idle/...) -> bolinha no chip
    onOpenPair?: () => void;
    // Toggle "mandar pro grupo" (só aparece pareada): ligado = prompt vai pra esta sessão E pros membros.
    sendToPair?: boolean;
    onToggleSendToPair?: () => void;
    inputText?: string;  // bindable: o pai injeta um draft (ex: interrupt devolve a msg pendente)
    // Faixa de estatísticas da sessão (evento SSE `stats`). null = sem faixa (ex: Codex).
    stats?: StatsEvent | null;
    // Provider da sessao (Chat.svelte, via allSessions). undefined/"claude" = comportamento de
    // sempre; "codex" esconde o picker de /model e o autocomplete de slash-commands (Claude-only —
    // o Codex nao tem nem um nem outro); "kimi" nao tem sheet de modelo neste MVP (pill so leitura).
    provider?: 'claude' | 'codex' | 'pi' | 'kimi';
  }
  let {
    sessionName, sessionState, status, lastCache = null, onSend, onSteer, onCommand, onInterrupt, onOpenGit,
    onOpenPreview, pairPeers = null, pairedState = null, onOpenPair,
    sendToPair = false, onToggleSendToPair,
    inputText = $bindable(''),
    provider = 'claude',
    filaCount = 0,
    stats = null,
  }: Props = $props();

  // ── Faixa de estatísticas ──────────────────────────────────────────────────
  // "52s" / "18m01s" / "1h02m". Sub-10s ganha 1 decimal (TTFT vive nessa faixa).
  function fmtDur(ms: number): string {
    const s = ms / 1000;
    if (s < 10) return `${s.toFixed(1)}s`;   // ponto decimal: precedente do ActivitySheet:195
    if (s < 60) return `${Math.round(s)}s`;
    const min = Math.floor(s / 60);
    if (min < 60) return `${min}m${String(Math.round(s % 60)).padStart(2, '0')}s`;
    return `${Math.floor(min / 60)}h${String(min % 60).padStart(2, '0')}m`;
  }

  // ── Prazo do cache de prompt ───────────────────────────────────────────────
  // O cache do Claude expira num prazo fixo a partir do ultimo turno (usar renova). Saber se ele
  // ainda vale muda a decisao de mandar agora ou nao, entao o prazo mora no composer, onde a
  // decisao acontece. O TTL vem MEDIDO do transcript — sem ele, nao mostramos nada.
  let agora = $state(Date.now());
  $effect(() => {
    // 20s: o chip mostra minutos, entao nao precisa de mais resolucao que isso.
    const id = setInterval(() => (agora = Date.now()), 20_000);
    return () => clearInterval(id);
  });
  // `ts` e do servidor e `agora` e do aparelho: com o relogio do celular adiantado/atrasado a conta
  // desanda. Nao da pra corrigir sem uma referencia de hora do servidor, mas da pra impedir o
  // absurdo — o que resta nunca pode ser MAIOR que a propria janela.
  const cacheLeftS = $derived(
    lastCache
      ? Math.min(lastCache.ttl, Math.round(lastCache.ts + lastCache.ttl - agora / 1000))
      : 0,
  );
  const cacheAtivo = $derived(!!lastCache && cacheLeftS > 0);
  // Ultimo quinto do prazo. Fixo em 300s, a janela CURTA (5min) nascia ja em ambar e nunca
  // mostrava o estado tranquilo.
  const cacheAcabando = $derived(
    cacheAtivo && !!lastCache && cacheLeftS <= Math.max(60, lastCache.ttl * 0.2),
  );
  const cacheLabel = $derived.by(() => {
    if (!cacheAtivo) return m.composer_expirou();
    const min = Math.ceil(cacheLeftS / 60);
    return min >= 60 ? `${Math.floor(min / 60)}h${String(min % 60).padStart(2, '0')}` : `${min}min`;
  });

  const isCodex = $derived(provider === 'codex');
  const isPi = $derived(provider === 'pi');
  const isKimi = $derived(provider === 'kimi');

  // ── Slash commands: busca uma vez por sessao (com cache) ────────────────────
  // Comeca vazio; o $effect popula na hora a partir do cache (sincrono) ou da rede.
  let commands = $state<CommandInfo[]>([]);
  let commandSheetOpen = $state(false);
  let confirmStopOpen = $state(false);

  // Estilo do ditado: o valor mora no servidor (runtime_config.ditado_estilo) pra valer também
  // pro Ctrl+Espaço e pro celular. Carrega uma vez por store compartilhado — o Composer monta em
  // mais de uma tela (chat e peek do quadro) e cada uma pediria a config por conta própria.
  let estiloAberto = $state(false);
  let estiloPillEl = $state<HTMLElement | null>(null);
  const rotuloEstilo = $derived(
    estilosDitado().find((e) => e.valor === ditadoEstilo.valor)?.rotulo ?? m.composer_ditado_pill(),
  );
  $effect(() => { void ditadoEstilo.carregar(); });

  $effect(() => {
    if (isCodex) return;   // Codex nao tem slash-commands do Claude Code -> nem busca a lista
    const sn = sessionName;
    const cached = commandCache.get(sn);
    if (cached) {
      commands = cached;
      return;
    }
    getCommands(sn)
      .then((c) => {
        commandCache.set(sn, c);
        commands = c;
      })
      .catch(() => {
        // endpoint indisponivel -> segue com lista vazia, sem quebrar a UI
      });
  });

  let textareaEl: HTMLTextAreaElement | undefined = $state();

  // Exposto pro pai (atalho de teclado desktop "/" foca o campo).
  export function focus() { textareaEl?.focus(); }

  // ── Anexos: lista de arquivos + preview local + estado de upload ────────────
  // isImage -> preview; resto -> chip de arquivo. Audio NAO vira anexo: e transcrito e cai no textarea.
  // Restaura do cache em memoria da sessao atual (mesma ideia do draft de texto em Chat.svelte).
  // svelte-ignore state_referenced_locally
  let attachments = $state<{ file: File; url: string; isImage: boolean }[]>(attachmentCache.get(sessionName) ?? []);
  // Mantem o cache em sincronia a cada mudanca (add/remove/clear); some da sessao ao esvaziar.
  $effect(() => {
    if (attachments.length) attachmentCache.set(sessionName, attachments);
    else attachmentCache.delete(sessionName);
  });
  let fileInput: HTMLInputElement | undefined = $state();
  let uploading = $state(false);
  let attachError = $state('');
  let sending = $state(false);
  let sendError = $state('');
  let transcribing = $state(false);   // audio gravado/anexado sendo transcrito pro composer

  // ── Gravacao de audio pelo microfone (MediaRecorder) ────────────────────────
  let recording = $state(false);
  let recError = $state('');
  // Desfazer a limpeza do ditado: guarda o texto cru (raw) devolvido pelo backend junto do texto
  // ja limpo. `before` e o inputText anterior a esta transcricao, pra remontar a mesma concatenacao
  // com o cru no lugar do limpo. Some ao enviar, ao editar o campo, ou em ~10s (undoTimer) -- senao
  // fica pendurado e reaparece no ditado seguinte.
  let undo = $state<{ before: string; raw: string } | null>(null);
  let undoTimer: ReturnType<typeof setTimeout> | undefined;
  let mediaRecorder: MediaRecorder | undefined;
  let recChunks: Blob[] = [];
  let recStream: MediaStream | undefined;
  let recFailed = false;   // marcado no onerror -> onstop nao anexa audio truncado
  let starting = $state(false);    // guarda reentrancia entre o tap e o await getUserMedia resolver —
                                   // $state porque o hint "preparando microfone…" aparece nesse intervalo
  // Geracao da tentativa de gravacao: cancelar a espera (2o tap) ou uma tentativa nova incrementa —
  // uma promise de getUserMedia que resolve tarde compara a geracao dela e se descarta (ver toggleRecord).
  let recGeracao = 0;
  let destroyed = false;         // onDestroy: um getUserMedia em voo nao pode ligar o mic num componente morto
  // Feedback da gravacao: timer (segundos) + waveform (nivel de voz por barra, deslizante).
  let recSeconds = $state(0);
  let recBars = $state<number[]>([]);
  let audioCtx: AudioContext | undefined;
  let rafId = 0;
  let recTimer: ReturnType<typeof setInterval> | undefined;
  // Maos-livres: lido ao INICIAR a gravacao, nao reativo — mudar a chave no meio de um ditado nao
  // pode trocar o comportamento daquela gravacao.
  let maosLivres = false;
  let vadEstado: EstadoVad | undefined;
  let recIniciadoEm = 0;
  let motivoDoFim: MotivoFim | null = null;
  let wakeLock: WakeLockSentinel | undefined;
  // Acumulador da janela de 55ms (ver Step 3).
  let somaQuad = 0;
  let nQuad = 0;

  /** Teto de seguranca: 3 min por RELOGIO DE PAREDE. Nao usar recSeconds — ele vem de setInterval,
   *  que o navegador limita em aba escondida, exatamente o caso que este teto existe pra cobrir. */
  const TETO_MS = 3 * 60_000;

  function pararPorMotivo(motivo: MotivoFim) {
    if (!recording || motivoDoFim) return;   // primeiro motivo vence; nao encerra duas vezes
    motivoDoFim = motivo;
    if (mediaRecorder?.state === 'recording') mediaRecorder.stop();
  }

  let waveW = $state(0);   // largura medida da faixa da waveform (bind:clientWidth)
  // densidade adaptativa: ~1 barra a cada 5px (barra ~3px + gap ~2px) -> preenche denso de mobile a
  // desktop, sem numero fixo que fica esparso na tela larga ou apertado na estreita.
  const barCount = $derived(Math.min(320, Math.max(24, Math.floor((waveW || 320) / 5))));
  const recTimeLabel = $derived(
    `${Math.floor(recSeconds / 60)}:${String(recSeconds % 60).padStart(2, '0')}`,
  );

  // hasInput: tem texto OU anexo. Usado tb pro botao stop/send (ver control-right).
  const hasInput = $derived(inputText.trim().length > 0 || attachments.length > 0);
  const canSend = $derived(hasInput && !uploading && !sending && !recording && !transcribing);
  const isWorking = $derived(sessionState === 'working');

  // ── Contagem regressiva do maos-livres: 3s antes do envio automatico ────────
  // Segundos restantes, ou null = sem contagem. $state porque aparece no template (mesmo padrao
  // de recError/recSeconds).
  let contagem = $state<number | null>(null);
  let contagemTimer: ReturnType<typeof setInterval> | undefined;
  // Prazo por RELOGIO DE PAREDE (mesma decisao do TETO_MS acima, agora aplicada tambem aqui):
  // guarda o INSTANTE-ALVO do envio, nao um contador de setInterval. Um setInterval cru derrete
  // em aba escondida (Chrome Android cai pra ~1 tick/min) -- os "3s pra cancelar" virariam ~3min
  // com o envio acontecendo de celular no bolso.
  const CONTAGEM_MS = 3_000;
  let contagemAlvo = 0;

  function cancelarContagem() {
    if (contagemTimer) { clearInterval(contagemTimer); contagemTimer = undefined; }
    contagem = null;
    contagemAlvo = 0;
  }

  function iniciarContagem() {
    cancelarContagem();
    // Tela ja escondida ANTES da contagem comecar (a transcricao ficou em voo 1-4s e a tela apagou
    // nesse meio-tempo -- aoEsconder rodou com contagem ainda null, sem nada pra cancelar). Sem este
    // guard o alvo de relogio de parede ja nasce vencido e o primeiro tick (<=250ms) manda na hora,
    // sem a janela de cancelar que a correcao 2 existe pra garantir. Mesma recusa sonora dos outros
    // motivos de supressao.
    if (document.hidden) {
      somRecusa();
      setTimeout(fecharBipes, 400);
      return;
    }
    contagemAlvo = Date.now() + CONTAGEM_MS;
    contagem = 3;
    // Tick so pra REDESENHAR o numero -- quem decide a hora de enviar e o relogio (contagemAlvo),
    // nao a contagem de ticks. 250ms: exibicao fluida sem depender de precisao do interval.
    contagemTimer = setInterval(() => {
      const restanteMs = contagemAlvo - Date.now();
      if (restanteMs <= 0) {
        cancelarContagem();
        enviarAutomatico();
        return;
      }
      contagem = Math.ceil(restanteMs / 1000);
    }, 250);
  }

  async function enviarAutomatico() {
    // `submit()` volta CALADO quando canSend e falso (anexo subindo, gravacao reaberta, transcricao
    // em voo). Quem esta na mesa ve o texto parado; quem dirige, nao. Regra do projeto: falha
    // aparece, nao some.
    if (!canSend) {
      recError = m.composer_nao_enviou_sozinho();
      somRecusa();
      setTimeout(fecharBipes, 400);
      return;
    }
    recError = '';        // nao deixar aviso velho embaixo de um envio que deu certo
    // O bipe so toca DEPOIS do desfecho: som otimista mentia quando o /input falhava (o erro pintava
    // na tela, que e exatamente o que quem dirige nao esta olhando -- o bipe e o unico canal dela).
    const ok = await submit();
    if (ok) somEnvio(); else somRecusa();
    setTimeout(fecharBipes, 400);
  }

  // Cancelam: toque em qualquer lugar e tecla. FOCO NAO CANCELA — existe um $effect que da focus()
  // no textarea quando a sessao fica idle, e se foco contasse a contagem se cancelaria sozinha.
  // Tocar no proprio microfone tambem cancela: o pointerdown no documento roda ANTES do click que
  // dispara o toggleRecord, entao a contagem morre e a gravacao nova comeca. Nenhuma linha extra.
  function aoInteragir() {
    if (contagem !== null) cancelarContagem();
  }

  $effect(() => {
    document.addEventListener('pointerdown', aoInteragir);
    document.addEventListener('keydown', aoInteragir);
    return () => {
      document.removeEventListener('pointerdown', aoInteragir);
      document.removeEventListener('keydown', aoInteragir);
    };
  });

  // Atalho do mic (so desktop): Ctrl+Espaco alterna gravar/parar — o mesmo toggleRecord do botao.
  // O Chrome NAO reserva Ctrl+Space (diferente de Ctrl+T/W/D, que nem chegam na pagina); quem pode
  // roubar e o OS: IBus/fcitx no Linux e a troca de input source do macOS usam Ctrl+Space por
  // padrao, entao quem digita CJK pode nunca ver este atalho. e.repeat ignorado: segurar a tecla
  // ligaria/desligaria em rajada.
  // stopImmediatePropagation: com 2 Composers montados (split view), os DOIS recebiam o keydown e
  // iniciavam gravacao (recording e $state por componente, sem guard global) — dois getUserMedia
  // por um keystroke. O primeiro Composer montado vence; deterministico.
  // Sem dialog aberto (mesmo guard do DesktopShell — o overlay do board usa role="region", nem
  // casa com o seletor; o :not() e cinto-e-suspensorios) pra nao gravar "por tras" de uma tela de
  // config. O keydown tambem cai no aoInteragir acima, entao apertar durante a contagem do envio
  // automatico cancela a contagem e comeca a gravar — exatamente o que o toque no 🎤 ja faz.
  function aoAtalhoMic(e: KeyboardEvent) {
    if (e.repeat || e.shiftKey || e.altKey || e.metaKey || !e.ctrlKey || e.code !== 'Space') return;
    if (!window.matchMedia('(min-width: 820px)').matches) return;
    if (document.querySelector('[role="dialog"]:not(.board-overlay)')) return;
    e.preventDefault();   // senao o Ctrl+Space solta um espaco no textarea
    e.stopImmediatePropagation();
    void toggleRecord();
  }

  $effect(() => {
    document.addEventListener('keydown', aoAtalhoMic);
    return () => document.removeEventListener('keydown', aoAtalhoMic);
  });

  // ── Pills de modelo e de esforco: cada uma abre seu popover (aplica via endpoint dedicado) ──
  // Display otimista: a escolha aparece na hora; o status (read-back real do statusline)
  // reconcilia o modelo quando confirma. Esforco e write-only (sem read-back confiavel)
  // -> a escolha local persiste.
  // Popovers do Claude ancorados nas pills (antes: uma folha de altura cheia, com modelo e esforco
  // juntos). Mesma forma do Pi e a mesma referencia do usuario (opencode).
  let claudePopOpen = $state(false);
  let claudeEffortOpen = $state(false);
  let claudePillEl = $state<HTMLElement | null>(null);
  let claudeEffortPillEl = $state<HTMLElement | null>(null);
  let chosenModel = $state<string | null>(null);   // rotulo otimista: 'Opus' | 'Sonnet' | ...
  let chosenEffort = $state<string | null>(null);   // low | medium | high | xhigh | max | ultracode
  // A caixa do Claude fecha ANTES do backend responder (pra nao tapar o picker no terminal), entao
  // a falha que chega depois nao tem onde aparecer la dentro: sai aqui, na linha de erro do envio.
  let modelError = $state<string | null>(null);
  $effect(() => { if (claudePopOpen) modelError = null; });

  // ── Pill de modelo do Codex (Task C): equivalente do pill acima, so quando isCodex ──
  // Fonte de verdade do rotulo do pill e o GET /models (sidecar/dict do backend), nao a
  // statusline -- mais preciso (o backend guarda o valor exato escolhido). O anel de contexto
  // (Task D) ja vem da statusline, igual ao Claude, ja que o CodexAdapter agora acumula
  // tokenUsage/rateLimits e emite o status_line no mesmo formato. Seedado ao montar/trocar de
  // sessao; o sheet atualiza via onApplied apos um POST /model com sucesso.
  let codexSheetOpen = $state(false);
  let codexModel = $state<string | null>(null);
  let codexEffort = $state<string | null>(null);

  $effect(() => {
    if (!isCodex) return;
    const sn = sessionName;
    getCodexModels(sn)
      .then((res) => {
        codexModel = res.current.model;
        codexEffort = res.current.effort;
      })
      .catch(() => {
        // endpoint indisponivel -> pill fica com o rotulo generico, sem quebrar a UI
      });
  });

  function handleCodexModelApplied(model: string, effort: string | null) {
    codexModel = model;
    codexEffort = effort;
  }

  // ── Pill de modelo do Pi: mesmo desenho do de Codex, terceira fonte ──────────────────────────
  // O `/model` do Pi e uma lista com busca de ~300 modelos e o nivel de raciocinio mora dentro do
  // /settings — nada disso e dirigivel por send-keys como o picker do Claude. Quem responde e o
  // sidecar da extensao (GET /pi/models); ver backend/app/pi_models.py. Falha (extensao velha ->
  // 409) deixa o pill com o rotulo da statusline, sem quebrar o composer.
  // Popovers ancorados nas pills (antes: uma folha de altura cheia com modelo e nivel juntos). A
  // referencia pedida pelo usuario e o opencode: caixa compacta sobre a pill, clique aplica, e o
  // nivel tem pill propria. Os `El` sao as ancoras que o Popover mede.
  let piPopOpen = $state(false);
  let piEffortOpen = $state(false);
  let piPillEl = $state<HTMLElement | null>(null);
  let piEffortPillEl = $state<HTMLElement | null>(null);
  let piModel = $state<string | null>(null);
  let piEffort = $state<string | null>(null);
  // Kimi: mesma forma do Pi (pill que abre popover). O nível de pensamento tem pill própria —
  // a linha "Thinking (←→)" do picker aplica só na sessão (Alt+S), sem tocar o config global.
  // `kimiModel`/`kimiEffort` são os otimistas locais até a statusline confirmar (reconciliação
  // logo abaixo, mesma regra do chosenModel do Claude).
  let kimiPopOpen = $state(false);
  let kimiPillEl = $state<HTMLElement | null>(null);
  let kimiModel = $state<string | null>(null);
  let kimiEffortOpen = $state(false);
  let kimiEffortPillEl = $state<HTMLElement | null>(null);
  let kimiEffort = $state<string | null>(null);

  // Reconciliação dos otimistas do Kimi: a statusline confirmou -> solta o override pra que uma
  // troca feita DIRETO no terminal reapareça (mesma regra do chosenModel do Claude).
  $effect(() => {
    if (kimiModel && status?.model && status.model === kimiModel) kimiModel = null;
  });
  $effect(() => {
    if (kimiEffort && status?.effort && status.effort === kimiEffort) kimiEffort = null;
  });

  $effect(() => {
    if (!isPi) return;
    const sn = sessionName;
    getPiModels(sn)
      .then((res) => {
        piModel = res.current?.name ?? res.current?.id ?? null;
        piEffort = res.thinking;
      })
      .catch(() => {});
  });

  function handlePiModelApplied(model: string, effort: string | null) {
    piModel = model || piModel;
    piEffort = effort;
  }

  // ── Pílula de permissão (Task 5): terceira pill só em sessão Claude ─────────
  // Lista viva via GET /permission-modes (ciclo 4 ou 5 lido ao vivo), troca via POST
  // com BTab. Modos fora do ciclo (dontAsk isolado, bypass sem ter nascido nele)
  // aparecem desabilitados com motivo "só na criação".
  let permPopOpen = $state(false);
  let permPillEl = $state<HTMLElement | null>(null);
  let permCurrent = $state<string | null>(null);
  let permModes = $state<string[]>([]);
  let permError = $state<string | null>(null);
  let permSondavel = $state(true);
  let permCarregando = $state(false);
  const isClaude = $derived(!isCodex && !isPi && !isKimi);
  $effect(() => { if (permPopOpen) permError = null; });
  $effect(() => {
    if (!isClaude) return;
    const sn = sessionName;
    // só lê o atual (zero teclas); ciclo só ao abrir a pílula (bloqueador 1)
    getPermissionModes(sn, false)
      .then((res) => {
        permCurrent = res.current;
        permModes = res.modes;
        permSondavel = res.sondavel;
      })
      .catch(() => {
        permCurrent = null;
        permModes = [];
      });
  });
  async function abrirPermissao() {
    permPopOpen = true;
    if (!permSondavel || permModes.length > 0 || permCarregando) return;
    permCarregando = true;
    try {
      const res = await getPermissionModes(sessionName, true);
      permCurrent = res.current;
      permModes = res.modes;
      permSondavel = res.sondavel;
    } catch (e) {
      permError = e instanceof Error ? e.message : String(e);
    }
    permCarregando = false;
  }
  // Reconciliação otimista: se a statusline um dia trouxer permissão, solta; hoje não traz,
  // mas mantém o padrão do thinking do Pi (devolver o que FICOU).
  async function handlePermApply(modo: string): Promise<void> {
    permError = null;
    try {
      const res = await setPermissionMode(sessionName, modo);
      // backend devolve o que FICOU (pode ser diferente do pedido se houve clamp/teto)
      const ficou = res.mode ?? res.current ?? modo;
      permCurrent = ficou;
    } catch (e) {
      // 409 com modo que ficou: backend devolveu {mode: ficou} mas jogou 409.
      // Tenta extrair o modo que ficou da mensagem, senão mantém o atual.
      const msg = e instanceof Error ? e.message : String(e);
      permError = msg;
      // tenta re-ler o atual para refletir o que ficou de verdade
      try {
        const cur = await getPermissionModes(sessionName);
        permCurrent = cur.current;
        permModes = cur.modes;
      } catch (e2) {
        // mantém permError do POST; releitura falhou mas 409 já está na tela
        if (!permError) permError = e2 instanceof Error ? e2.message : String(e2);
      }
      throw e;
    }
  }

  const pillModel = $derived(chosenModel ?? status?.model ?? null);
  const pillEffort = $derived(chosenEffort ?? status?.effort ?? null);
  // Haiku nao usa esforco de raciocinio (o picker responde "Effort not supported").
  const semEsforcoClaude = $derived((pillModel ?? '').toLowerCase().includes('haiku'));
  // Reconciliacao do modelo: quando o statusline confirma a escolha (substring match),
  // solta a escolha otimista pra que mudancas feitas direto no terminal reaparecam.
  $effect(() => {
    const modelo = status?.model?.toLowerCase();
    if (chosenModel && modelo && modelo.includes(chosenModel.toLowerCase())) {
      chosenModel = null;
    }
  });

  // Aplica modelo+esforco via o endpoint dedicado, que dirige o picker interativo do /model
  // (scope 'session' = so esta sessao, sem virar o default). Devolve a Promise pro sheet
  // aguardar/tratar erro. O display otimista so muda APOS sucesso (uma aplicacao que falha
  // nao deixa o pill mostrando uma escolha que nao pegou). 'default' resolve pra um modelo
  // concreto -> deixa o statusline ditar o rotulo; os demais aparecem capitalizados.
  function handleApply(body: ModelEffortBody): Promise<void> {
    return setModelEffort(sessionName, body).then((res) => {
      // `pending_confirm` = o Claude abriu "Change effort level?" no terminal e ESPERA o usuario.
      // Adiantar o pill aqui mostraria um nivel que so vale se ele tocar "Yes" — e se tocar "No",
      // o pill ficaria mentindo ate a proxima statusline. Deixa a conversa contar o desfecho.
      if (res?.pending_confirm) return;
      if (body.model) {
        chosenModel = body.model === 'default'
          ? null
          : body.model.charAt(0).toUpperCase() + body.model.slice(1);
      }
      if (body.effort) chosenEffort = body.effort;
    });
  }

  // Sessao de MOTOR: o modelo veio do /v1/models do provedor e foi aplicado com `/model <id>`
  // (o sheet ja chamou o backend). O id do provedor E o rotulo — nada de capitalizar: `k3` nao
  // vira `K3`, e a statusline mostra exatamente esse texto.
  function handleEngineModelApplied(model: string, effort: string | null) {
    chosenModel = model;
    if (effort) chosenEffort = effort;
  }

  // ── Slash commands: preencher x enviar ──────────────────────────────────────
  // Preenche "/nome " no textarea e devolve o foco (pro usuario digitar o argumento).
  async function fillCommand(name: string) {
    inputText = '/' + name + ' ';
    await tick();
    autoGrow();
    textareaEl?.focus();
  }

  // Envia o comando e limpa o textarea (zero-arg ou apos confirmacao).
  function runCommand(cmd: string) {
    inputText = '';
    if (textareaEl) textareaEl.style.height = 'auto';
    onCommand(cmd);
  }

  // `/model` e `/effort` viraram DUAS caixas. Mandar as duas pro seletor de modelo (o que acontecia
  // desde a separacao) deixava `/effort` sem jeito nenhum de chegar no nivel: quem digitava o
  // comando caia na lista de modelos e tinha que fechar e achar a outra pill.
  // Haiku nao tem esforco -> a pill nao existe -> cai no seletor de modelo, que existe sempre.
  function abrirSeletor(qual: 'model' | 'effort') {
    if (qual === 'effort' && !semEsforcoClaude) claudeEffortOpen = true;
    else claudePopOpen = true;
  }

  // Toque numa sugestao do strip inline. model/effort abrem a caixa correspondente; comando com
  // argumento (ou destrutivo) preenche pra revisao antes de enviar; o resto envia direto.
  function handleSuggestPick(cmd: CommandInfo) {
    if (cmd.name === 'model' || cmd.name === 'effort') {
      inputText = '';
      abrirSeletor(cmd.name === 'effort' ? 'effort' : 'model');
      return;
    }
    if (cmd.argumentHint || cmd.destructive) {
      fillCommand(cmd.name);
      return;
    }
    runCommand('/' + cmd.name);
  }

  // ── Textarea: auto-grow ate 120px ──────────────────────────────────────────
  function autoGrow() {
    if (!textareaEl) return;
    textareaEl.style.height = 'auto';
    textareaEl.style.height = Math.min(textareaEl.scrollHeight, 120) + 'px';
  }

  function handleInput() {
    sendError = '';
    limparUndo();   // editar o campo invalida a oferta de desfazer a limpeza do ditado
    autoGrow();
  }

  // Tap em qualquer area do composer que nao seja um controle -> foca o input.
  // .focus() dentro do gesto de clique levanta o teclado no mobile.
  function focusInput(e: Event) {
    const t = e.target as HTMLElement;
    if (t.closest('button, a, input, textarea, [role="option"], [role="listbox"]')) return;
    textareaEl?.focus();
  }

  function handleKeydown(e: KeyboardEvent) {
    // Enter-envia SO no desktop (hover + pointer fine). No teclado do celular, Enter QUEBRA LINHA
    // (comportamento nativo do textarea): enviar era facil demais de disparar sem querer — no
    // mobile o envio e pelo botao. Shift+Enter segue quebrando linha no desktop. Checado na hora
    // (nao cacheado): tablet que pluga/despluga teclado troca de modo sem reload.
    // Enter envia no LAYOUT desktop (>=820px) — não em "hover+pointer fine": iPad com teclado
    // físico segue coarse/touch pro WebKit e o Enter só quebrava linha (visto ao vivo no iPad 10).
    // Celular (<820) continua: Enter quebra linha, envio pelo botão. Shift+Enter quebra sempre.
    if (e.key === 'Enter' && !e.shiftKey
        && window.matchMedia('(min-width: 820px)').matches) {
      e.preventDefault();
      submit();
    }
  }

  // ── Anexos: escolher / remover (multiplas imagens) ─────────────────────────
  // Adiciona arquivos de imagem a lista (do picker ou do paste), cada um com preview local.
  // ditado=true so na gravacao pelo mic (toggleRecord) -> so ela pede limpeza do texto; arquivo de
  // audio anexado (picker/paste) nunca passa por limpeza.
  function addFiles(files: Iterable<File>, opts?: { ditado?: boolean; avisoTeto?: boolean }) {
    for (const f of files) {
      // audio (gravacao do mic ou arquivo audio/*) NAO vira anexo: transcreve e cai no textarea,
      // como se o usuario tivesse digitado — ele revisa/edita e envia quando quiser.
      if (f.type.startsWith('audio/')) {
        transcribeIntoComposer(f, { ditado: !!opts?.ditado, avisoTeto: !!opts?.avisoTeto });
        continue;
      }
      const isImage = f.type.startsWith('image/');
      // url so pra preview de imagem; outros tipos viram chip com o nome (sem objectURL pra revogar).
      attachments = [...attachments, { file: f, url: isImage ? URL.createObjectURL(f) : '', isImage }];
    }
    attachError = '';
  }

  // Transcreve o audio (Groq) e joga o texto no composer, anexando ao que ja houver — o usuario
  // revisa e envia. NAO envia sozinho. Transcricao vazia / falha -> avisa em recError, sem sumir.
  // ditado=true pede `limpar=1`: a resposta ja vem com o texto limpo (`text`), o cru (`raw`, pro
  // desfazer) e um `aviso` quando a limpeza nao valeu -- sem corrida, a troca acontece ANTES da
  // resposta chegar na tela.
  async function transcribeIntoComposer(file: File, opts?: { ditado?: boolean; avisoTeto?: boolean }) {
    // Uma por vez: transcribing e setado SINCRONO antes de qualquer await, entao um segundo audio
    // (ex: multi-selecao no picker) cai aqui e avisa em vez de correr concorrente e pisar no estado
    // compartilhado (transcribing/recError/inputText) — que sairia fora de ordem.
    if (transcribing) { recError = m.composer_aguarde_transcricao(); return; }
    transcribing = true;
    recError = '';
    try {
      const { text, raw, aviso } = await transcribeFile(sessionName, file, { limpar: !!opts?.ditado });
      const t = text.trim();
      if (!t) {
        recError = m.composer_transcricao_vazia();
        // Ditado do mic: falou, ouviu a gravacao encerrar sozinha e nada vai acontecer -- mesmo
        // aviso sonoro dos outros motivos de supressao. So ditado (nao anexo de audio pelo 📎).
        if (opts?.ditado) { somRecusa(); setTimeout(fecharBipes, 400); }
        return;
      }
      // Le inputText SO agora (pos-await, na hora de atribuir): o campo continua digitavel durante
      // "transcrevendo…" (canSend so trava o botao de enviar), entao ler antes do await perderia o
      // que o usuario digitou a mao enquanto esperava o round-trip (mais lento agora, com a limpeza).
      const before = inputText.trim();
      inputText = before ? `${before} ${t}` : t;
      // Oferece desfazer so quando a limpeza de fato mudou o texto (raw ausente/igual -> nada a desfazer).
      const cru = raw?.trim();
      if (opts?.ditado && cru && cru !== t) {
        undo = { before, raw: cru };
        if (undoTimer) clearTimeout(undoTimer);
        undoTimer = setTimeout(limparUndo, 10_000);
      } else {
        limparUndo();
      }
      if (aviso) recError = aviso;
      // Teto (3min sem detectar silencio): diz o motivo provavel em vez de "grave de novo" -- so
      // quando nao ha aviso da limpeza pra mostrar (esse e mais grave, tem prioridade).
      else if (opts?.avisoTeto) {
        recError = m.composer_silencio();
      }
      if (opts?.ditado) {
        if (podeEnviarSozinho({ motivo: motivoDoFim, texto: t, aviso, rascunhoAntes: before.length > 0 })) {
          iniciarContagem();
        } else {
          // Envio automatico suprimido (motivo != silencio, aviso da limpeza ou rascunho ja no
          // campo) -- o texto some calado pra quem esta olhando a tela, mas quem dirige so tem o
          // ouvido. audioCtx so segue aberto numa gravacao maos-livres (teardownRecording ja fechou
          // na hora pro ditado normal), entao o bipe() abaixo e um no-op silencioso fora dela.
          somRecusa();
          setTimeout(fecharBipes, 400);
        }
      }
      await tick();
      autoGrow();
      textareaEl?.focus();
    } catch (err) {
      console.error(m.composer_transcricao_falhou(), err);
      cancelarContagem();   // erro de transcricao nunca inicia contagem
      const status = (err as { status?: number } | null)?.status;
      // Sem o ditado ao vivo, ficar sem chave da Groq passa a significar SEM DITADO NENHUM -> avisa
      // onde resolver, nao so "falhou".
      recError = status === 503
        ? m.composer_groq_chave()
        : err instanceof Error ? err.message : m.composer_falha_transcricao();
      // Mesmo aviso sonoro do ditado que termina sem texto -- inclui o 503 de chave da Groq
      // ausente. So ditado (nao anexo de audio pelo 📎, que nunca teve bipe).
      if (opts?.ditado) { somRecusa(); setTimeout(fecharBipes, 400); }
    } finally {
      transcribing = false;
    }
  }

  // Limpa a oferta de desfazer (timeout, edicao do campo, ou envio).
  function limparUndo() {
    undo = null;
    if (undoTimer) { clearTimeout(undoTimer); undoTimer = undefined; }
  }

  // Troca o texto limpo pelo cru que a Groq devolveu, preservando o que havia antes do ditado.
  function desfazerLimpeza() {
    if (!undo) return;
    const { before, raw } = undo;
    inputText = before ? `${before} ${raw}` : raw;
    limparUndo();
    void tick().then(autoGrow);
  }

  // ── Gravar audio: toggle (tap grava, tap para) -> vira um anexo de audio ─────
  // Para o stream do mic e zera o estado. Chamado no onstop, no onerror, em falha e no onDestroy
  // (trocar de sessao com gravacao ativa desmonta o Composer -> sem isto o mic ficaria ligado).
  function teardownRecording() {
    if (rafId) { cancelAnimationFrame(rafId); rafId = 0; }
    if (recTimer) { clearInterval(recTimer); recTimer = undefined; }
    // Maos-livres: o contexto foi destravado pelo toque no mic e ainda vai tocar os bipes depois da
    // gravacao. Fechar aqui deixaria o aviso sonoro mudo no iPhone. Quem fecha e o fecharBipes().
    if (!maosLivres) {
      audioCtx?.close().catch((err) => console.warn(m.composer_audiocontext_close_falhou(), err));
      audioCtx = undefined;
    }
    recStream?.getTracks().forEach((t) => t.stop());
    recStream = undefined;
    mediaRecorder = undefined;
    recording = false;
    soltarTela();
  }

  // Bipe de confirmacao do maos-livres: reusa o audioCtx destravado pelo toque no mic (um contexto
  // NOVO criado aqui dentro do setInterval da contagem nasceria suspenso no WebKit e sairia mudo).
  function bipe(freq: number, ms: number) {
    const ctx = audioCtx;
    if (!ctx || ctx.state === 'closed') return;   // sem contexto destravado: segue sem som
    try {
      void ctx.resume().catch((err) => console.warn(m.composer_audiocontext_falhou(), err));
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.frequency.value = freq;
      gain.gain.value = 0.05;                     // discreto: confirmacao, nao alarme
      osc.connect(gain).connect(ctx.destination);
      osc.start();
      setTimeout(() => { try { osc.stop(); } catch { /* ja parado */ } }, ms);
    } catch (err) {
      console.warn(m.composer_bipes_falhou(), err);
    }
  }

  const somEnvio = () => bipe(880, 90);     // agudo curto = foi
  const somRecusa = () => bipe(220, 220);   // grave longo = nao foi

  function fecharBipes() {
    audioCtx?.close().catch(() => {});
    audioCtx = undefined;
  }

  // Tela apagada = rAF congelado = detector morto. Segura a tela acesa enquanto o maos-livres grava.
  async function segurarTela() {
    try {
      const lock = await navigator.wakeLock?.request('screen');
      // Gravacao ja terminou enquanto o request estava em voo (2s de silencio ja e o piso) --
      // soltarTela() ja rodou com wakeLock ainda undefined (nada pra liberar), e este sentinel
      // chegaria depois sem ninguem pra soltar neste ciclo. Libera na hora em vez de guardar.
      if (!recording) {
        lock?.release().catch(() => {});
        return;
      }
      wakeLock = lock;
    } catch (err) {
      console.warn(m.composer_wake_lock(), err);   // degrada: o visibilitychange abaixo cobre
    }
  }

  function soltarTela() {
    wakeLock?.release().catch(() => {});
    wakeLock = undefined;
  }

  // Escondeu mesmo com o wake lock (trocou de app, botao de bloqueio): encerra. Continuar gravando
  // sem detector e pior que parar — viraria 3 min de audio que ninguem pediu.
  // NAO readquirir o wakeLock ao voltar: o navegador o solta sozinho ao esconder, e aqui a gravacao
  // ja terminou. Um reacquire brigaria com este pararPorMotivo.
  function aoEsconder() {
    if (!document.hidden) return;
    if (recording && maosLivres) pararPorMotivo('escondeu');
    // Contagem em andamento com a tela escondida: enviar do bolso nao e o que ela promete.
    if (contagem !== null) {
      cancelarContagem();
      // Sem somRecusa() aqui: a tela apagou por um motivo que a gravacao nao escolheu (bloqueio,
      // ligacao entrando, trocou de app) -- um bipe agora interromperia o que quer que esteja
      // tocando aí (ou soaria no bolso, sem ninguem pra ouvir). fecharBipes fecha o contexto calado,
      // sem precisar do delay de 400ms que os outros caminhos usam pra deixar o som terminar.
      fecharBipes();
    }
  }

  $effect(() => {
    document.addEventListener('visibilitychange', aoEsconder);
    return () => document.removeEventListener('visibilitychange', aoEsconder);
  });

  // Medidor de voz: liga um AnalyserNode no stream do mic e empurra o nivel (RMS) numa janela
  // deslizante de barras (~18fps). E so feedback visual — se o Web Audio falhar, a gravacao segue.
  function startMeter(stream: MediaStream) {
    try {
      // Fecha incondicionalmente um contexto que tenha sobrado de uma gravacao anterior — no
      // mãos-livres, teardownRecording NÃO fecha (o bipe ainda precisa dele) e so quem fecha depois
      // e fecharBipes(), que so roda quando o envio automatico acontece. Toda gravacao que termina
      // por outro caminho (botao, teto, escondeu, sem autorizacao de podeEnviarSozinho) deixava esse
      // contexto aberto; sem fechar aqui, o WebKit acumula um AudioContext por ditado ate o iPhone
      // recusar criar outro e o bipe sair mudo sem aviso.
      audioCtx?.close().catch(() => {});
      audioCtx = new AudioContext();
      // iOS: pode iniciar 'suspended' -> waveform ficaria parada. Loga se nao resumir (unica pista).
      void audioCtx.resume().catch((err) => console.warn(m.composer_audiocontext_falhou(), err));
      const analyser = audioCtx.createAnalyser();
      analyser.fftSize = 256;
      audioCtx.createMediaStreamSource(stream).connect(analyser);
      const data = new Uint8Array(analyser.frequencyBinCount);
      let last = 0;
      const loop = () => {
        analyser.getByteTimeDomainData(data);
        let sum = 0;
        for (const v of data) { const x = (v - 128) / 128; sum += x * x; }
        // *5: RMS de voz normal fica baixo (~0.05-0.2); sem ganho as barras mal saem do minimo.
        const level = Math.min(1, Math.sqrt(sum / data.length) * 5);
        // Acumula o quadrado medio de CADA quadro; a decisao sai uma vez por janela de 55ms, que e
        // a mesma cadencia dos envelopes usados pra calibrar a regra (ver lib/vad.ts).
        somaQuad += sum / data.length;
        nQuad++;
        const now = performance.now();
        if (now - last > 55) {   // ~18fps (nao re-renderiza o array a cada frame de tela)
          last = now;
          // cresce da esquerda ate encher a largura (barCount), depois desliza mantendo os ultimos.
          recBars = [...recBars, level].slice(-barCount);
          if (maosLivres && vadEstado && nQuad) {
            const rmsJanela = Math.sqrt(somaQuad / nQuad);   // SEM ganho e SEM clamp
            if (passoVad(vadEstado, rmsJanela, now) === 'encerra') pararPorMotivo('silencio');
          }
          somaQuad = 0;
          nQuad = 0;
        }
        rafId = requestAnimationFrame(loop);
      };
      rafId = requestAnimationFrame(loop);
    } catch (err) {
      console.warn(m.composer_medidor_audio(), err);
      audioCtx?.close().catch(() => {});   // wiring falhou apos criar o ctx -> fecha ja, nao deixa ocioso
      audioCtx = undefined;
    }
  }

  // Grava webm/mp4 e transcreve na Groq ao parar -> cai no composer.
  function startMediaRecorder(stream: MediaStream) {
    mediaRecorder = new MediaRecorder(stream);
    mediaRecorder.ondataavailable = (e) => { if (e.data.size) recChunks.push(e.data); };
    mediaRecorder.onstop = () => {
      const type = mediaRecorder?.mimeType || 'audio/webm';
      // Chrome grava webm/opus; iOS Safari grava mp4/aac. A Groq aceita os dois direto.
      const ext = type.includes('mp4') ? 'm4a' : type.includes('ogg') ? 'ogg' : 'webm';
      // onerror dispara stop logo depois -> se ja falhou, nao anexa o audio (truncado). Sem chunk
      // nenhum (gravacao rapida demais / driver sem dado) -> avisa, nao some calado.
      if (recFailed) {
        teardownRecording();
      } else if (recChunks.length) {
        const blob = new Blob(recChunks, { type });
        // Teto de 3min: transcreve normal, como qualquer ditado -- a regra de silencio nao
        // sobrevive a ruido de fundo constante (piso de RMS medido abaixo de motor de carro), e o
        // teto e justamente o caminho provavel de quem dita dirigindo. Nao envia sozinho mesmo
        // assim (podeEnviarSozinho ja barra por motivo != 'silencio'); avisoTeto troca a mensagem
        // por uma que diga o motivo provavel em vez de jogar a fala fora.
        addFiles([new File([blob], `gravacao-${Date.now()}.${ext}`, { type })],
          { ditado: true, avisoTeto: motivoDoFim === 'teto' });
        teardownRecording();
      } else {
        recError = m.composer_gravacao_vazia();
        teardownRecording();
      }
    };
    mediaRecorder.onerror = (e) => {
      console.error(m.composer_mediarecorder_erro(), (e as { error?: unknown }).error ?? e);
      recFailed = true;
      recError = m.composer_falha_gravacao();
      // Maos-livres: audioCtx ainda esta aberto (teardownRecording so fecha quando !maosLivres) --
      // e o unico caminho de falha do arquivo sem som, entao avisa quem esta dirigindo. Fora do
      // maos-livres o contexto ja fechou e a pessoa esta com o celular na mao, vendo o erro na tela.
      if (maosLivres) {
        somRecusa();
        setTimeout(fecharBipes, 400);
      }
      teardownRecording();
    };
    mediaRecorder.start();
  }

  async function toggleRecord() {
    if (recording) {
      pararPorMotivo('botao');
      return;
    }
    // 2o tap/atalho DURANTE o "preparando…" CANCELA a espera: sem isto, um prompt de permissao
    // ignorado (a promise do getUserMedia nunca resolve nem rejeita) prendia o hint na tela e o
    // botao morria — o toque seguinte era engolido calado pelo guard de reentrancia. A promise em
    // voo nao da pra abortar; a geracao abaixo invalida o que ela resolver depois.
    if (starting) { recGeracao++; starting = false; return; }
    if (transcribing) return;   // nao regravar durante a transcricao
    starting = true;
    const geracao = ++recGeracao;
    recError = '';
    recFailed = false;
    recBars = [];
    recSeconds = 0;

    // O mic captaria a propria narracao: o detector nunca veria silencio e o whisper transcreveria
    // o que o app estava lendo em voz alta.
    if (ttsPlayer.playing) ttsPlayer.toggle();

    // Grava pelo mic e transcreve na Groq ao parar; waveform real do audio.
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (err) {
      // Reject TARDIO de uma tentativa morta (cancelada ou componente destruido): silencio total —
      // zerar starting aqui mataria o hint de uma tentativa NOVA em voo, e o recError seria um
      // erro de algo que o usuario deliberadamente cancelou.
      if (destroyed || geracao !== recGeracao) return;
      console.error(m.composer_getusermedia_falhou(), err);
      const name = err instanceof DOMException ? err.name : '';
      recError = name === 'NotFoundError' ? m.composer_sem_microfone()
        : name === 'NotReadableError' ? m.composer_mic_em_uso()
        : m.composer_sem_acesso_mic();
      starting = false;
      return;
    }
    // A promise resolveu TARDE: ou o usuario cancelou a espera (2o tap), ou trocou de sessao e
    // este Composer morreu no meio do await. Sem este guard, a stream abria o mic num componente
    // destruido/sem UI — gravador e interval vazavam pra sempre, sem ninguem pra chamar stop().
    if (destroyed || geracao !== recGeracao) {
      stream.getTracks().forEach((t) => t.stop());
      return;
    }
    recStream = stream;
    maosLivres = lerMaosLivres();
    vadEstado = maosLivres ? novoEstadoVad() : undefined;
    motivoDoFim = null;
    recIniciadoEm = Date.now();
    somaQuad = 0;
    nQuad = 0;
    if (maosLivres) void segurarTela();
    recChunks = [];
    try {
      startMediaRecorder(stream);
      recTimer = setInterval(() => {
        recSeconds++;
        if (maosLivres && Date.now() - recIniciadoEm >= TETO_MS) pararPorMotivo('teto');
      }, 1000);
      startMeter(stream);
    } catch (err) {
      console.error(m.composer_inicio_gravacao_falhou(), err);
      recError = m.composer_audio_nao_suportado();
      teardownRecording();
      starting = false;
      return;
    }
    recording = true;
    starting = false;
  }

  onDestroy(teardownRecording);
  onDestroy(() => { destroyed = true; });   // getUserMedia em voo se descarta ao resolver (toggleRecord)
  onDestroy(limparUndo);   // troca de sessao desmonta o Composer -> nao deixa o setTimeout solto
  onDestroy(cancelarContagem);   // troca de sessao desmonta o Composer -> nao deixa o setInterval solto
  // Fecha o audioCtx incondicionalmente no unmount: teardownRecording() so fecha quando !maosLivres
  // (o bipe ainda pode precisar dele), entao sem isto uma sessao trocada com mãos-livres ligado
  // deixava o contexto aberto pra sempre. fecharBipes() e idempotente (audioCtx?.close()).
  onDestroy(fecharBipes);

  function onPickFile(e: Event) {
    const files = (e.target as HTMLInputElement).files;
    if (files && files.length) addFiles(files);
  }

  // Colar imagem(ns) (desktop garante; iOS Safari e instavel). Pega todos os itens de imagem
  // do clipboard e joga no mesmo fluxo do anexo.
  function onPaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    const imgs: File[] = [];
    for (const it of items) {
      if (it.kind === 'file' && it.type.startsWith('image/')) {
        const f = it.getAsFile();
        if (f) imgs.push(f);
      }
    }
    if (imgs.length) {
      e.preventDefault();
      addFiles(imgs);
    }
  }

  // Remove um anexo pelo indice (libera o objectURL).
  function removeAttachment(idx: number) {
    const a = attachments[idx];
    if (a?.url) URL.revokeObjectURL(a.url);
    attachments = attachments.filter((_, i) => i !== idx);
    attachError = '';
    if (fileInput) fileInput.value = '';
  }

  // Limpa todos os anexos (apos envio com sucesso).
  function clearAttachments() {
    for (const a of attachments) if (a.url) URL.revokeObjectURL(a.url);
    attachments = [];
    if (fileInput) fileInput.value = '';
  }

  // Devolve se o envio deu certo -- enviarAutomatico() usa isso pra escolher o bipe (agudo/grave)
  // SO depois do desfecho real, nunca antes. O botao de enviar (onclick={submit}) ignora o retorno,
  // igual sempre ignorou.
  // ctrl-s sem texto: não passa pelo submit (não há o que digitar nem o que limpar). Erro vai pro
  // mesmo lugar do erro de envio — falha calada aqui seria um toque que não faz nada.
  async function steerFila(): Promise<void> {
    sendError = '';
    try {
      await onSteer?.();
    } catch (err) {
      sendError = err instanceof Error ? err.message : m.composer_fila_erro();
    }
  }

  async function submit(): Promise<boolean> {
    if (!canSend) return false;
    cancelarContagem();   // envio manual torna a contagem sem sentido
    const caption = inputText.trim();
    sendError = '';
    limparUndo();   // enviou -> a oferta de desfazer a limpeza do ditado nao faz mais sentido
    if (attachments.length) {
      uploading = true;
      attachError = '';
      let ok = true;
      try {
        // Sobe todos os anexos e junta os paths numa UNICA linha (o backend rejeita '\n' no
        // send-keys). Cada path nao tem espaco (nome gerado). Marca imagem x arquivo pelo tipo.
        const parts: string[] = [];
        for (const a of attachments) {
          // Encolhe foto/converte HEIC antes de subir. Falhou? prepareImage devolve o original.
          const arquivo = a.isImage ? await prepareImage(a.file) : a.file;
          const { path, frames, transcript } = await uploadFile(sessionName, arquivo);
          parts.push((a.isImage ? `📎 ${m.board_imagem()}: ` : `📎 ${m.board_arquivo()}: `) + path);
          // Video: o backend extraiu quadros ao longo da duracao e transcreveu a fala. Os quadros
          // entram como imagens (o Read abre; a lista tambem vira miniatura no chat) e a fala vai
          // como texto — sem isso o mp4 era um caminho que o modelo nao consegue abrir.
          for (const q of frames ?? []) parts.push(`📎 ${m.board_imagem()}: ` + q);
          if (transcript) parts.push(m.composer_fala_video({ texto: transcript }));
        }
        const attachPart = parts.join(' ');
        const msg = (caption ? caption + ' — ' : '') + attachPart;
        await onSend(msg);                 // espera o /input; so limpa se foi
        inputText = '';
        if (textareaEl) textareaEl.style.height = 'auto';
        clearAttachments();
      } catch (err) {
        // upload OU envio falhou -> mantem as fotos e o texto, mostra o erro.
        attachError = err instanceof Error ? err.message : m.composer_falha_envio();
        ok = false;
      } finally {
        uploading = false;
      }
      return ok;
    }
    // texto puro: limpa a caixa OTIMISTA (no toque) pra tirar a sensacao de lag do round-trip; restaura
    // o texto se o /input falhar (nao perde nada). O settle ~200ms do backend (anti-"Enter engolido")
    // continua valendo no servidor, mas a UI nao bloqueia mais a limpeza nele.
    sending = true;
    inputText = '';
    if (textareaEl) textareaEl.style.height = 'auto';
    let ok = true;
    try {
      await onSend(caption);
    } catch (err) {
      // falhou -> devolve o texto, MAS so se a caixa segue vazia (nao pisa no que o usuario digitou
      // na janela do envio em voo).
      if (!inputText.trim()) inputText = caption;
      sendError = err instanceof Error ? err.message : m.composer_falha_envio();
      ok = false;
    } finally {
      sending = false;
    }
    return ok;
  }

  // Auto-focus quando ocioso — SO em desktop (ponteiro fino). No iOS, focus() programatico nao abre
  // o teclado (so abre com gesto real) mas mexe no estado de foco do campo -> briga com o tap do
  // usuario e o teclado so abre depois de varios toques. matchMedia('(pointer: fine)') = mouse/trackpad
  // (desktop), exclui touch.
  $effect(() => {
    if (sessionState === 'idle' && textareaEl && window.matchMedia('(pointer: fine)').matches) {
      setTimeout(() => textareaEl?.focus(), 100);
    }
  });
</script>

<footer class="composer">
  <input
    type="file"
    accept="*/*"
    multiple
    bind:this={fileInput}
    onchange={onPickFile}
    class="file-input"
    aria-hidden="true"
    tabindex="-1"
  />
  <!-- O card precisa delegar foco para a textarea em areas vazias, mas contem varios botoes:
       nao pode virar button/role=button sem aninhar controles interativos. -->
  <div class="composer-card" role="button" tabindex="-1" onclick={focusInput} onkeydown={focusInput}>
    <div class="composer-top">
      <div class="top-left">
        {#if !isCodex}
          <button class="slash-btn" onclick={() => (commandSheetOpen = true)} aria-label={m.comandos_titulo()}>
            <span class="slash-glyph" aria-hidden="true">/</span>
          </button>
        {/if}
        <button class="slash-btn" onclick={onOpenPreview} aria-label={m.composer_preview_rodando()}>
          <IconMonitor size={17} />
        </button>
        {#if onOpenPair}
          {@const pairLabel = pairPeers?.length === 1 ? pairPeers[0]
            : pairPeers?.length ? `grupo (${pairPeers.length + 1})` : null}
          <button class="repo-chip pair-chip" class:pair-chip--on={!!pairPeers?.length}
                  title={pairPeers?.length ? m.composer_grupo_voce({ n: pairPeers.join(', ') }) : m.composer_parear_outra()}
                  onclick={onOpenPair} aria-label={m.composer_pareamento_sessoes()}>
            <span class="repo-glyph" aria-hidden="true">🤝</span>
            {#if pairLabel}
              <span class="repo-name">{pairLabel}</span>
              {#if pairedState}
                <!-- Estado vivo do par ÚNICO (mesmas cores da lista); grupo de N não tem bolinha. -->
                <span class="pair-dot" style="background: {stateColors[pairedState as keyof typeof stateColors] ?? 'var(--text-muted)'};"
                      title={m.composer_par_estado({ n: pairedState })} aria-hidden="true"></span>
              {/if}
            {/if}
          </button>
          {#if pairPeers?.length && onToggleSendToPair}
            <!-- "Mandar pro grupo": prompt vai pra esta sessão E pros membros (broadcast). Aceso = ativo. -->
            <button class="repo-chip both-chip" class:both-chip--on={sendToPair}
                    title={sendToPair ? m.composer_mandando_grupo() : m.composer_mandar_tambem({ n: pairPeers.join(', ') })}
                    onclick={onToggleSendToPair} aria-pressed={sendToPair} aria-label={m.composer_mandar_grupo()}>
              <span class="repo-glyph" aria-hidden="true">⇄</span>
              {#if sendToPair}<span class="repo-name">{pairPeers.length === 1 ? m.composer_pros_dois() : m.composer_pro_grupo()}</span>{/if}
            </button>
          {/if}
        {/if}
        {#if isKimi && isWorking && filaCount > 0 && onSteer}
          <!-- FILA da TUI do Kimi: msg já mandada, esperando o turno atual acabar. O chip existe pra
               DIZER que há fila (antes disso a bolha translúcida era a única pista) e dar a saída:
               tocar manda o `ctrl-s`, que promove a msg pro turno em curso. Não tocar = espera, que
               é o comportamento de sempre. -->
          <button class="repo-chip fila-chip" onclick={steerFila}
                  title={m.composer_fila_titulo()}
                  aria-label={m.composer_fila_aria()}>
            <span class="repo-glyph" aria-hidden="true">⏳</span>
            <span class="repo-name">{m.composer_fila_contagem({ n: filaCount })}</span>
            <span class="repo-sep" aria-hidden="true">·</span>
            <span class="fila-acao">{m.composer_fila_acao()}</span>
          </button>
        {/if}
        {#if status?.repo}
          <button class="repo-chip" title={m.composer_git_chip()} onclick={onOpenGit}>
            <IconFolder size={13} />
            <span class="repo-name">{status.repo}</span>
            {#if status.branch}
              <span class="repo-sep" aria-hidden="true">·</span>
              <span class="repo-branch">{status.branch}{#if status.dirty}<span class="repo-dirty" aria-label={m.composer_alteracoes_nao_commitadas()}>*</span>{/if}</span>
            {/if}
          </button>
        {/if}
      </div>
      {#if lastCache}
        <!-- Prazo do cache. Nao e botao: nao ha o que fazer com ele alem de saber. -->
        <span
          class="cache-chip"
          class:acabando={cacheAcabando}
          class:frio={!cacheAtivo}
          title={cacheAtivo
            ? m.composer_cache_vale({ label: cacheLabel, janela: lastCache.ttl >= 3600 ? m.composer_cache_1_hora() : m.composer_cache_5_min() })
            : m.composer_cache_expirou()}
        >
          <span class="cache-glyph" aria-hidden="true"></span>{cacheLabel}
        </span>
      {/if}
    </div>

    {#if attachments.length}
      <div class="attach-row">
        {#each attachments as a, idx (a.file)}
          <div class="attach-chip">
            {#if a.isImage}
              <img class="attach-thumb" src={a.url} alt="anexo" />
            {:else}
              <span class="attach-file" title={a.file.name}>
                <span class="attach-file-glyph" aria-hidden="true">📎</span>
                <span class="attach-file-name">{a.file.name}</span>
              </span>
            {/if}
            <button class="attach-remove" onclick={() => removeAttachment(idx)} aria-label={m.board_remover_anexo()}>×</button>
          </div>
        {/each}
        {#if uploading}<span class="attach-status">{m.composer_enviando()}</span>{/if}
        {#if attachError}<span class="attach-error">{attachError}</span>{/if}
      </div>
    {/if}

    {#if !isCodex}
      <SlashSuggest {commands} query={inputText} onPick={handleSuggestPick} />
    {/if}

    <textarea
      bind:this={textareaEl}
      bind:value={inputText}
      class="composer-textarea"
      placeholder={isCodex ? m.composer_mensagem_para({ nome: 'Codex' }) : isKimi ? m.composer_mensagem_para({ nome: 'Kimi' }) : m.composer_mensagem_para({ nome: 'Claude' })}
      rows={1}
      oninput={handleInput}
      onkeydown={handleKeydown}
      onpaste={onPaste}
      aria-label={m.composer_aria_mensagem()}
    ></textarea>

    {#if starting && !recording}
      <!-- O getUserMedia leva 300-800ms pra acordar o mic — sem este aviso, quem aperta Ctrl+Espaco
           e ja sai falando nao ve nada acontecer e perde o comeco da frase. Quando o recorder
           dispara, este hint vira o de "gravando" (timer + waveform) logo abaixo: o timer e o
           sinal de "pode falar". -->
      <div class="rec-hint" role="status" aria-label={m.composer_preparando_mic()}>
        <span class="rec-dot rec-dot--waiting" aria-hidden="true"></span>
        <span class="rec-time">{m.composer_preparando_mic_curto()}</span>
      </div>
    {/if}
    {#if recording}
      <div class="rec-hint" role="status" aria-label={m.composer_gravando_audio()}>
        <span class="rec-dot" aria-hidden="true"></span>
        <!-- timer aria-hidden: senao o leitor de tela re-anunciaria a cada segundo (a live region so
             precisa dizer "Gravando áudio" uma vez, via aria-label). -->
        <span class="rec-time" aria-hidden="true">{recTimeLabel}</span>
        <span class="rec-wave" bind:clientWidth={waveW} aria-hidden="true">
          {#each recBars as b, i (i)}<span class="rec-bar" style="--h: {b}"></span>{/each}
        </span>
      </div>
    {/if}
    {#if transcribing}
      <div class="rec-hint" role="status" aria-label={m.composer_transcrevendo_audio()}>
        <span class="rec-dot" aria-hidden="true"></span>
        <span class="rec-time">{m.board_transcrevendo()}</span>
      </div>
    {/if}
    {#if contagem !== null}
      <div class="rec-hint" role="status">
        <span>{m.composer_enviando_cancelar({ n: contagem })}</span>
      </div>
    {/if}
    {#if undo}
      <!-- Oferece voltar ao texto cru do ditado (antes da limpeza). Some sozinha (limparUndo): ao
           enviar, ao editar o campo, ou apos ~10s. -->
      <div class="rec-hint" role="status">
        <span class="rec-time">{m.composer_ditado_limpo()}</span>
        <button type="button" class="undo-btn" onclick={desfazerLimpeza}>{m.composer_original()}</button>
      </div>
    {/if}
    {#if recError}
      <div class="send-error" role="alert">{recError}</div>
    {/if}
    {#if sendError}
      <div class="send-error" role="alert">{sendError}</div>
    {/if}
    {#if modelError}
      <div class="send-error" role="alert">{modelError}</div>
    {/if}
    {#if permError}
      <div class="send-error" role="alert">{permError}</div>
    {/if}

    <div class="control-row">
      <div class="control-left">
        {#if isPi}
          <!-- pill-duo: no celular as duas pills (modelo + esforço) se fundem num chip só —
               fundo compartilhado, divisor fino, cada metade abre seu popover. Desktop:
               display:contents, idêntico a duas pills soltas. -->
          <span class="pill-duo">
            <button
              class="model-pill"
              bind:this={piPillEl}
              onclick={() => (piPopOpen = true)}
              aria-haspopup="dialog"
              aria-expanded={piPopOpen}
              aria-label={m.composer_modelo_pi()}
            >
              <span class="pill-label">
                <span class="pill-model">{piModel ?? status?.model ?? m.composer_modelo()}</span>
              </span>
              <ContextRing pct={status?.ctxPct ?? null} />
            </button>
            <!-- Nivel de raciocinio em pill propria (referencia: opencode). Antes vivia dentro da
                 folha de modelo, atras da lista de 390 itens. -->
            <button
              class="model-pill"
              bind:this={piEffortPillEl}
              onclick={() => (piEffortOpen = true)}
              aria-haspopup="dialog"
              aria-expanded={piEffortOpen}
              aria-label={m.composer_nivel_raciocinio_pi()}
            >
              <span class="pill-label">
                <span class="pill-model">{piEffort ?? m.composer_nivel()}</span>
              </span>
            </button>
          </span>
        {:else if isKimi}
          <!-- Kimi: pill-duo como o Pi — modelo abre o KimiModelPopover (catálogo do config.toml;
               aplica dirigindo o picker do /model com busca + Alt+S) e o nível de pensamento abre
               o KimiEffortPopover (linha "Thinking (←→)" do mesmo picker). Os dois valem SÓ pra
               sessão: nada de escrever no default global do config.toml. -->
          <span class="pill-duo">
            <button
              class="model-pill"
              bind:this={kimiPillEl}
              onclick={() => (kimiPopOpen = true)}
              aria-haspopup="dialog"
              aria-expanded={kimiPopOpen}
              aria-label={m.composer_modelo()}
            >
              <span class="pill-label">
                <span class="pill-model">{kimiModel ?? status?.model ?? m.composer_modelo()}</span>
              </span>
              <ContextRing pct={status?.ctxPct ?? null} />
            </button>
            <button
              class="model-pill"
              bind:this={kimiEffortPillEl}
              onclick={() => (kimiEffortOpen = true)}
              aria-haspopup="dialog"
              aria-expanded={kimiEffortOpen}
              aria-label={m.composer_esforco()}
            >
              <span class="pill-label">
                <span class="pill-model">{kimiEffort ?? status?.effort ?? m.composer_esforco()}</span>
              </span>
            </button>
          </span>
        {:else if !isCodex}
          <span class="pill-duo">
            <button
              class="model-pill"
              bind:this={claudePillEl}
              onclick={() => (claudePopOpen = true)}
              aria-haspopup="dialog"
              aria-expanded={claudePopOpen}
              aria-label={m.composer_modelo_contexto()}
            >
              <span class="pill-label">
                <span class="pill-model">{pillModel ?? m.composer_modelo()}</span>
              </span>
              <ContextRing pct={status?.ctxPct ?? null} />
            </button>
            <!-- Esforco em pill propria. Some no Haiku, que nao usa esforco (o picker responde
                 "Effort not supported") — pill que nao aplica nada e pior que pill ausente. -->
            {#if !semEsforcoClaude}
              <button
                class="model-pill"
                bind:this={claudeEffortPillEl}
                onclick={() => (claudeEffortOpen = true)}
                aria-haspopup="dialog"
                aria-expanded={claudeEffortOpen}
                aria-label={m.composer_esforco_raciocinio()}
              >
                <span class="pill-label">
                  <span class="pill-model">{pillEffort ?? m.composer_esforco()}</span>
                </span>
              </button>
            {/if}
            <!-- Permissão: terceira pill só em Claude, fora do duo (chip próprio). -->
            <button
              class="model-pill"
              bind:this={permPillEl}
              onclick={abrirPermissao}
              aria-haspopup="dialog"
              aria-expanded={permPopOpen}
              aria-label={m.composer_permissao()}
            >
              <span class="pill-label">
                <span class="pill-model">{permCurrent ?? m.composer_permissao()}</span>
              </span>
            </button>
          </span>
        {:else}
          <button
            class="model-pill"
            onclick={() => (codexSheetOpen = true)}
            aria-label={m.composer_modelo_codex()}
          >
            <span class="pill-label">
              <span class="pill-model">{codexModel ?? m.composer_modelo()}</span>
              {#if codexEffort}<span class="pill-effort">· {codexEffort}</span>{/if}
            </span>
            <ContextRing pct={status?.ctxPct ?? null} />
          </button>
        {/if}
        <button class="attach-btn" onclick={() => fileInput?.click()} aria-label={m.composer_anexar_arquivo()}>
          <IconAttach size={20} />
        </button>
        <button
          class="attach-btn mic-btn"
          class:mic-btn--recording={recording}
          onclick={toggleRecord}
          aria-label={recording ? m.composer_parar_gravacao() : starting ? m.composer_cancelar_prep_mic() : m.composer_gravar_audio()}
        >
          {#if recording}<IconInterrupt size={18} />{:else}<IconMic size={20} />{/if}
        </button>
        <!-- Estilo do ditado, colado no microfone e na MESMA pill do modelo/esforço: é decisão que
             se toma ANTES de falar, do mesmo tamanho que escolher o esforço. Some durante a
             gravação — trocar no meio não muda nada (quem lê o estilo é o backend, no fim) e a
             pill só roubaria o alvo do dedo que vai parar de gravar. -->
        {#if !recording && !starting}
          <button
            class="model-pill"
            bind:this={estiloPillEl}
            onclick={() => (estiloAberto = true)}
            aria-haspopup="dialog"
            aria-expanded={estiloAberto}
            aria-label={m.ditado_estilo_titulo() + ': ' + rotuloEstilo}
          >
            <span class="pill-label">
              <span class="pill-model">{rotuloEstilo}</span>
            </span>
          </button>
        {/if}
      </div>

      <div class="control-right">
        {#if isWorking && !hasInput}
          <!-- Pensando + input vazio -> o slot vira STOP. Ao digitar/colar algo, volta a ser SEND
               (enfileira a msg). Um slot so -> ganha espaco. -->
          <button class="stop-btn" onclick={() => (confirmStopOpen = true)} aria-label={m.composer_interromper_aria()}>
            <IconInterrupt size={18} />
          </button>
        {:else}
          <button
            class="send-btn"
            class:send-btn--disabled={!canSend}
            onclick={() => submit()}
            disabled={!canSend}
            aria-label={isKimi && isWorking ? m.composer_enviar_fila_kimi() : m.composer_enviar_mensagem()}
          >
            <IconSend size={18} />
          </button>
        {/if}
      </div>
    </div>
  </div>

  {#if stats}
    <!-- Faixa de estatísticas (app/stats.py). Números de tempo/velocidade são aproximados
         por construção -> "~" no rótulo. transparent: quem carrega o material é o .composer. -->
    <!-- tabindex: a faixa rola de lado sem barra visível; sem foco, teclado não alcança o
         que transbordou. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -- região rolável: o padrão WAI manda
         ela ser focável pra rolar por teclado (role=region + rótulo). -->
    <div class="stats-strip" role="region" aria-label={m.stats_faixa_aria()} tabindex="0">
      <!-- wrapper com margin-inline auto: centraliza quando cabe, e quando transborda as margens
           colapsam pra 0 -> o INÍCIO continua alcançável pelo scroll (justify-content: center
           cortaria a esquerda sem como rolar até ela). -->
      <span class="stats-inner">
      <span>{stats.turns === 1 ? m.stats_turnos_1() : m.stats_turnos({ n: stats.turns })}</span>
      <span class="dot">·</span>
      <span>{stats.steps === 1 ? m.stats_chamadas_1() : m.stats_chamadas({ n: stats.steps })}</span>
      {#if stats.llm_ms}
        <span class="sep">|</span>
        <span>{m.stats_llm({ t: fmtDur(stats.llm_ms) })}</span>
        {#if stats.tool_ms}
          <span class="dot">·</span>
          <span>{m.stats_tools({ t: fmtDur(stats.tool_ms) })}</span>
        {/if}
      {/if}
      {#if stats.tok_s}
        <span class="sep">|</span>
        <span>{m.stats_toks({ n: Math.round(stats.tok_s) })}</span>
      {/if}
      {#if stats.ttft_ms}
        <span class="dot">·</span>
        <span>{m.stats_ttft({ t: fmtDur(stats.ttft_ms) })}</span>
      {/if}
      {#if stats.cache_pct != null}
        <span class="sep">|</span>
        <span>{m.stats_cache({ n: stats.cache_pct })}</span>
      {/if}
      <span class="sep">|</span>
      <span>{m.stats_io({ i: abbrevNum(stats.in_tok), o: abbrevNum(stats.out_tok) })}</span>
      </span>
    </div>
  {/if}

  <ClaudeModelPopover
    open={claudePopOpen}
    anchor={claudePillEl}
    {sessionName}
    currentModel={pillModel}
    currentEffort={pillEffort}
    onApply={handleApply}
    onApplied={handleEngineModelApplied}
    onFail={(msg) => (modelError = msg)}
    onClose={() => (claudePopOpen = false)}
  />

  <ClaudeEffortPopover
    open={claudeEffortOpen}
    anchor={claudeEffortPillEl}
    currentEffort={pillEffort}
    onApply={handleApply}
    onClose={() => (claudeEffortOpen = false)}
  />

  <ClaudePermissionPopover
    open={permPopOpen}
    anchor={permPillEl}
    current={permCurrent}
    modes={permModes}
    sondavel={permSondavel}
    carregando={permCarregando}
    onApply={handlePermApply}
    onClose={() => (permPopOpen = false)}
  />

  <CodexModelSheet
    open={codexSheetOpen}
    {sessionName}
    onApplied={handleCodexModelApplied}
    onClose={() => (codexSheetOpen = false)}
  />

  <PiModelPopover
    open={piPopOpen}
    anchor={piPillEl}
    {sessionName}
    onApplied={handlePiModelApplied}
    onClose={() => (piPopOpen = false)}
  />

  <PiEffortPopover
    open={piEffortOpen}
    anchor={piEffortPillEl}
    {sessionName}
    onApplied={(effort) => (piEffort = effort)}
    onClose={() => (piEffortOpen = false)}
  />

  <KimiModelPopover
    open={kimiPopOpen}
    anchor={kimiPillEl}
    {sessionName}
    currentName={kimiModel ?? status?.model ?? null}
    onApplied={(model) => (kimiModel = model)}
    onClose={() => (kimiPopOpen = false)}
  />

  <KimiEffortPopover
    open={kimiEffortOpen}
    anchor={kimiEffortPillEl}
    {sessionName}
    currentName={kimiModel ?? status?.model ?? null}
    currentEffort={kimiEffort ?? status?.effort ?? null}
    onApplied={(effort) => (kimiEffort = effort)}
    onClose={() => (kimiEffortOpen = false)}
  />

  <CommandSheet
    open={commandSheetOpen}
    {commands}
    onCommand={runCommand}
    onFill={fillCommand}
    onOpenModelEffort={abrirSeletor}
    onClose={() => (commandSheetOpen = false)}
  />

  <DitadoEstiloPopover
    open={estiloAberto}
    anchor={estiloPillEl}
    onClose={() => (estiloAberto = false)}
  />

  <ConfirmSheet
    open={confirmStopOpen}
    title={m.composer_interromper_claude()}
    message={m.composer_interromper_msg()}
    confirmLabel={m.composer_interromper()}
    danger={true}
    onConfirm={onInterrupt}
    onClose={() => (confirmStopOpen = false)}
  />
</footer>

<style>
  .composer {
    background: transparent;
    /* Folga do home indicator vive AQUI (externa) -> o card flutua com respiro do fundo, sem
       colar na borda da tela. max() = só a folga do indicator quando há, senão o space-2 mínimo. */
    padding: var(--space-2) var(--space-3) var(--composer-pb, max(var(--space-2), env(safe-area-inset-bottom)));
  }
  /* Desktop: nao ha home indicator pra desviar, entao o piso de 8px deixava o card colado na borda
     da janela. Mais respiro embaixo — o dock flutua, e o que separa ele do fim da tela e esta folga. */
  @media (min-width: 820px) {
    .composer { padding-bottom: var(--composer-pb, var(--space-5)); }
  }

  /* Faixa de estatísticas: métrica passiva sob o card (precedente do cache-chip), na cor
     apagada e SEM fundo próprio — o vidro é do .composer. Numa tela estreita rola de lado
     (scrollbar escondida): quebrar linha empurraria o textarea pra cima. */
  .stats-strip {
    display: flex;
    align-items: center;
    padding: 6px 4px 0;
    /* 11px: o mesmo degrau da faixa de cota logo abaixo. 11,5 era o único no app inteiro. */
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow-x: auto;
    scrollbar-width: none;
    background: transparent;
  }
  .stats-strip::-webkit-scrollbar { display: none; }
  .stats-inner {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-inline: auto;   /* centro quando cabe; 0 quando transborda (início alcançável) */
  }
  .stats-strip .dot { opacity: 0.5; }
  .stats-strip .sep { color: var(--border-strong); }

  /* Card unico que reune status, textarea e controles. */
  .composer-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 600px;
    margin: 0 auto;
    /* Card de vidro fosco. O blur NÃO fica mais aqui: ficava no MESMO elemento que tem conteúdo +
       borda + filhos, forçando o WebKit a promover a subárvore e quebrar o fast-path do scroll ->
       bloco PRETO no topo durante o streaming (WebKit #89475). Agora o filtro vive só no ::before
       (leaf isolado). Host = stacking context próprio (position+isolation), transparente, só
       borda/sombra. Padrão de produção (Ionic .footer-background). Os sheets do Composer são IRMÃOS
       do card (fora dele) -> isolation/position aqui NÃO os clipa. */
    position: relative;
    isolation: isolate;
    border: 1px solid var(--glass-border);
    box-shadow:               /* specular rim (brilho de borda) = cara de glass iOS; glow no host */
      inset 0 1px 1px var(--glass-specular),
      inset 0 -1px 1px rgba(255, 255, 255, 0.05),
      0 1px 2px rgba(0, 0, 0, 0.18),
      0 12px 40px var(--glass-shadow);
    border-radius: var(--radius-lg);
    /* Padding interno uniforme. A folga do home indicator saiu daqui pro .composer (margem externa)
       -> o card nao cola mais na borda; flutua com respiro do fundo. */
    padding: var(--space-3);
  }

  /* Camada de vidro: leaf bare (sem conteúdo, sem descendente posicionado), bounded à caixa do dock. */
  .composer-card::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;                /* atrás do conteúdo, dentro do stacking context do card */
    border-radius: inherit;
    pointer-events: none;
    /* WebKit/iOS: SEM backdrop-filter. Tira o blur(40px) e com ele o bug #89475 (bloco preto no
       streaming/momentum) de vez — sem filtro, nada pra promover/corromper. Fundo quase opaco = o
       look de "vidro" que antes só aparecia no scroll, agora permanente. */
    background: var(--chrome-bg);   /* --chrome-bg = o mesmo valor, mas entra no veu quando ha papel de parede (app.css) */
  }
  /* Chromium (data-liquid): refracao SVG real (liquid glass). O blur fica aqui — Chromium não tem o
     bug do WebKit. Fundo transparente pra refração aparecer. */
  :global(html[data-liquid]) .composer-card::before {
    background: var(--glass-bg);
    backdrop-filter: url(#liquid-glass) blur(16px) saturate(180%);
  }

  /* Desktop: composer mais largo (aditivo; mobile fica nos 600px). min() acompanha a lista
     (messages-inner): sem degrau 600->1400 em tablet. */
  @media (min-width: 820px) {
    .composer-card { max-width: min(1400px, 94vw); }
  }

  /* ── Textarea (transparente dentro do card) ─────────────────────────────── */
  .composer-textarea {
    width: 100%;
    min-height: 24px;
    max-height: 120px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    line-height: 1.55;
    padding: var(--space-1) 0;
    resize: none;
    outline: none;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .composer-textarea::placeholder {
    color: var(--text-muted);
  }

  /* ── Control row ────────────────────────────────────────────────────────── */
  .control-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    min-height: 44px;
  }

  .control-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  /* Chip de modelo/esforco: botao que abre o popover correspondente (mantem o visual do chip).
     min-height:0 sobrescreve o alvo global de 44px pra preservar o pill compacto de 28px
     (o tap fica confortavel dentro da control-row de 44px). :active scale vem do global. */
  .model-pill {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    height: 30px;
    min-height: 0;
    padding: 0 var(--space-2) 0 var(--space-3);
    background: var(--accent-dim);
    border-radius: var(--radius-md);
    font-size: var(--text-xs);
    font-weight: 500;
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  /* Duo modelo+esforço: no desktop não existe visualmente (display:contents = duas pills
     soltas, como sempre). No celular vira UM chip estilo app do Claude ("Opus 5 Alto"):
     fundo compartilhado, esforço em cinza, dois alvos de toque com emenda invisível. */
  .pill-duo {
    display: contents;
  }
  @media (max-width: 819px) {
    /* 8px entre controles (era 4): com 4px o vão entre as peças ficava MENOR que o padding de
     dentro delas, e a fileira lia como um bloco único em vez de cinco controles. */
  .control-left { gap: var(--space-2); flex-wrap: wrap; }       /* com três pílulas a fileira não cabe em 390px e quem encolhe é sempre o nome do modelo, porque as irmãs têm flex-shrink 0 */
    .pill-duo {
      display: inline-flex;
      align-items: center;
      background: var(--accent-dim);
      border-radius: var(--radius-md);
      flex-shrink: 1;
      min-width: 0;
    }
    .pill-duo > .model-pill {
      background: transparent;
      flex-shrink: 1;
      min-width: 0;
      gap: var(--space-1);                       /* anel de contexto mais colado no nome */
    }
    /* A pill avulsa (estilo do ditado) também perde o padding assimétrico de 12px. */
    .control-left > .model-pill { padding: 0 var(--space-2); }
    .pill-duo > .model-pill:first-child {
      padding-left: var(--space-2);
      padding-right: var(--space-1);
    }
    .pill-duo > .model-pill + .model-pill { padding-left: var(--space-1); }
    /* Haiku não tem esforço -> o duo fica com UMA pill; o padding-right de emenda não vale. */
    .pill-duo > .model-pill:only-child { padding-right: var(--space-2); }
    .pill-duo > .model-pill + .model-pill .pill-model {
      color: var(--text-muted);                  /* "Alto" em cinza, como no app do Claude */
      font-weight: 500;
    }
    /* O esforço é palavra curta ("high") — encolher ele vira "hi…", pior que nada. */
    .pill-duo > .model-pill + .model-pill { flex-shrink: 0; }
    /* Ícone de 20px não precisa de 44px de largura (a ALTURA da fileira segue 44 = alvo do
       dedo). É daqui que sai o espaço que faltava, em vez de cortar palavra com reticências.
       `.control-left >` não é enfeite: a regra base de .attach-btn vem DEPOIS neste arquivo e,
       em especificidade igual, venceria — o seletor composto desempata. */
    .control-left > .attach-btn { width: 32px; min-width: 0; }  /* min-width fura o alvo global de 44px */
    /* Anel de contexto de 26px vira 20 dentro do chip (o viewBox escala o desenho inteiro). */
    .pill-duo .model-pill :global(svg) { width: 20px; height: 20px; }
    /* Última defesa numa tela muito estreita: as pills de texto encolhem com reticências
       (pill-model já tem ellipsis) em vez de invadir o send-btn. */
    .control-left > .model-pill { flex-shrink: 1; min-width: 0; }
  }

  .pill-label {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
    min-width: 0;
  }

  .pill-model {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 130px;
    color: var(--text-primary);
    font-weight: 600;
  }

  .pill-effort {
    flex-shrink: 0;
    color: var(--text-muted);
  }

  /* Botao [ / ]: abre o CommandSheet. Chip compacto na faixa do topo, igual ao cost-chip.
     min-height/min-width:0 sobrescrevem o alvo global de 44px pra manter o chip enxuto
     (tap confortavel dentro da faixa). */
  .slash-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 28px;
    min-height: 0;
    min-width: 0;
    padding: 0 var(--space-2);
    flex-shrink: 0;
    border-radius: var(--radius-md);
    /* `--surface-raised`, nao `--bg-hover` cru: o chip fica DENTRO do composer, que e vidro sobre a
       foto — com cor opaca ele virava um retangulo chapado por cima do vidro e ignorava o slider
       Solidez (CLAUDE.md, "Transparencia"). O realce de :active abaixo segue em `--bg-*` cru, que e
       tinta de estado por cima, nao superficie. */
    background: var(--surface-raised);
    color: var(--text-secondary);
  }

  .slash-btn:active {
    background: var(--bg-elevated);
  }

  .slash-glyph {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    font-weight: 600;
    line-height: 1;
  }

  .control-right {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  .stop-btn {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    background: var(--bg-elevated);
    border-radius: var(--radius-md);
    color: var(--error);
    transition: background 180ms var(--ease-out);
  }

  .stop-btn:active {
    background: var(--bg-hover);
  }

  /* Linha fina no topo do card: slash a esquerda, custo a direita (fora do control-row). */
  /* Prazo do cache: mono e discreto, no tom do resto da faixa. So muda de cor no fim do prazo —
     e informacao de fundo, nao alarme. */
  .cache-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-left: auto;
    flex-shrink: 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-muted);
    white-space: nowrap;
  }
  .cache-glyph {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--success);
  }
  .cache-chip.acabando { color: var(--warning); }
  .cache-chip.acabando .cache-glyph { background: var(--warning); }
  .cache-chip.frio .cache-glyph { background: var(--text-muted); opacity: 0.5; }

  .composer-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .top-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  /* Pareada: chip acende no accent (o 🤝 sem par fica na cor muted padrão do repo-chip). */
  .pair-chip--on { color: var(--accent); }
  .pair-chip--on .repo-name { color: var(--accent); }
  .pair-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }
  /* Toggle "pros dois": aceso = broadcast pro par ativo. */
  .both-chip--on { color: var(--accent); }
  .both-chip--on .repo-name { color: var(--accent); }

  .repo-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    appearance: none;
    border: 0;
    background: transparent;
    padding: 0;
    cursor: pointer;
  }
  /* O icone da pasta virou componente, entao nao herda mais o flex-shrink do .repo-glyph: sem
     isto, nome de repo longo espremia o SVG em vez de truncar o texto. */
  .repo-chip :global(svg) { flex-shrink: 0; }
  .repo-glyph { font-size: 11px; flex-shrink: 0; }
  .repo-name {
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 120px;
  }
  .repo-sep { color: var(--text-muted); }
  .repo-branch {
    font-family: var(--font-mono);
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 110px;
  }
  .repo-dirty { color: var(--warning); margin-left: 1px; }


  .send-btn {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    transition: background 180ms var(--ease-out);
  }

  .send-btn:active:not(:disabled) {
    background: var(--accent-press);
  }

  .send-btn--disabled {
    background: var(--bg-hover);
    color: var(--text-muted);
    cursor: default;
  }

  /* Chip da FILA do Kimi. Accent (e nao o cinza dos outros chips da fileira) porque ele nao e
     informacao de contexto como o repo: e uma acao disponivel AGORA, e some sozinho quando a fila
     esvazia. Segue a mesma caixa `.repo-chip` — a fileira e uma linha so de chips, nao um lugar
     onde cada assunto inventa um formato. */
  .fila-chip {
    color: var(--accent);
    border-color: var(--accent);
  }
  .fila-chip .repo-name { color: var(--accent); font-weight: 600; }
  .fila-chip .fila-acao { color: var(--accent); text-decoration: underline; }
  .fila-chip:active { background: var(--accent-dim); }

  /* ── Anexo de imagem ────────────────────────────────────────────────────── */
  .file-input { display: none; }

  .attach-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }
  .attach-chip {
    position: relative;
    flex-shrink: 0;
  }
  .attach-thumb {
    width: 48px;
    height: 48px;
    border-radius: var(--radius-sm);
    object-fit: cover;
    display: block;
  }
  .attach-file {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 140px;
    height: 48px;
    padding: 0 var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
  }
  .attach-file-glyph { flex-shrink: 0; font-size: 13px; }
  .attach-file-name {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .attach-status { font-size: var(--text-xs); color: var(--text-muted); }
  .attach-error { font-size: var(--text-xs); color: var(--error); }
  .attach-remove {
    position: absolute;
    top: -6px;
    right: -6px;
    width: 20px;
    height: 20px;
    min-height: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--radius-full);
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    color: var(--text-secondary);
    font-size: 14px;
    line-height: 1;
  }

  .attach-btn {
    width: 44px; height: 44px; flex-shrink: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text-secondary);
  }
  .attach-btn:active { background: var(--bg-hover); }
  .attach-btn :global(svg) { display: block; }


  /* Botao de gravar audio: ícone de mic (IconMic) / quadrado stop (IconInterrupt) enquanto grava.
     Gravando -> vermelho e pulsa. */
  .mic-btn--recording { color: var(--error); animation: mic-pulse 1.2s var(--ease-out) infinite; }
  @keyframes mic-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.45; }
  }

  /* Feedback de gravação: bolinha vermelha piscando + timer + waveform de voz. */
  .rec-hint {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0 var(--space-1);
  }
  .rec-dot {
    width: 8px;
    height: 8px;
    flex-shrink: 0;
    border-radius: var(--radius-full);
    background: var(--error);
    animation: mic-pulse 1.2s var(--ease-out) infinite;
  }
  /* "preparando microfone…": ambar em vez do vermelho de gravando — espera, ainda nao e hora de
     falar. Mesmo pulso, so a cor muda (o vermelho ja significa "no ar" nos outros dois hints). */
  .rec-dot--waiting { background: var(--warning, #e0a030); }
  .rec-time {
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
    flex-shrink: 0;
    min-width: 30px;
  }
  /* Waveform: fileira de barras cuja altura reflete o nível de voz (--h de 0 a 1). O array desliza
     -> a transição suaviza o passo entre frames, dando o movimento de "andar". */
  .rec-wave {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 64px;
    flex: 1;
    min-width: 0;
    overflow: hidden;   /* barras de largura fixa entram da esquerda; nunca vazam a faixa */
  }
  .rec-bar {
    flex: 0 0 3px;   /* largura fixa 3px + gap 2px = 5px/barra; barCount = largura/5 preenche exato */
    border-radius: 3px;
    background: var(--accent);
    height: calc(8px + var(--h, 0) * 56px);
    transition: height 60ms linear;
  }

  /* Botao "↩ original" do desfazer da limpeza do ditado: texto discreto, sem fundo proprio
     (superficie e a do rec-hint/composer, ver regra de transparencia do CLAUDE.md). */
  .undo-btn {
    flex-shrink: 0;
    padding: 0;
    background: transparent;
    color: var(--accent);
    font-size: var(--text-xs);
    font-weight: 600;
  }

  .send-error {
    font-size: var(--text-xs);
    color: var(--error);
    padding: 0 var(--space-1);
  }
</style>
