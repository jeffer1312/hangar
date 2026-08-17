<script lang="ts">
  // Aba Contas (Configurações › Contas do Claude neste servidor). Fonte única do estado das
  // contas: backend/app/conta_estado.py (GET /api/conta-estado). Esta Task desenha a lista com
  // login, plano e a idade do último limite; a Task 7 liga o botão Entrar e a Task 9 desenha a
  // faixa de cota na coluna do limite (a partir de `limite.linha`, que o lib/statusline.ts já
  // sabe parsear).
  import { criarConta, apagarConta } from '../../lib/api';
  import { listarEstadosDeConta, formatarIntervalo, type ContaEstado } from '../../lib/contaEstado';
  import { initials } from '../../lib/format';
  import * as m from '../../paraglide/messages';

  let contas = $state<ContaEstado[]>([]);
  let carregando = $state(true);
  let erro = $state('');

  // Criar: campo inline no rodapé, mesmo fluxo do CreateSessionSheet.
  let pedindoNome = $state(false);
  let nomeConta = $state('');
  let criando = $state(false);
  // Apagar: kebab por linha abre o menu; confirmar apaga e recarrega.
  let menuDe = $state<string | null>(null);       // path da conta com o menu aberto
  let confirmando = $state<string | null>(null);  // path da conta com a confirmação aberta
  let apagando = $state(false);
  let aviso = $state('');
  let avisoErro = $state(false);

  async function carregar() {
    carregando = true;
    erro = '';
    try {
      contas = await listarEstadosDeConta();
    } catch (e) {
      erro = e instanceof Error && e.message ? e.message : String(e);
    } finally {
      carregando = false;
    }
  }
  carregar();

  const leituraFresca = (c: ContaEstado) =>
    c.limite.estado === 'lido' && c.limite.idade_s != null && c.limite.idade_s < 60;

  async function novaConta() {
    const nome = nomeConta.trim();
    if (!nome || criando) return;
    criando = true;
    aviso = '';
    avisoErro = false;
    try {
      await criarConta(nome);
      nomeConta = '';
      pedindoNome = false;
      aviso = m.criar_conta_deslogada();
      await carregar();
    } catch (e) {
      aviso = e instanceof Error && e.message ? e.message : m.criar_conta_erro();
      avisoErro = true;
    } finally {
      criando = false;
    }
  }

  async function apagar() {
    const path = confirmando;
    if (!path || apagando) return;
    // `confirmando` guarda o PATH (chave única da lista); o DELETE espera o NOME (label) —
    // a pasta é ~/.claude-<nome>. Derivar do estado atual evita mandar um nome velho se a
    // lista mudou entre o clique e o fim da operação.
    const conta = contas.find((x) => x.path === path);
    if (!conta) return;
    apagando = true;
    aviso = '';
    avisoErro = false;
    try {
      await apagarConta(conta.label);
      confirmando = null;
      menuDe = null;
      aviso = m.criar_conta_apagada({ nome: conta.label });
      // A conta pode ter sumido da lista entre o clique e o fim do DELETE (outro painel, outra
      // sessão) — recarregar é a fonte única, não remover item por item.
      await carregar();
    } catch (e) {
      aviso = e instanceof Error && e.message ? e.message : m.criar_apagar_conta_erro();
      avisoErro = true;
    } finally {
      apagando = false;
    }
  }
</script>

