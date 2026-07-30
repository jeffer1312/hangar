<script lang="ts">
  import type { StatusFields } from '../lib/statusline';

  interface Props {
    status: StatusFields | null;
    // Optional: com `limited` sem status (banner sem statusline custom), o NavBar pode nao ter
    // uma tela de detalhe pra abrir -- o clique so-existe se o caller passar um handler.
    onExpand?: () => void;
    // Feature #8 (rate-limit radar): banner de limite de uso detectado no pane (independente da
    // statusline custom do usuario -- pode existir mesmo sem status). limitReset = horario cru do
    // reset ("3pm"/"15:30"), ou null/undefined se nao deu pra ler.
    limited?: boolean;
    limitReset?: string | null;
    // Duas leituras do MESMO dado. 'dial' e o mostrador de 48px que a NavBar precisa (largura e o
    // recurso escasso la). 'bars' e pro painel do desktop, que tem 248px de coluna e ja mostra o
    // contexto em barra logo acima: la o anel comprimia 5h e 7d em dois arcos de ~40px que so se
    // leem de perto, enquanto sobra largura pra duas barras rotuladas.
    variant?: 'dial' | 'bars';
  }
  let { status, onExpand, limited = false, limitReset = null, variant = 'dial' }: Props = $props();

  // ── Mostrador duplo ────────────────────────────────────────────────────────
  // Dois aneis concentricos + os dois numeros empilhados no miolo. Antes eram duas pilulas com
  // emoji (⚡37% 📅7%) que custavam 106px da navbar e deixavam o nome da sessao em "clau…"; o
  // mostrador custa 48px. O buraco do meio e onde sobra espaco pro texto: 11px e 9px, contra os
  // 7px que sobrariam no vao ENTRE os aneis.
  //
  // Quem fica em qual anel nao e arbitrario: a janela de 5h vai por FORA porque e a que anda (enche
  // na sessao, zera no reset) e a circunferencia maior faz o mesmo % virar um arco bem mais longo.
  // A de 7 dias vai por dentro, mais fina e apagada -- e informacao de ambiente.
  const R_5H = 20;
  const R_7D = 13.5;
  const C_5H = 2 * Math.PI * R_5H;
  const C_7D = 2 * Math.PI * R_7D;

  // Mesmos limiares do ContextRing (70 / 90) — um vocabulario so de medidor no app inteiro.
  function tone(pct: number | undefined): 'ok' | 'warn' | 'hot' {
    if (typeof pct !== 'number' || !isFinite(pct)) return 'ok';
    if (pct >= 90) return 'hot';
    if (pct >= 70) return 'warn';
    return 'ok';
  }
  function known(pct: number | undefined): boolean {
    return typeof pct === 'number' && isFinite(pct);
  }
  function clamp(pct: number | undefined): number {
    return known(pct) ? Math.min(100, Math.max(0, pct as number)) : 0;
  }
  // stroke-dashoffset: quanto FALTA pra fechar a volta.
  function offset(pct: number | undefined, circumference: number): number {
    return circumference * (1 - clamp(pct) / 100);
  }
  function label(pct: number | undefined): string {
    return known(pct) ? String(Math.round(clamp(pct))) : '—';
  }

  const five = $derived(status?.fiveHourPct);
  const week = $derived(status?.weeklyPct);
  const month = $derived(status?.monthlyPct);
  const hasDial = $derived(known(five) || known(week));
  // O painel de barras aceita a 3ª janela (30d) mesmo quando as outras duas faltam; o anel duplo,
  // não — fica restrito a 5h/7d por desenho.
  const hasBars = $derived(hasDial || known(month));

  // 100% e o unico valor de 3 digitos: com o rotulo na frente o par estoura os 24,5px uteis do
  // miolo (medido: 26,7px), entao numero e rotulo encolhem juntos SO nesse caso.
  const wide5 = $derived(Math.round(clamp(five)) >= 100);
  const wide7 = $derived(Math.round(clamp(week)) >= 100);

  // Texto pro leitor de tela: o desenho e aria-hidden, entao o rotulo carrega os numeros.
  const a11y = $derived(
    `Uso: 5 horas ${known(five) ? Math.round(clamp(five)) + '%' : 'sem dado'}` +
    `, 7 dias ${known(week) ? Math.round(clamp(week)) + '%' : 'sem dado'}` +
    (known(month) ? `, 30 dias ${Math.round(clamp(month))}%` : ''),
  );
</script>

