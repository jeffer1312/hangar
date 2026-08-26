<script lang="ts">
  import { tick } from 'svelte';
  import BottomSheet from './BottomSheet.svelte';
  import * as m from '../paraglide/messages';
  import { listUploads, uploadUrl } from '../lib/api';
  import { fileKind, fmtBytes, relativeTime } from '../lib/format';
  import { abrirVisor, type MidiaVisor } from '../lib/visor';
  import type { UploadFile } from '../lib/types';

  // Galeria dos anexos JÁ enviados pra esta sessão. Até aqui, rever uma foto mandada do celular
  // significava rolar o chat inteiro atrás dela — e agora os anexos EXPIRAM (retenção configurável),
  // então o usuário precisa de um lugar onde dê pra ver o que ainda existe e por quanto tempo.
  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
    // Manda o audio de volta pro ditado (transcreve de novo e abre a barra de versoes no composer).
    // Quem busca o arquivo e o Chat, que e quem fala com o Composer — aqui so sai o anexo escolhido.
    onUsarNoDitado?: (f: UploadFile) => void;
  }
  let { open, sessionName, onClose, onUsarNoDitado }: Props = $props();

  let files = $state<UploadFile[]>([]);
  let loading = $state(false);
  let erro = $state<string | null>(null);
  // Toca no proprio card: nome do arquivo em reproducao + quanto ja andou (0..1). UM player pra
  // sheet inteira, entao dar play num audio para o anterior sozinho — dois ditados ao mesmo tempo
  // nao e uma coisa que alguem queira ouvir.
  let tocando = $state<string | null>(null);
  let progresso = $state(0);
  let player = $state<HTMLAudioElement | undefined>();
  // filename -> o .webm nao tem faixa de video (descoberto no loadedmetadata do thumb).
  let semVideo = $state<Record<string, boolean>>({});
  // filename -> o botao do card. O visor le dali o tamanho natural da midia e a origem da animacao.
  const tiles: Record<string, HTMLElement | undefined> = {};

  // "Da pra tratar como audio?" — kind audio, ou video cujo metadata disse que nao tem imagem.
  // Enquanto o metadata nao chegou, um .webm conta como video: o card ja mostra o play do mesmo
  // jeito, e errar pro lado do visor e reversivel (fecha), errar pro outro tocaria video sem imagem.
  const ehAudio = (f: UploadFile) => fileKind(f.filename) === 'audio' || !!semVideo[f.filename];

  // Midias que o VISOR navega: imagem e video, na ordem da lista. Audio fica de fora — ele toca na
  // propria linha, e tela cheia pra um ditado nao serve pra nada.
  function visoraveis(): MidiaVisor[] {
    return files
      .filter((f) => fileKind(f.filename) === 'image' || (fileKind(f.filename) === 'video' && !semVideo[f.filename]))
      .map((f) => ({
        url: url(f),
        nome: f.filename,
        tipo: fileKind(f.filename) === 'image' ? 'image' : 'video',
        meta: `${fmtBytes(f.size)} · ${relativeTime(f.mtime)} · ${prazo(f.expires_in_days).txt}`,
        // O card da grade: e dele que sai o tamanho natural da midia (e a animacao de abrir).
        element: tiles[f.filename],
      }));
  }

  function abrir(f: UploadFile) {
    const lista = visoraveis();
    const i = lista.findIndex((x) => x.nome === f.filename);
    void abrirVisor(lista, Math.max(0, i));
  }

  function tocarOuAbrir(f: UploadFile) {
    if (!ehAudio(f)) { abrir(f); return; }
    if (tocando === f.filename) { player?.pause(); tocando = null; return; }
    tocando = f.filename;
    progresso = 0;
    // O src so muda depois que o Svelte pinta o <audio> com a url nova; tocar antes disso pegaria
    // o arquivo anterior.
    tick().then(() => player?.play().catch(() => (tocando = null)));
  }

  // Recarrega A CADA abertura em vez de uma vez só: entre uma abertura e outra o usuário mandou
  // anexos novos (e o backend pode ter podado os vencidos no meio) — uma lista em cache mostraria
  // miniatura de arquivo que não existe mais.
  // Fechar a sheet PARA o audio: sem isto o ditado seguia tocando com a tela do chat na frente e
  // sem nenhum controle pra parar.
  $effect(() => {
    if (!open) { player?.pause(); tocando = null; progresso = 0; }
  });

  $effect(() => {
    if (!open) return;
    const sess = sessionName;
    let vivo = true;
    loading = true;
    erro = null;
    listUploads(sess)
      .then((r) => { if (vivo) files = r.files; })
      .catch((e) => { if (vivo) erro = e instanceof Error ? e.message : String(e); })
      .finally(() => { if (vivo) loading = false; });
    return () => { vivo = false; };
  });

  const url = (f: UploadFile) => uploadUrl(sessionName, f.filename);

  function icone(f: UploadFile): string {
    const k = fileKind(f.filename);
    return k === 'pdf' ? '📄' : k === 'html' ? '🌐' : k === 'audio' ? '🎵' : '📎';
  }

  // Prazo em texto curto. O backend pode mandar negativo (o prune só roda no próximo upload), e aí
  // o honesto é avisar que o arquivo está vencido, não arredondar pra "expira em 0 d".
  function prazo(d: number | null): { txt: string; urgente: boolean } {
    if (d === null) return { txt: m.anexos_sem_expiracao(), urgente: false };
    if (d <= 0) return { txt: m.anexos_vencido(), urgente: true };
    if (d < 1) return { txt: m.anexos_expira_h({ n: Math.max(1, Math.round(d * 24)) }), urgente: true };
    return { txt: m.anexos_expira_d({ n: Math.round(d) }), urgente: d <= 3 };
  }

