// Player de TTS: UM HTMLAudioElement pra aplicacao inteira.
//
// Por que singleton, e nao um <audio> por bolha: no WebKit o `play()` so e aceito dentro do gesto do
// usuario, e o gesto EXPIRA quando a pilha JS desenrola. Como sintetizar leva segundos, um
// `new Audio()` criado depois do fetch nasce travado e o play() volta NotAllowedError — sem som e
// sem erro visivel. Com um elemento unico, o toque destrava ele NA HORA (unlock, com um WAV
// silencioso inline) e o src real entra depois, no mesmo elemento ja liberado.
//
// .svelte.ts porque usa runes fora de componente — mesmo padrao do sessionsStore.

// WAV silencioso de 46 bytes (1 frame, 16-bit mono 44.1k). Serve so pra destravar o elemento.
const SILENCE =
  'data:audio/wav;base64,UklGRiYAAABXQVZFZm10IBAAAAABAAEARKwAAIhYAQACABAAZGF0YQIAAAAAAA==';

let el: HTMLAudioElement | null = null;

let active = $state(false);      // a barra esta na tela
let playing = $state(false);
let loading = $state(false);     // sintetizando: barra aparece antes do som existir
let error = $state('');
let label = $state('');          // trecho do texto, pra pessoa saber o que esta tocando
let current = $state(0);
let duration = $state(0);
let rate = $state(1);

function element(): HTMLAudioElement {
  if (el) return el;
  const a = new Audio();
  a.preload = 'auto';
  a.addEventListener('timeupdate', () => { current = a.currentTime; });
  a.addEventListener('durationchange', () => { duration = a.duration; });
  a.addEventListener('play', () => { playing = true; });
  a.addEventListener('pause', () => { playing = false; });
  a.addEventListener('ended', () => { playing = false; current = 0; });
  // Falha de rede/decodificacao no proprio elemento nao pode ficar muda: a barra some do estado
  // "tocando" e mostra o motivo.
  a.addEventListener('error', () => {
    loading = false; playing = false;
    error = 'não consegui tocar o áudio gerado';
  });
  el = a;
  return a;
}

export const ttsPlayer = {
  get active() { return active; },
  get playing() { return playing; },
  get loading() { return loading; },
  get error() { return error; },
  get label() { return label; },
  get current() { return current; },
  get duration() { return duration; },
  get rate() { return rate; },

  /** Chamar SINCRONO dentro do handler do toque, antes de qualquer await. */
  unlock(texto: string) {
    const a = element();
    active = true; loading = true; error = ''; playing = false;
    current = 0; duration = 0;
    label = texto.length > 60 ? texto.slice(0, 60) + '…' : texto;
    a.src = SILENCE;
    // O catch e obrigatorio: navegador que recusa mesmo assim rejeita a promessa, e rejeicao sem
    // catch vira "unhandled promise rejection" no console do usuario.
    a.play().catch(() => {});
  },

  playUrl(url: string) {
    const a = element();
    loading = false; error = '';
    a.src = url;
    a.playbackRate = rate;
    a.play().catch(() => {
      // Chegou aqui = o unlock nao segurou (ex: aba em segundo plano). Diz o que fazer.
      error = 'toque em ▶ para tocar';
    });
  },

  fail(msg: string) { loading = false; playing = false; error = msg; },

  toggle() {
    const a = element();
    if (a.paused) a.play().catch(() => { error = 'não consegui tocar'; });
    else a.pause();
  },

  seek(seconds: number) { element().currentTime = seconds; },

  setRate(r: number) { rate = r; element().playbackRate = r; },

  close() {
    const a = element();
    a.pause();
    a.removeAttribute('src');
    a.load();          // solta o buffer; sem isso o mp3 anterior fica na memoria
    active = false; playing = false; loading = false; error = '';
    current = 0; duration = 0; label = '';
  },
};