{#if hasBars || limited}
  <div class="rate-chips" class:as-bars={variant === 'bars'}>
    {#if hasBars && variant === 'bars'}
      <!-- Uma barra por janela, no mesmo desenho da barra de Contexto do painel (rotulo + valor
           em cima, trilho de 4px embaixo). Janela sem dado nao vira barra vazia: some. -->
      <button class="bars" onclick={onExpand} aria-label={a11y} title={a11y}>
        {#if known(five)}
          <span class="bar-row tone-{tone(five)}">
            <span class="bar-head"><span class="bar-cap">5 horas</span><span class="bar-pct">{label(five)}%</span></span>
            <span class="bar"><span style:width={`${clamp(five)}%`}></span></span>
            <!-- Quando zera: a statusline crua ja traz ("↺1h20m", "↺sab 18h·4d7h") e o painel tem
                 largura pra isso — sem ele a barra diz o quanto foi, nunca ate quando dura. -->
            {#if status?.fiveHourReset}<span class="bar-reset">reseta {status.fiveHourReset}</span>{/if}
          </span>
        {/if}
        {#if known(week)}
          <span class="bar-row tone-{tone(week)}">
            <span class="bar-head"><span class="bar-cap">7 dias</span><span class="bar-pct">{label(week)}%</span></span>
            <span class="bar"><span style:width={`${clamp(week)}%`}></span></span>
            {#if status?.weeklyReset}<span class="bar-reset">reseta {status.weeklyReset}</span>{/if}
          </span>
        {/if}
        {#if known(month)}
          <span class="bar-row tone-{tone(month)}">
            <span class="bar-head"><span class="bar-cap">30 dias</span><span class="bar-pct">{label(month)}%</span></span>
            <span class="bar"><span style:width={`${clamp(month)}%`}></span></span>
            {#if status?.monthlyReset}<span class="bar-reset">reseta {status.monthlyReset}</span>{/if}
          </span>
        {/if}
      </button>
    {:else if hasDial}
      <button class="dial tone-5-{tone(five)} tone-7-{tone(week)}" onclick={onExpand} aria-label={a11y} title={a11y}>
        <svg width="44" height="44" viewBox="0 0 44 44" aria-hidden="true">
          <!-- Fora: janela de 5 horas -->
          <circle cx="22" cy="22" r={R_5H} class="track track-5h" />
          {#if known(five)}
            <circle
              cx="22" cy="22" r={R_5H}
              class="arc arc-5h"
              stroke-dasharray={C_5H}
              stroke-dashoffset={offset(five, C_5H)}
              transform="rotate(-90 22 22)"
            />
          {/if}
          <!-- Dentro: janela de 7 dias -->
          <circle cx="22" cy="22" r={R_7D} class="track track-7d" />
          {#if known(week)}
            <circle
              cx="22" cy="22" r={R_7D}
              class="arc arc-7d"
              stroke-dasharray={C_7D}
              stroke-dashoffset={offset(week, C_7D)}
              transform="rotate(-90 22 22)"
            />
          {/if}
          <!-- Numeros empilhados na MESMA ordem dos aneis (de fora pra dentro = de cima pra baixo),
               cada um com o rotulo da janela num corpo bem menor: sem ele "4" em cima e "9"
               embaixo nao dizem quem e quem. O rotulo leva a LETRA ("5h", nao "5") de proposito:
               so o digito colava no valor e virava outro numero — 5h em 4% renderizava "54", que
               se le como 54%. A letra quebra a sequencia de digitos. Sem "%": o anel ja diz que e
               percentual, e o buraco do meio tem 24,5px de largura pra gastar. -->
          <text x="22" y="19" class="num num-5h" class:wide={wide5} text-anchor="middle" dominant-baseline="middle"
          ><tspan class="cap">5h</tspan><tspan dx={wide5 ? 1 : 1.5}>{label(five)}</tspan></text>
          <text x="22" y="29" class="num num-7d" class:wide={wide7} text-anchor="middle" dominant-baseline="middle"
          ><tspan class="cap">7d</tspan><tspan dx={wide7 ? 1 : 1.5}>{label(week)}</tspan></text>
        </svg>
      </button>
    {:else if known(month)}
      <!-- Sessao clinepass no celular: so tem a janela de 30 dias, e o anel duplo e 5h/7d por
           desenho. Sem este ramo a barra abriria vazia e o toque que abre a tela de uso sumia. -->
      <button class="rchip tone-chip-{tone(month)}" onclick={onExpand} aria-label={a11y} title={a11y}>
        <span aria-hidden="true">🗓</span>{label(month)}%
      </button>
    {/if}
    {#if limited}
      <!-- Banner de limite detectado no pane (feature #8), independente da statusline custom.
           Calmo (mesma cor "warm" do resto), nao alarmante. -->
      <button class="rchip warm" onclick={onExpand} aria-label="Limite de uso atingido">
        <span aria-hidden="true">⏳</span>{limitReset ? `volta ${limitReset}` : 'limitado'}
      </button>
    {/if}
  </div>
{/if}

<style>
  .rate-chips {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    flex-shrink: 0;
  }
  /* No painel a secao e uma coluna: as barras ocupam a largura toda e o banner de limite cai
     embaixo, nao ao lado. */
  .rate-chips.as-bars {
    display: block;
    width: 100%;
  }

  .bars {
    display: flex;
    flex-direction: column;
    /* O `button` global e inline-flex com align-items/justify-content center: sem sobrescrever,
       cada linha encolhe pro tamanho do texto e a barra fica com meia largura, centralizada. */
    align-items: stretch;
    justify-content: flex-start;
    gap: var(--space-2);
    width: 100%;
    min-width: 0;
    min-height: 0;
    padding: 0;
    text-align: left;
    -webkit-tap-highlight-color: transparent;
  }
  .bar-row { display: block; width: 100%; }
  .bar-head {
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
  }
  /* Qualificador calmo a esquerda, leitura forte a direita — a coluna de numeros alinha com a do
     Contexto logo acima, entao o olho desce so um eixo. */
  .bar-cap { color: var(--text-muted); }
  .bar-pct { color: var(--text-primary); font-weight: 650; }
  .bar {
    display: block;
    height: 4px;
    margin-top: 6px;
    overflow: hidden;
    border-radius: var(--radius-full);
    background: var(--bg-elevated);
  }
  .bar > span {
    display: block;
    height: 100%;
    min-width: 2px;   /* 1% ainda deixa marca, mesmo papel do stroke-linecap do anel */
    border-radius: inherit;
    background: var(--accent);
    transition: width 600ms var(--ease-out), background 300ms ease;
  }
  .bar-reset {
    display: block;
    margin-top: 3px;
    overflow: hidden;
    color: var(--text-muted);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Mesmos limiares do anel (70/90), cada janela com a propria rampa. */
  .bar-row.tone-warn .bar > span { background: var(--warning); }
  .bar-row.tone-warn .bar-pct { color: var(--warning); }
  .bar-row.tone-hot .bar > span { background: var(--error); }
  .bar-row.tone-hot .bar-pct { color: var(--error); }
  .bars + .rchip { margin-top: var(--space-2); }

  /* Alvo de toque de 44px sem inflar o desenho (que tem 44 de altura no proprio svg). */
  .dial {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    min-width: 48px;
    height: 44px;
    min-height: 44px;
    padding: 0;
    flex-shrink: 0;
    border-radius: var(--radius-sm);
    -webkit-tap-highlight-color: transparent;
  }
  .dial:active { background: var(--bg-hover); }
  .dial svg { display: block; }

  .track { fill: none; stroke: var(--border-default); }
  .track-5h { stroke-width: 3.5; }
  .track-7d { stroke-width: 2.5; }

  .arc {
    fill: none;
    stroke-linecap: round;   /* garante uma marca visivel mesmo em 1-2% */
    transition: stroke-dashoffset 600ms var(--ease-out), stroke 300ms ease;
  }
  /* Repouso = neutro. O indigo (--accent) fica reservado pro que e clicavel; um medidor calmo nao
     precisa gritar. Quem sobe de tom e so o anel que passou do limiar. */
  .arc-5h { stroke-width: 3.5; stroke: var(--text-secondary); }
  .arc-7d { stroke-width: 2.5; stroke: var(--text-muted); opacity: 0.8; }

  .num {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    /* Mesma duracao do `stroke` do arco: numero e anel cruzam o limiar juntos, nao em tempos
       diferentes (um saltando e o outro derretendo). */
    transition: fill 300ms ease;
  }
  .num-5h { font-size: 11px; font-weight: 700; fill: var(--text-primary); }
  .num-7d { font-size: 9px; font-weight: 600; fill: var(--text-muted); }
  /* 100%: numero E rotulo encolhem, senao o par estoura os 24,5px do miolo (medido: 26,7px). */
  .num-5h.wide { font-size: 8.5px; }
  .num-7d.wide { font-size: 7.5px; }
  .wide .cap { font-size: 5.5px; }
  /* Rotulo da janela: o menor corpo do desenho, so pra desambiguar. Nao acompanha a cor do alerta
     (quem grita e o numero e o arco) — se acompanhasse, viraria mais ruido vermelho. */
  .cap {
    font-size: 6.5px;
    font-weight: 700;
    fill: var(--text-muted);
    opacity: 0.85;
  }

  /* Cada anel tem a propria rampa: o 5h pode estar no vermelho com o 7d tranquilo, e vice-versa.
     So o arco que apertou muda de cor — junto com o numero dele. */
  .tone-5-warn .arc-5h { stroke: var(--warning); }
  .tone-5-warn .num-5h { fill: var(--warning); }
  .tone-5-hot .arc-5h { stroke: var(--error); }
  .tone-5-hot .num-5h { fill: var(--error); }
  .tone-7-warn .arc-7d { stroke: var(--warning); opacity: 1; }
  .tone-7-warn .num-7d { fill: var(--warning); }
  .tone-7-hot .arc-7d { stroke: var(--error); opacity: 1; }
  .tone-7-hot .num-7d { fill: var(--error); }

  .rchip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 28px;
    min-height: 0;
    min-width: 0;
    padding: 0 var(--space-2);
    background: var(--bg-hover);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
    white-space: nowrap;
  }
  .rchip.warm { color: var(--warning); }
  /* Mesmos limiares do anel e das barras — 70 ambar, 90 vermelho. */
  .rchip.tone-chip-warn { color: var(--warning); }
  .rchip.tone-chip-hot { color: var(--error); }
</style>