</script>

<BottomSheet {open} {onClose} ariaLabel={m.ctx_anexos_da_sessao()}>
  <div class="atts">
    <h2 class="atts-title">
      {m.ctx_anexos()}
      {#if files.length}<span class="count">{files.length}</span>{/if}
    </h2>

    {#if loading}
      <p class="atts-msg">{m.comum_carregando()}</p>
    {:else if erro}
      <p class="atts-msg erro">{m.anexos_erro_listar()} {erro}</p>
    {:else if !files.length}
      <p class="atts-msg">{m.anexos_nenhum()}</p>
    {:else}
      <ul class="grid">
        {#each files as f (f.filename)}
          {@const kind = fileKind(f.filename)}
          {@const p = prazo(f.expires_in_days)}
          <!-- Audio nao ganha card quadrado: nao ha o que olhar num audio. Ele vira uma LINHA de
               largura inteira (play + nome + acao), e a grade de quadrados fica pra quem tem
               imagem — foto e video. -->
          <li class="item" class:linha={ehAudio(f)}>
            {#if kind === 'image'}
              <button class="tile" bind:this={tiles[f.filename]} onclick={() => abrir(f)} aria-label={m.anexos_ver({ n: f.filename })}>
                <img class="media" src={url(f)} alt={f.filename} loading="lazy" />
              </button>
            {:else if kind === 'video' || kind === 'audio'}
              <!-- Audio toca NO PROPRIO CARD: um ditado nao merece tela cheia — e um play e uma
                   barrinha. Video segue no visor (ali a tela cheia e o ponto). O `#t=0.1` (media
                   fragment) faz o browser, iOS incluso, buscar o 1o frame pro thumb; e ele tambem
                   e quem responde se aquele .webm tem faixa de video: o ditado do app e gravado em
                   .webm, que pela extensao e indistinguivel de video. -->
              <button class="tile" bind:this={tiles[f.filename]} onclick={() => tocarOuAbrir(f)}
                      aria-label={ehAudio(f) ? m.anexos_tocar({ nome: f.filename }) : m.anexos_ver({ n: f.filename })}>
                {#if kind === 'video'}
                  <video class="media" class:oculto={semVideo[f.filename]} src={url(f) + '#t=0.1'}
                         preload="metadata" muted playsinline
                         onloadedmetadata={(e) => (semVideo[f.filename] = e.currentTarget.videoHeight === 0)}></video>
                {/if}
                <span class="play" class:sobre-video={kind === 'video' && !semVideo[f.filename]} aria-hidden="true"
                  >{tocando === f.filename ? '⏸' : '▶'}</span>
              </button>
              {#if tocando === f.filename}
                <!-- Irma do botao, nao filha: a barra atravessa a LINHA inteira, e dentro do botao
                     de 36px ela ficaria do tamanho do proprio play. -->
                <span class="prog" aria-hidden="true"><span class="prog-fill" style="width: {progresso * 100}%"></span></span>
              {/if}
            {:else}
              <a class="tile chip" href={url(f)} target="_blank" rel="noopener noreferrer" aria-label={m.compare_abrir({ n: f.filename })}>
                <span class="chip-ico" aria-hidden="true">{icone(f)}</span>
              </a>
            {/if}
            <span class="txt">
              <span class="nome" title={f.filename}>{f.filename}</span>
              <span class="meta">{fmtBytes(f.size)} · {relativeTime(f.mtime)}</span>
              <span class="prazo" class:urgente={p.urgente}>{p.txt}</span>
            </span>
            {#if onUsarNoDitado && ehAudio(f)}
              <button class="usar" onclick={() => { onUsarNoDitado(f); onClose(); }}>{m.anexos_usar_no_ditado()}</button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if tocando}
      {@const atual = files.find((f) => f.filename === tocando)}
      {#if atual}
        <!-- svelte-ignore a11y_media_has_caption -->
        <audio
          bind:this={player}
          src={url(atual)}
          onended={() => { tocando = null; progresso = 0; }}
          ontimeupdate={(e) => {
            const el = e.currentTarget;
            progresso = el.duration ? el.currentTime / el.duration : 0;
          }}
        ></audio>
      {/if}
    {/if}
  </div>
</BottomSheet>


<style>
  .atts { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; }
  .atts-title {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin: 0;
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
  }
  .count {
    font-size: 11px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    color: var(--text-secondary);
    background: var(--surface-raised);
  }
  .atts-msg {
    font-size: var(--text-sm);
    color: var(--text-muted);
    padding: var(--space-4) 0;
    text-align: center;
  }
  .atts-msg.erro { color: var(--error); }

  /* auto-fill: no celular cabem 3 colunas, no dock desktop (mais largo) enche sozinho — sem media
     query pra manter em duas versoes. */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
    gap: var(--space-3);
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .item { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  /* Linha de audio: ocupa a largura toda da grade e troca o quadrado por um botao redondo. */
  .item.linha {
    grid-column: 1 / -1;
    position: relative;
    flex-direction: row;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2);
    border-radius: var(--radius-md);
    background: var(--surface-raised);
    overflow: hidden;
  }
  .item.linha .tile {
    flex: 0 0 auto;
    width: 36px;
    height: 36px;
    aspect-ratio: auto;
    border-radius: var(--radius-full);
    background: color-mix(in srgb, var(--accent) 18%, transparent);
  }
  .item.linha .txt { flex: 1 1 auto; }
  .item.linha .usar { margin-top: 0; flex: 0 0 auto; }
  /* Andamento na base da LINHA inteira (dentro do botao de 36px seria um traco de nada). */
  .item.linha .prog { position: absolute; left: 0; right: 0; bottom: 0; }
  .tile {
    position: relative;
    display: block;
    aspect-ratio: 1;
    width: 100%;
    padding: 0;
    border: none;
    background: var(--surface-raised);
    border-radius: var(--radius-md);
    overflow: hidden;
    line-height: 0;
    -webkit-tap-highlight-color: transparent;
  }
  .media { width: 100%; height: 100%; object-fit: cover; display: block; }
  .play {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: var(--text-lg);
    line-height: 1;
    color: #fff;
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.6);
  }
  /* Play do card: grande e centrado quando nao ha imagem atras (audio), disco escuro por cima do
     frame quando ha (video). */
  .play.sobre-video { text-shadow: 0 1px 4px rgba(0, 0, 0, 0.6); }
  .play:not(.sobre-video) { font-size: 30px; color: var(--accent); text-shadow: none; }
  .oculto { visibility: hidden; }
  /* Andamento: faixa fina colada no pe do item, no lugar de uma barra de player inteira. */
  .prog {
    position: absolute;
    left: 0; right: 0; bottom: 0;
    height: 3px;
    background: var(--bg-base);
  }
  .item { position: relative; }
  .prog-fill { display: block; height: 100%; background: var(--accent); }
  .usar {
    margin-top: 2px;
    padding: 4px 8px;
    border: none;
    border-radius: var(--radius-full);
    background: var(--surface-raised);
    color: var(--text-secondary);
    font-size: 11px;
    cursor: pointer;
  }
  .usar:hover { color: var(--text-primary); }
  .chip { display: flex; align-items: center; justify-content: center; }
  .chip-ico { font-size: 28px; line-height: 1; }
  .nome {
    font-size: var(--text-xs);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta { font-size: 11px; color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .prazo { font-size: 11px; color: var(--text-muted); }
  .prazo.urgente { color: var(--warning); }

</style>