<div class="ct-superficie">
  <p class="st-secao ct-topo">{m.contas_secao_lista()}</p>
  <p class="ct-legenda">{m.contas_legenda()}</p>

  {#if carregando}
    <p class="ct-aviso">{m.comum_carregando()}</p>
  {:else if erro}
    <p class="ct-aviso erro" role="alert">{erro}</p>
  {:else if !contas.length}
    <p class="ct-aviso">{m.comum_nada_encontrado()}</p>
  {:else}
    <div class="ct-cartao">
      {#each contas as conta (conta.path)}
        <div class="ct-linha" class:fora={conta.login.estado === 'ok' && !conta.login.loggedIn}>
          <span class="ct-av" aria-hidden="true">{initials(conta.label)}</span>
          <span class="ct-txt">
            <span class="ct-nome-l">
              <span class="ct-nome">{conta.label}</span>
              {#if conta.active}<span class="ct-emuso">{m.contas_em_uso()}</span>{/if}
            </span>
            {#if conta.login.estado === 'ok' && conta.login.loggedIn && conta.login.email}
              <!-- Decisão do árbitro 17/08 (recorte): NÃO exibir a frase do plano. A chave
                   contas_email_plano tem "Max" literal + {max} que o auth status não entrega —
                   um mapa inventaria o multiplicador. Só o e-mail, que vem da fonte. Task de
                   ajuste serial troca a frase quando o Lote A mergear. -->
              <span class="ct-sub">{conta.login.email}</span>
            {:else if conta.login.estado === 'ok'}
              <span class="ct-sub fraco">{m.contas_nao_conectada()}</span>
            {/if}
            <span class="ct-dir">{conta.path}</span>
          </span>

          {#if conta.limite.estado === 'lido'}
            <!-- Coluna do limite: nesta Task só a idade; a Task 9 desenha a faixa de cota -->
            <!-- (barras 5h/7d) aqui dentro, a partir de conta.limite.linha. -->
            <span class="ct-cota" class:velha={!leituraFresca(conta)}>
              {#if leituraFresca(conta)}
                <span class="ct-idade">{m.cota_lido_agora()}</span>
              {:else}
                <span class="ct-idade">{m.cota_ultima_leitura({ n: formatarIntervalo(conta.limite.idade_s) })}</span>
              {/if}
            </span>
          {:else}
            <span class="ct-semleitura">{m.contas_sem_leitura()}</span>
          {/if}

          {#if conta.login.estado === 'ok' && !conta.login.loggedIn}
            <!-- Inerte de propósito nesta Task: o fluxo de Entrar é a Task 7, montada por cima
                 deste mesmo estado. Presente para a tela dizer o que uma conta deslogada é. -->
            <button type="button" class="ct-acao primaria"
              aria-label={m.contas_entrar_titulo({ nome: conta.label })}>{m.contas_entrar()}</button>
          {/if}

          <button type="button" class="ct-kebab" aria-haspopup="true" aria-expanded={menuDe === conta.path}
            aria-label={m.comum_fechar_menu_conta()} onclick={() => (menuDe = menuDe === conta.path ? null : conta.path)}>⋯</button>

          {#if menuDe === conta.path && confirmando !== conta.path}
            <div class="ct-menu">
              <button type="button" class="ct-menu-item"
                onclick={() => { menuDe = null; confirmando = conta.path; }}>{m.comum_apagar()}</button>
            </div>
          {/if}

          {#if confirmando === conta.path}
            <div class="ct-confirma">
              <span class="ct-confirma-txt">
                {m.comum_apagar()} <strong>{conta.label}</strong> {m.criar_apagar_fim()}
              </span>
              <button type="button" class="ct-confirma-btn perigo" onclick={apagar}
                disabled={apagando}>{apagando ? '…' : m.comum_apagar()}</button>
              <button type="button" class="ct-confirma-btn"
                onclick={() => (confirmando = null)} disabled={apagando}>{m.comum_cancelar()}</button>
            </div>
          {/if}
        </div>
      {/each}
    </div>

    <div class="ct-rodape">
      <button type="button" class="ct-btn" onclick={() => (pedindoNome = true)}
        disabled={pedindoNome}>{m.contas_nova()}</button>
      {#if pedindoNome}
        <!-- svelte-ignore a11y_autofocus -->
        <input class="ct-campo" type="text" autofocus bind:value={nomeConta}
          placeholder={m.criar_conta_placeholder()} aria-label={m.criar_conta_nova_aria()}
          disabled={criando}
          onkeydown={(e) => {
            if (e.key === 'Enter') { e.preventDefault(); novaConta(); }
            else if (e.key === 'Escape') { pedindoNome = false; nomeConta = ''; }
          }} />
        <button type="button" class="ct-btn" onclick={novaConta}
          disabled={criando || !nomeConta.trim()}>{criando ? '…' : m.comum_criar()}</button>
        <button type="button" class="ct-btn" onclick={() => { pedindoNome = false; nomeConta = ''; }}
          disabled={criando}>{m.comum_cancelar()}</button>
      {/if}
    </div>
  {/if}

  {#if aviso}
    <p class="ct-aviso" class:erro={avisoErro} role={avisoErro ? 'alert' : 'status'} aria-live="polite">{aviso}</p>
  {/if}

  <div class="ct-sep"></div>

  <p class="st-secao ct-topo">{m.contas_herda_titulo()}</p>
  <div class="ct-herda">
    <p>{m.contas_herda_igual()}</p>
    <p>{m.contas_herda_so({ arquivo: '.credentials.json', pasta: 'projects/' })}</p>
  </div>
</div>

<style>
  /* As classes repetem o mock (mocks/contas.html, estado 1) — mesmos tokens, mesmas medidas.
     A superfície inteira é um container de largura: no celular a folha é estreita e quem aperta
     a linha é a largura do PAINEL, não a da janela (régua: container query, não media query). */
  .ct-superficie { container-type: inline-size; }

  .ct-topo { margin-top: 0; }

  .ct-legenda { margin: 0 var(--space-2) var(--space-3); color: var(--text-muted);
                font-size: var(--text-xs); line-height: 1.45; }
  .ct-sep { height: 1px; background: var(--border-subtle); margin: var(--space-4) 0 var(--space-3); }
  .ct-aviso { margin: var(--space-2); color: var(--text-muted); font-size: var(--text-sm); }
  .ct-aviso.erro { color: var(--error); }

  .ct-cartao { background: var(--surface-card); border: 1px solid var(--border-subtle);
               border-radius: var(--radius-md); overflow: hidden; }
  .ct-linha { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-3);
              flex-wrap: wrap; position: relative; }
  .ct-linha + .ct-linha { border-top: 1px solid var(--border-subtle); }

  .ct-av { flex-shrink: 0; width: 34px; height: 34px; border-radius: var(--radius-full);
           display: grid; place-items: center; font-size: var(--text-sm); font-weight: 600;
           background: var(--accent-dim); color: var(--accent); }
  .ct-linha.fora .ct-av { background: var(--bg-elevated); color: var(--text-muted); }

  .ct-txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
  .ct-nome-l { display: flex; align-items: center; gap: var(--space-2); }
  .ct-nome { color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .ct-emuso { flex-shrink: 0; height: 18px; padding: 0 var(--space-2); border-radius: var(--radius-full);
              background: var(--accent-dim); color: var(--accent); font-size: 11px; line-height: 18px;
              letter-spacing: 0.02em; }
  .ct-sub { color: var(--text-secondary); font-size: var(--text-xs); }
  .ct-sub.fraco { color: var(--text-muted); }
  .ct-dir { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted);
            word-break: break-all; }

  /* Coluna do limite — o número/Nome é velho de propósito e tem que PARECER velho (régua
     "dado velho parece velho"; a faixa de cota da Task 9 vive aqui dentro). */
  .ct-cota { flex-shrink: 0; display: flex; flex-direction: column; gap: 4px; width: 190px; }
  .ct-idade { font-size: 11px; color: var(--text-muted); opacity: 0.9; }
  .ct-cota.velha .ct-idade { opacity: 0.75; }
  .ct-cota.velha { opacity: 0.55; }
  .ct-semleitura { flex-shrink: 0; width: 190px; font-size: 11px; color: var(--text-muted);
                   line-height: 1.4; }

  .ct-acao { flex-shrink: 0; height: 30px; min-height: 0; padding: 0 var(--space-3);
             border-radius: var(--radius-sm); border: 1px solid var(--border-subtle);
             background: var(--surface-raised); color: var(--text-primary); font-size: var(--text-xs);
             font-family: inherit; cursor: pointer; }
  .ct-acao.primaria { background: var(--accent); border-color: var(--accent); color: #fff; }
  .ct-kebab { flex-shrink: 0; width: 28px; height: 28px; min-height: 0; min-width: 0;
              border-radius: var(--radius-full); border: 1px solid var(--border-subtle);
              background: var(--surface-raised); color: var(--text-muted); font-size: var(--text-xs);
              cursor: pointer; }

  .ct-menu { flex-basis: 100%; display: flex; justify-content: flex-end; }
  .ct-menu-item { height: 30px; min-height: 0; padding: 0 var(--space-3); border-radius: var(--radius-sm);
                  border: 1px solid var(--border-subtle); background: var(--surface-raised);
                  color: var(--text-primary); font-size: var(--text-xs); font-family: inherit;
                  cursor: pointer; }

  .ct-confirma { flex-basis: 100%; display: flex; align-items: center; gap: var(--space-2);
                 flex-wrap: wrap; }
  .ct-confirma-txt { font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.4; }
  .ct-confirma-txt strong { color: var(--text-primary); }
  .ct-confirma-btn { height: 30px; min-height: 0; padding: 0 var(--space-3); border-radius: var(--radius-sm);
                     border: 1px solid var(--border-subtle); background: var(--surface-raised);
                     color: var(--text-primary); font-size: var(--text-xs); font-family: inherit;
                     cursor: pointer; }
  .ct-confirma-btn.perigo { color: var(--error); border-color: var(--border-default); }

  .ct-rodape { display: flex; gap: var(--space-2); margin-top: var(--space-3); flex-wrap: wrap;
               align-items: center; }
  .ct-btn { height: 36px; min-height: 0; padding: 0 var(--space-4); border-radius: var(--radius-sm);
            border: 1px solid var(--border-subtle); background: var(--surface-raised);
            color: var(--text-primary); font-size: var(--text-sm); font-family: inherit;
            cursor: pointer; }
  .ct-campo { height: 36px; min-height: 0; padding: 0 var(--space-3); flex: 1; min-width: 180px;
              background: var(--surface-inset); border: 1px solid var(--border-default);
              border-radius: var(--radius-sm); color: var(--text-primary);
              font-family: var(--font-mono); font-size: var(--text-sm); box-sizing: border-box; }

  .ct-herda { padding: var(--space-3); background: var(--surface-card);
              border: 1px solid var(--border-subtle); border-radius: var(--radius-md); }
  .ct-herda p { margin: 0 0 var(--space-2); font-size: var(--text-xs); color: var(--text-secondary);
                line-height: 1.45; }
  .ct-herda p:last-child { margin-bottom: 0; }

  /* Tangível no celular: target de toque >= 44px quando o painel aperta (o mock é desktop 1440px,
     onde 30px é confortável em mouse). O resto do layout quebra para a coluna do limite embaixo. */
  @container (max-width: 620px) {
    .ct-cota, .ct-semleitura { width: auto; flex-basis: 100%; }
    .ct-acao, .ct-kebab, .ct-menu-item, .ct-confirma-btn { height: 44px; min-height: 44px; }
    .ct-kebab { width: 44px; }
  }
</style>