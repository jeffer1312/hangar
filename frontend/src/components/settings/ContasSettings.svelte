<script lang="ts">
  // Aba Contas — TODA credencial deste servidor numa lista só: conta do Claude (login) e chave
  // de API, mesma linha, mesmo menu, mesmo limite à direita. Decisão do usuário em 18/08/2026,
  // depois de a chave de API ter vivido numa tela separada ("Motores") com outro vocabulário: a
  // pergunta "quanto sobrou nessa credencial?" só tinha resposta num dos dois lugares.
  //
  // Fonte: GET /api/credenciais (backend/app/credenciais.py), que já traz apelido e cota. As
  // ESCRITAS continuam nas rotas de sempre — /api/claude-configs pra conta, /api/engines pra
  // chave —, porque unificar a tela não pode virar dois donos do mesmo dado no servidor.
  //
  // Nome exibido é `nome` (o apelido, quando existe); TUDO que vai pra rota usa `nome_natural`,
  // que é o nome no disco. Trocar os dois faz o Entrar e o Apagar mirarem uma conta que não
  // existe assim que a pessoa renomear a primeira.
  import { onDestroy, untrack } from 'svelte';
import { criarConta, apagarConta, putEngine, putEngineForServer, deleteEngine, deleteEngineForServer, isAbortError, isTimeoutError } from '@hangar/core';
  import { formatarIntervalo } from '../../lib/contaEstado';
  import { listarCredenciais, definirApelido, definirCookie, type Credencial } from '../../lib/credenciais';
  import { iniciarLogin, passoLogin, confirmarLogin, cancelarLogin, type PassoLogin } from '../../lib/loginConta';
  import { initials } from '@hangar/core';
  import { nivelDePct, VELHA_APOS_S, motivoParado, motivoSessaoViva } from '../../lib/cota';
  import NovaCredencialSheet from './NovaCredencialSheet.svelte';
  import ProvedorIcone from '../icons/ProvedorIcone.svelte';
  import { serverIdentidade, type Server } from '../../lib/auth';
  import * as m from '../../paraglide/messages';

  // Contrato do apiTarget (o mesmo de ServidoresSettings): null = servidor ATIVO (API global com
  // self-heal de 401); Server explícito = a máquina que o ?srv= escolheu. Quem resolve é o App
  // (targetConfig): com ?srv=B a aba tem de falar com B, não com o ativo — o bloqueador da
  // revisão final (apagar/Entrar agiam na máquina errada, com o cabeçalho nomeando outra).
  interface Props {
    apiTarget: Server | null;
  }
  let { apiTarget }: Props = $props();

  let contas = $state<Credencial[]>([]);
  let carregando = $state(true);
  let erro = $state('');

  // Criar: um botão só ("+ Nova conta") e a escolha do TIPO acontece depois do clique — pedido
  // do usuário: "eu seleciono qual vou criar na hora". Dois botões lado a lado obrigavam a
  // decidir antes de saber que existiam duas coisas.
  let novo = $state<null | 'escolha' | 'claude' | 'chave'>(null);
  let nomeConta = $state('');
  let criando = $state(false);
  // Campos da conta por chave de API.
  let chaveNome = $state('');
  let chaveUrl = $state('');
  let chaveSegredo = $state('');
  // Renomear: o apelido é do app, não do disco — renomear pasta mexeria em caminho que um CLI
  // vivo tem aberto, e renomear motor quebraria o `hangar-engine --exec <nome>` de sessão rodando.
  let renomeando = $state<string | null>(null);   // id da credencial em edição
  let apelidoTexto = $state('');
  let salvandoApelido = $state(false);
  // Cookie do painel do OpenCode: ele não tem rota de cota (ver backend/app/opencode_cota.py),
  // então a leitura é a página do painel. Fica atrás do kebab e só na credencial que aceita —
  // oferecer o campo pra quem tem rota de verdade seria prometer trabalho inútil.
  let cookieDe = $state<string | null>(null);   // id da credencial com o formulário aberto
  let cookieWs = $state('');
  let cookieValor = $state('');
  let salvandoCookie = $state(false);
  // Apagar: kebab por linha abre o menu; confirmar apaga e recarrega.
  let menuDe = $state<string | null>(null);       // id da credencial com o menu aberto
  let confirmando = $state<string | null>(null);  // id da credencial com a confirmação aberta
  let apagando = $state(false);
  let aviso = $state('');
  let avisoErro = $state(false);

  // Botão de atualizar do cabeçalho: `atualizadoEm` alimenta o "atualizado há X", e o relógio
  // de 30 s faz o texto envelhecer sozinho. O intervalo morre no onDestroy junto com o poll
  // do login — nada fica rodando depois que a aba desmonta.
  let atualizando = $state(false);
  let atualizadoEm = $state<number | null>(null);
  let agora = $state(Date.now());
  const relogio = setInterval(() => { agora = Date.now(); }, 30_000);
  const idadeAtualizacao = $derived(
    atualizadoEm == null ? '' : formatarIntervalo(Math.max(0, (agora - atualizadoEm) / 1000)),
  );

  async function carregar(meu: number) {
    carregando = true;
    erro = '';
    try {
      const lista = await listarCredenciais(apiTarget);
      if (meu !== geracao) return;
      contas = lista;
      atualizadoEm = Date.now();
      agora = atualizadoEm;
    } catch (e) {
      if (meu !== geracao) return;
      erro = e instanceof Error && e.message ? e.message : String(e);
    } finally {
      if (meu === geracao) carregando = false;
    }
  }

  // Ao contrário de carregar(), NÃO liga `carregando`: a lista fica na tela durante a busca
  // (refresh não esvazia a coleção — padrão Cloudscape), quem gira é o ícone do botão. Pede a
  // leitura de AGORA (?forcar=true), não o cache de 5 min. Erro vai pro aviso embaixo da
  // lista — nunca some com os dados que já estavam certos.
  async function atualizar() {
    if (atualizando || carregando) return;
    // ++geracao, não só leitura: o refresh forçado é a leitura MAIS NOVA por definição —
    // invalida qualquer carga em voo (ex.: o carregar de um rename) pra um snapshot velho não
    // sobrescrever a resposta do botão (achado da revisão).
    const meu = ++geracao;
    atualizando = true;
    aviso = '';
    avisoErro = false;
    try {
      const lista = await listarCredenciais(apiTarget, true);
      if (meu !== geracao) return;
      contas = lista;
      atualizadoEm = Date.now();
      agora = atualizadoEm;
    } catch (e) {
      if (meu !== geracao) return;
      aviso = e instanceof Error && e.message ? e.message : String(e);
      avisoErro = true;
    } finally {
      if (meu === geracao) atualizando = false;
    }
  }

  // Geração da carga em voo (mesmo guard de ServidoresSettings.svelte:207-221): a resposta de um
  // alvo que a aba já não mostra não escreve na tela. Trocar de alvo com carga pendente deixava o
  // dado da máquina anterior na tela e o apagar clicado nele saía para a máquina errada — o
  // defeito que a Task 5 antiga levou duas rodadas pra fechar (33b0bffb, fa43b83e).
  let geracao = 0;
  let alvoAnterior: Server | null = null;
  // Identidade COMPOSTA (id+label+baseUrl+token), não o objeto: o App reconstrói o Server a cada
  // listServers() (JSON.parse do localStorage) e o sync sobe versaoServidores sem o usuário tocar
  // na aba — comparar o objeto matava um login em voo com o servidor sendo o MESMO (R1 do parecer;
  // mesmo contrato do SettingsModal.svelte:23-26).
  let identidadeAnterior: string | null = null;

  $effect(() => {
    const identidade = serverIdentidade(apiTarget);
    if (identidade === identidadeAnterior) return;
    identidadeAnterior = identidade;
    const meu = ++geracao;
    const alvoVelho = alvoAnterior;
    alvoAnterior = apiTarget;
    // Login em voo vive no processo do backend do alvo ANTIGO (uma tentativa por conta, naquela
    // máquina): trocar de alvo com login aberto tem de cancelar LÁ — cancelar no alvo novo
    // mataria a janela da máquina que saiu da tela. `untrack` de propósito: ler loginDe sem o
    // registrar como dependência, senão o próprio `loginDe = null` da limpeza reexecutava o
    // efeito numa segunda rodada de carregar()/cancelar.
    const loginAberto = untrack(() => loginDe);
    if (loginAberto) {
      cancelarLogin(alvoVelho, loginAberto).catch(() => {});
    }
    // Estados de diálogo/gravação pertencem ao alvo que saiu da tela: sem isto a confirmação de
    // apagar nascia aberta no alvo novo, o campo de criar ficava preso e o poll do login seguia
    // batendo no servidor antigo pra sempre.
    pararPoll();
    loginDe = null; loginCodigo = ''; loginPasso = { etapa: 'idle' };
    loginErro = ''; loginEnviando = false; loginIniciando = false; loginParado = false;
    erro = ''; aviso = ''; avisoErro = false;
    confirmando = null; menuDe = null;
    renomeando = null; apelidoTexto = ''; salvandoApelido = false;
    cookieDe = null; cookieWs = ''; cookieValor = ''; salvandoCookie = false;
    novo = null; nomeConta = ''; chaveNome = ''; chaveUrl = ''; chaveSegredo = '';
    criando = false; apagando = false;
    // Refresh em voo pertence ao alvo que saiu: o finally de atualizar() só limpa o flag se a
    // geração for a mesma, então sem este reset o botão ficava desabilitado PRA SEMPRE no alvo
    // novo (achado da revisão). O carimbo "atualizado há" também zera — ele era da outra máquina.
    atualizando = false; atualizadoEm = null;
    void carregar(meu);
  });

  // O backend relê a cota a cada 5 min, então "velha" aqui é o mesmo corte da faixa do rodapé
  // (10 min = duas tentativas falhadas), não "não é deste segundo": com o corte de 1 minuto a
  // coluna inteira nasceria esmaecida em toda montagem da tela.
  const leituraFresca = (c: Credencial) =>
    c.cota?.estado === 'lida' && (c.cota.idade_s == null || c.cota.idade_s <= VELHA_APOS_S);

  async function salvarChave() {
    const nome = chaveNome.trim();
    const url = chaveUrl.trim();
    const segredo = chaveSegredo.trim();
    if (!nome || !url || !segredo || criando) return;
    const g = geracao;
    criando = true;
    aviso = '';
    avisoErro = false;
    try {
      // Mesma rota que a tela de Motores sempre usou: o cadastro da chave é o engines.json.
      // `model: ''` é deliberado — a chave pode existir só pra acompanhar o limite; escolher
      // modelo é assunto de quem for RODAR o Claude Code nela, não de quem só a cadastra.
      const dados = { label: nome, base_url: url, api_key: segredo, model: '' };
      if (apiTarget) await putEngineForServer(apiTarget, chaveIdDe(nome), dados);
      else await putEngine(chaveIdDe(nome), dados);
      if (g !== geracao) return;
      novo = null; chaveNome = ''; chaveUrl = ''; chaveSegredo = '';
      await carregar(geracao);
    } catch (e) {
      if (g !== geracao) return;
      aviso = e instanceof Error && e.message ? e.message : m.criar_conta_erro();
      avisoErro = true;
    } finally {
      criando = false;
    }
  }

  // O engines.json tem alfabeto próprio pro nome (minúsculas, números, '-' e '_'): o nome bonito
  // vai pro `label` e o id sai daqui. Sem isto, "PMédico 01" seria recusado com 400 e o usuário
  // levaria a culpa por ter digitado um nome com espaço.
  function chaveIdDe(nome: string): string {
    const base = nome.toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g, '')
      .replace(/[^a-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 32);
    return base || `chave-${Date.now().toString(36)}`;
  }

  async function salvarApelido(c: Credencial) {
    if (salvandoApelido) return;
    const texto = apelidoTexto.trim();
    const g = geracao;
    salvandoApelido = true;
    try {
      await definirApelido(apiTarget, c.id, texto);
      if (g !== geracao) return;
      renomeando = null;
      apelidoTexto = '';
      await carregar(geracao);
    } catch (e) {
      if (g !== geracao) return;
      aviso = e instanceof Error && e.message ? e.message : String(e);
      avisoErro = true;
    } finally {
      salvandoApelido = false;
    }
  }

  async function salvarCookie(c: Credencial, apagar = false) {
    if (salvandoCookie) return;
    const g = geracao;
    salvandoCookie = true;
    aviso = '';
    avisoErro = false;
    try {
      await definirCookie(apiTarget, c.id, apagar ? '' : cookieWs.trim(), apagar ? '' : cookieValor.trim());
      if (g !== geracao) return;
      cookieDe = null; cookieWs = ''; cookieValor = '';
      await carregar(geracao);
    } catch (e) {
      if (g !== geracao) return;
      aviso = e instanceof Error && e.message ? e.message : String(e);
      avisoErro = true;
    } finally {
      salvandoCookie = false;
    }
  }

  async function novaConta() {
    const nome = nomeConta.trim();
    if (!nome || criando) return;
    // Geração desta operação: o aviso de "criada" pertence à máquina que recebeu o POST — se o
    // ?srv= trocou no meio do voo, a resposta da máquina antiga não escreve na tela da nova
    // (parecer da rodada 2, mesmo molde do iniciarEntrar).
    const g = geracao;
    criando = true;
    aviso = '';
    avisoErro = false;
    try {
      await criarConta(apiTarget, nome);
      if (g !== geracao) return;
      nomeConta = '';
      novo = null;
      aviso = m.criar_conta_deslogada();
      await carregar(geracao);
    } catch (e) {
      if (g !== geracao) return;
      aviso = e instanceof Error && e.message ? e.message : m.criar_conta_erro();
      avisoErro = true;
    } finally {
      criando = false;
    }
  }

  async function apagar() {
    const alvo = confirmando;
    if (!alvo || apagando) return;
    // `confirmando` guarda o ID (chave única da lista); a rota espera o nome NO DISCO. Derivar
    // do estado atual evita mandar um nome velho se a lista mudou entre o clique e o fim da
    // operação — e usar `nome_natural`, não `nome`, é o que faz apagar uma conta renomeada
    // mirar a pasta certa em vez de um apelido que rota nenhuma conhece.
    const conta = contas.find((x) => x.id === alvo);
    if (!conta) return;
    const idDisco = conta.id.startsWith('chave:') ? conta.id.slice('chave:'.length) : conta.nome_natural;
    // Geração desta operação: "conta X apagada" pertence à máquina que recebeu o DELETE — troca
    // de ?srv= no meio do voo não deixa o relato da máquina antiga na tela da nova (rodada 2).
    const g = geracao;
    apagando = true;
    aviso = '';
    avisoErro = false;
    try {
      if (conta.tipo === 'chave') {
        if (apiTarget) await deleteEngineForServer(apiTarget, idDisco);
        else await deleteEngine(idDisco);
      } else {
        await apagarConta(apiTarget, idDisco);
      }
      if (g !== geracao) return;
      confirmando = null;
      menuDe = null;
      aviso = m.criar_conta_apagada({ nome: conta.nome });
      // A conta pode ter sumido da lista entre o clique e o fim do DELETE (outro painel, outra
      // sessão) — recarregar é a fonte única, não remover item por item.
      await carregar(geracao);
    } catch (e) {
      if (g !== geracao) return;
      aviso = e instanceof Error && e.message ? e.message : m.criar_apagar_conta_erro();
      avisoErro = true;
    } finally {
      apagando = false;
    }
  }

  // ----------------------------------------------------------- login remoto (Task 7)
  // Estado da tentativa em voo: `loginDe` é o LABEL da conta (o nome ~/.claude-<nome>, a
  // chave estável do fluxo). Uma tentativa por conta; o botão Entrar de outra conta fica
  // desabilitado enquanto uma está em voo (uma janela escondida por vez).
  let loginDe = $state<string | null>(null);
  let loginPasso = $state<PassoLogin>({ etapa: 'idle' });
  let loginCodigo = $state('');
  let loginEnviando = $state(false);
  let loginErro = $state('');
  let loginPoll: ReturnType<typeof setInterval> | null = null;
  let loginIniciando = $state(false);
  let loginParado = $state(false);
  // B4 — o onDestroy só enxerga `loginDe`, que é escrito DEPOIS do await do iniciar:
  // desmontar ENTRE o clique e a resposta do servidor deixava o poll órfão e a tentativa
  // presa (o próximo Entrar caía em 409 sem botão de Cancelar). Flag como a do
  // Composer.svelte (getUserMedia em voo num componente morto): quem morreu não pode
  // armar poll nem registrar tentativa.
  let destruido = $state(false);

  async function iniciarEntrar(conta: Credencial) {
    if (loginDe || loginIniciando) return;
    // Alvo desta tentativa, capturado AGORA: se o ?srv= trocar no meio do voo, este é o alvo
    // ANTIGO — o cancelamento do efeito de geração e o ramo `g !== geracao` abaixo usam o
    // capturado, nunca o apiTarget corrente.
    const alvo = apiTarget;
    const g = geracao;
    loginIniciando = true;
    loginErro = '';
    loginCodigo = '';
    loginPasso = { etapa: 'idle' };
    try {
      await iniciarLogin(alvo, conta.nome_natural);
      if (destruido || g !== geracao) {
        // Desmontou ou o alvo trocou ENTRE o clique e a resposta: sem tela onde mostrar erro
        // (o mesmo ramo silencioso do onDestroy), mas a janela do servidor — a máquina ANTIGA —
        // precisa morrer de qualquer forma.
        cancelarLogin(alvo, conta.nome_natural).catch(() => {});
        return;
      }
      loginDe = conta.nome_natural;
      // Primeira leitura do passo logo de cara (a URL pode já estar no pane), depois o poll.
      // O poll só começa depois do login confirmado no servidor: um 409/404 no iniciar NÃO
      // deixa intervalo órfão rodando.
      try {
        loginPasso = await passoLogin(alvo, conta.nome_natural);
      } catch {
        // Poll silencioso: o erro de rede aparece na ação (confirmar/cancelar), não no loop.
      }
      loginPoll = setInterval(async () => {
        if (loginDe) {
          try {
            loginPasso = await passoLogin(alvo, loginDe);
          } catch {
            // Silencioso: o erro aparece nas ações, não no loop.
          }
        }
      }, 2000);
    } catch (e) {
      // O que chega: 401 com token (sessao_expirada), o texto do envelope do backend
      // traduzido (mensagemDeErro) ou erro de rede. 'Failed to fetch' cru (fetch abortado,
      // sem resposta) NAO vai pra tela — vira falha de conexao generica. Erro com status
      // (409/404/504) carrega a mensagem traduzida do servidor; o resto e rede.
      const comStatus = e instanceof Error && typeof (e as { status?: unknown }).status === 'number';
      loginErro = comStatus && e instanceof Error && e.message
        ? e.message
        : m.falha_conexao();
    } finally {
      loginIniciando = false;
    }
  }

  function pararPoll() {
    if (loginPoll) {
      clearInterval(loginPoll);
      loginPoll = null;
    }
  }

  async function confirmarEntrar() {
    const conta = loginDe;
    if (!conta || loginEnviando) return;
    // Geração desta tentativa: a resposta de um alvo que saiu da tela não escreve aviso/erro nela
    // (o molde é o iniciarEntrar, 60 linhas acima). O teto do confirmar (310s) deixa o voo aberto
    // por minutos — justo a janela em que o usuário espera o OAuth e pode trocar o ?srv=
    // (parecer da rodada 2).
    const g = geracao;
    loginEnviando = true;
    loginErro = '';
    try {
      const r = await confirmarLogin(apiTarget, conta, loginCodigo);
      if (g !== geracao) return;
      pararPoll();
      loginDe = null;
      loginCodigo = '';
      loginPasso = { etapa: 'idle' };
      // Aviso de sucesso: o e-mail e o plano que o servidor RELÊU da conta (a confirmação
      // por releitura, não pela aparência da tela).
      aviso = m.contas_login_ok({ email: r.email ?? '', plano: r.plano ?? '' });
      avisoErro = false;
      await carregar(geracao);
    } catch (e) {
      if (g !== geracao) return;
      loginErro = e instanceof Error && e.message ? e.message : m.falha_conexao();
      // O erro NÃO prova que o login falhou: o teto pode ter cortado com o backend SEGUINDO
      // (a conta acaba logada de verdade). Recarregar a lista para de mentir sozinha — a tela
      // mostra a conta como o servidor a vê (parecer da rodada 1, passo 4).
      await carregar(geracao);
    } finally {
      loginEnviando = false;
    }
  }

  async function cancelarEntrar() {
    const conta = loginDe;
    if (!conta) return;
    loginParado = true;
    loginErro = '';
    try {
      await cancelarLogin(apiTarget, conta);
    } catch {
      // O servidor pode não ter janela pra matar (já morreu); a tentativa local morre igual.
    } finally {
      pararPoll();
      loginDe = null;
      loginCodigo = '';
      loginPasso = { etapa: 'idle' };
      loginParado = false;
    }
  }

  // B8 — o poll e a tentativa em voo NÃO podem sobreviver à desmontagem do componente.
  // Portas: trocar de aba, fechar o modal, a janela cruzar 820px (DesktopShell →
  // SessionList desmonta a aba). Sem isto o setInterval fica órfão batendo em /login/passo
  // e a tentativa continua no servidor — o próximo Entrar cai em 409 sem que exista botão
  // de Cancelar na tela. O .catch(() => {}) é deliberado e é o único ramo silencioso
  // aceitável aqui: o componente já não existe, não há tela onde mostrar o erro, e a
  // janela do servidor precisa morrer de qualquer forma.
  onDestroy(() => {
    destruido = true;
    pararPoll();
    clearInterval(relogio);
    if (loginDe) cancelarLogin(apiTarget, loginDe).catch(() => {});
  });
</script>

<div class="ct-superficie">
  <!-- Cabeçalho da coleção: título + "atualizado há X" + botão de atualizar na mesma linha
       (referência Cloudscape/AWS: refresh no cabeçalho, timestamp ao lado, lista visível
       durante a busca). O ícone é SVG traçado 2, como o lápis e o kebab. -->
  <div class="ct-cab">
    <p class="st-secao ct-topo">{m.contas_secao_lista()}</p>
    {#if atualizadoEm != null}
      <span class="ct-atualizado" aria-live="polite">{m.contas_atualizado_ha({ n: idadeAtualizacao })}</span>
    {/if}
    <button type="button" class="ct-refresh" onclick={atualizar}
      disabled={atualizando || carregando} aria-label={m.contas_atualizar()}
      title={m.contas_atualizar()}>
      <svg class:girando={atualizando} width="15" height="15" viewBox="0 0 24 24" fill="none"
        stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M21 12a9 9 0 1 1-2.64-6.36" /><path d="M21 3v6h-6" />
      </svg>
    </button>
  </div>
  <p class="ct-legenda">{m.contas_legenda()}</p>

  {#if carregando}
    <p class="ct-aviso">{m.comum_carregando()}</p>
  {:else if erro}
    <p class="ct-aviso erro" role="alert">{erro}</p>
  {:else if !contas.length}
    <p class="ct-aviso">{m.comum_nada_encontrado()}</p>
  {:else}
    <div class="ct-cartao">
      {#each contas as conta (conta.id)}
        <!-- Marcas de uso ("roda o Claude Code", "cota pelo painel") saem da linha do NOME e viram
             texto na linha do subtítulo. Pílula fica só para o TIPO da credencial e para "em uso":
             com quatro pílulas na mesma linha, o nome — que é o que distingue uma linha da outra —
             sumia no meio do enfeite. Mesmas chaves de sempre, nenhum texto novo. -->
        {@const marcas = [
          ...(conta.usos.includes('claude_code') ? [m.contas_usa_claude_code()] : []),
          ...(conta.cookie_definido ? [m.contas_cookie_definido()] : []),
        ]}
        <!-- O caminho em mono é o último item das duas metades da lista (pasta da conta / nome no
             disco do motor), então ele é calculado uma vez em vez de repetir o <span> nos ramos. -->
        {@const dir = conta.tipo === 'chave'
          ? (conta.nome !== conta.nome_natural ? conta.nome_natural : '')
          : (conta.path ?? '')}
        <div class="ct-linha" class:fora={conta.login?.estado === 'ok' && !conta.login.loggedIn}>
          <span class="ct-ico">
            <ProvedorIcone tipo={conta.tipo} baseUrl={conta.base_url} iniciais={initials(conta.nome)} />
          </span>
          <span class="ct-txt">
            <span class="ct-nome-l">
              {#if renomeando === conta.id}
                <!-- svelte-ignore a11y_autofocus -->
                <input class="ct-campo ct-campo-nome" type="text" autofocus bind:value={apelidoTexto}
                  aria-label={m.contas_renomear({ nome: conta.nome })} disabled={salvandoApelido}
                  onkeydown={(e) => {
                    if (e.key === 'Enter') { e.preventDefault(); salvarApelido(conta); }
                    else if (e.key === 'Escape') { renomeando = null; apelidoTexto = ''; }
                  }} />
                <button type="button" class="ct-mini" onclick={() => salvarApelido(conta)}
                  disabled={salvandoApelido}>{salvandoApelido ? '…' : m.ctx_salvar()}</button>
                <button type="button" class="ct-mini" onclick={() => { renomeando = null; apelidoTexto = ''; }}
                  disabled={salvandoApelido}>{m.comum_cancelar()}</button>
              {:else}
                <span class="ct-nome">{conta.nome}</span>
                <!-- Lápis e kebab em SVG traçado 2, como o resto do app (components/icons,
                     DesktopSessionContext): glifo de texto (✎ / ⋯) no meio de uma UI de ícone
                     desenhado muda de peso e de linha de base conforme a fonte do sistema. -->
                <button type="button" class="ct-lapis" aria-label={m.contas_renomear({ nome: conta.nome })}
                  onclick={() => { renomeando = conta.id; apelidoTexto = conta.apelido ?? ''; }}>
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                    stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="M12 20h9" />
                    <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" />
                  </svg>
                </button>
                <span class="ct-tipo" class:chave={conta.tipo === 'chave'}>
                  {conta.tipo === 'chave' ? m.contas_tipo_chave() : m.contas_tipo_claude()}</span>
                {#if conta.ativa}<span class="ct-emuso">{m.contas_em_uso()}</span>{/if}
              {/if}
            </span>
            {#if conta.tipo === 'chave'}
              <!-- A chave NUNCA volta inteira do servidor (credenciais._mascarar): o que a tela
                   mostra é o rabicho, o bastante pra saber QUAL chave é sem expor a chave. -->
              <span class="ct-sub">{conta.base_url ?? ''}{conta.chave_mascarada ? ` · ${conta.chave_mascarada}` : ''}</span>
            {:else if conta.login?.estado === 'ok' && conta.login.loggedIn && conta.login.email}
              <span class="ct-sub">{conta.login.email}</span>
            {:else if conta.login?.estado === 'ok' && !conta.login.loggedIn}
              <span class="ct-sub fraco">{m.contas_nao_conectada()}</span>
            {/if}
            <!-- Marcas e caminho dividem UMA linha (densidade, 18/08): eram duas, e nenhuma das
                 duas é a identidade da conta — quem identifica é o nome e o e-mail acima. -->
            {#if marcas.length || dir}
              <span class="ct-sub-l">
                {#if marcas.length}<span class="ct-marcas">{marcas.join(' · ')}</span>{/if}
                {#if dir}<span class="ct-dir">{dir}</span>{/if}
              </span>
            {/if}
          </span>

          <!-- Coluna do limite: a MESMA leitura da faixa do rodapé (uma fonte só). Credencial sem
               número aparece dizendo por quê — some-la esconderia justo a que precisa de atenção.
               O medidor é o MESMO dado do número, em forma de comprimento: a pergunta da tela é
               "quanto sobrou nessa credencial?", e um dígito só se compara lendo, uma barra se
               compara de relance entre as linhas. Por ser repetição do número, é aria-hidden. -->
          {#if conta.cota && conta.cota.estado === 'lida' && conta.cota.janelas.length}
            <span class="ct-cota" class:velha={!leituraFresca(conta)}>
              {#each conta.cota.janelas as j (j.rotulo)}
                <span class="ct-jan">
                  <span class="ct-jan-rot">{j.rotulo}</span>
                  <span class="ct-barra" aria-hidden="true">
                    <i class={nivelDePct(j.pct)} style="width:{Math.min(100, Math.max(0, j.pct))}%"></i>
                  </span>
                  <b class={nivelDePct(j.pct)}>{Math.round(j.pct)}%</b>
                </span>
              {/each}
              {#if !leituraFresca(conta)}
                <span class="ct-idade">{m.cota_ultima_leitura({ n: formatarIntervalo(conta.cota.idade_s) })}</span>
              {/if}
            </span>
          {:else if conta.cota && (conta.cota.estado === 'expirada' || conta.cota.estado === 'sem_credencial')}
            <!-- sessao-viva NÃO é "abra uma sessão" — a sessão já está aberta (foi como o
                 usuário leu a frase estando dentro dela, 19/08). Quem renova é o CLI dela. -->
            <span class="ct-semleitura"
              >{motivoSessaoViva(conta.cota.motivo) ? m.cota_sessao_viva()
                : motivoParado(conta.cota.motivo) ? m.cota_conta_parada() : m.cota_precisa_entrar()}</span>
          {:else}
            <span class="ct-semleitura">{m.contas_sem_cota()}</span>
          {/if}

          <!-- Um envelope só para as ações: o Entrar é condicional, e sem ele o kebab caía na
               COLUNA do limite (a grade coloca por posição, não por classe). -->
          <span class="ct-acoes">
            {#if conta.tipo === 'claude' && conta.login?.estado === 'ok' && !conta.login.loggedIn}
              <button type="button" class="ct-acao primaria"
                aria-label={m.contas_entrar_titulo({ nome: conta.nome })}
                disabled={!!loginDe || loginIniciando}
                onclick={() => iniciarEntrar(conta)}>{m.contas_entrar()}</button>
            {/if}

            <button type="button" class="ct-kebab" aria-haspopup="true" aria-expanded={menuDe === conta.id}
              aria-label={m.comum_fechar_menu_conta()} onclick={() => (menuDe = menuDe === conta.id ? null : conta.id)}>
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <circle cx="12" cy="5" r="1" />
                <circle cx="12" cy="12" r="1" />
                <circle cx="12" cy="19" r="1" />
              </svg>
            </button>
          </span>

          {#if menuDe === conta.id && confirmando !== conta.id}
            <div class="ct-menu">
              {#if conta.aceita_cookie}
                <button type="button" class="ct-menu-item"
                  onclick={() => { menuDe = null; cookieDe = conta.id; cookieWs = ''; cookieValor = ''; }}
                  >{m.contas_cookie_acao()}</button>
                {#if conta.cookie_definido}
                  <button type="button" class="ct-menu-item"
                    onclick={() => { menuDe = null; salvarCookie(conta, true); }}
                    >{m.contas_cookie_apagar()}</button>
                {/if}
              {/if}
              <button type="button" class="ct-menu-item"
                onclick={() => { menuDe = null; confirmando = conta.id; }}>{m.comum_apagar()}</button>
            </div>
          {/if}

          {#if cookieDe === conta.id}
            <div class="ct-cookie">
              <p class="ct-form-leg">{m.contas_cookie_legenda()}</p>
              <div class="ct-form-linha">
                <label class="ct-campo-l">
                  <span>{m.contas_cookie_ws()}</span>
                  <!-- svelte-ignore a11y_autofocus -->
                  <input class="ct-campo" type="text" autofocus bind:value={cookieWs}
                    disabled={salvandoCookie} />
                </label>
                <label class="ct-campo-l larga">
                  <span>{m.contas_cookie_valor()}</span>
                  <input class="ct-campo" type="password" autocomplete="off"
                    bind:value={cookieValor} disabled={salvandoCookie} />
                </label>
              </div>
              <div class="ct-rodape">
                <button type="button" class="ct-btn primario" onclick={() => salvarCookie(conta)}
                  disabled={salvandoCookie || !cookieWs.trim() || !cookieValor.trim()}
                  >{salvandoCookie ? '…' : m.ctx_salvar()}</button>
                <button type="button" class="ct-btn" disabled={salvandoCookie}
                  onclick={() => { cookieDe = null; cookieWs = ''; cookieValor = ''; }}
                  >{m.comum_cancelar()}</button>
              </div>
            </div>
          {/if}

          {#if confirmando === conta.id}
            <div class="ct-confirma">
              <span class="ct-confirma-txt">
                {m.comum_apagar()} <strong>{conta.nome}</strong> {m.criar_apagar_fim()}
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
      <!-- UM botão. A escolha do provedor e o formulário vivem no modal (NovaCredencialSheet):
           inline, a pergunta e as opções viravam cinco controles competindo pela mesma linha. -->
      <button type="button" class="ct-btn" onclick={() => (novo = 'escolha')}
        disabled={!!novo}>{m.contas_nova()}</button>
    </div>
  {/if}

  {#if novo}
    <NovaCredencialSheet {apiTarget}
      onFechar={() => (novo = null)}
      onCriada={() => { void carregar(geracao); }} />
  {/if}

  {#if aviso}
    <p class="ct-aviso" class:erro={avisoErro} role={avisoErro ? 'alert' : 'status'} aria-live="polite">{aviso}</p>
  {/if}

  {#if loginDe}
    <!-- Passo a passo do login remoto (mock estado 2): o link de autorização, o campo do
         código e a confirmação por RELEITURA do estado da conta (o passo 4 não se completa
         pela aparência — só quando o servidor relê logada e devolve e-mail e plano). -->
    <div class="ct-login">
      <p class="st-secao ct-topo">{m.contas_entrar_titulo({ nome: loginDe })}</p>

      <div class="ct-passo feito">
        <span class="ct-num" aria-hidden="true">✓</span>
        <span class="ct-passo-txt">{m.contas_passo1()}</span>
      </div>

      <div class="ct-passo" class:espera={!loginPasso.url}>
        <span class="ct-num" aria-hidden="true">2</span>
        <span class="ct-passo-txt">
          <b>{m.contas_passo2()}</b>
          {#if loginPasso.url}
            <a class="ct-link" href={loginPasso.url} target="_blank" rel="noopener noreferrer">{loginPasso.url}</a>
          {/if}
        </span>
      </div>

      <div class="ct-passo">
        <span class="ct-num" aria-hidden="true">3</span>
        <span class="ct-passo-txt">
          <b>{m.contas_passo3()}</b>
          <input class="ct-campo-cod" type="text" autocomplete="one-time-code"
            placeholder={m.contas_codigo_placeholder()} aria-label={m.contas_codigo_placeholder()}
            bind:value={loginCodigo} disabled={loginEnviando}
            onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); confirmarEntrar(); } }} />
        </span>
      </div>

      <div class="ct-passo espera">
        <span class="ct-num" aria-hidden="true">4</span>
        <span class="ct-passo-txt">{m.contas_passo4()}</span>
      </div>
    </div>

    <div class="ct-rodape">
      <button type="button" class="ct-btn" onclick={cancelarEntrar}
        disabled={loginParado}>{loginParado ? '…' : m.comum_cancelar()}</button>
      <button type="button" class="ct-btn primario" onclick={confirmarEntrar}
        disabled={loginEnviando || !loginCodigo.trim()}>{loginEnviando ? '…' : m.contas_confirmar_codigo()}</button>
    </div>
  {/if}

  {#if loginErro}
    <!-- Fora do {#if loginDe} de proposito: quando o INICIO falha (409/404), o loginDe
         nunca é setado e o aviso de erro não poderia aparecer dentro do bloco. -->
    <p class="ct-aviso erro" role="alert">{loginErro}</p>
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

  /* Cabeçalho da coleção (19/08): título + "atualizado há X" + botão de atualizar na mesma
     linha — referência Cloudscape/AWS: refresh no cabeçalho, timestamp ao lado, lista visível
     durante a busca. O margin do .st-secao é zerado AQUI (local) pra não depender de onde
     vem o estilo-base do título. */
  .ct-cab { display: flex; align-items: center; gap: var(--space-2);
            margin: 0 var(--space-2) var(--space-1); }
  .ct-cab .st-secao { flex: 1; min-width: 0; margin: 0; }
  .ct-atualizado { font-size: var(--text-2xs); color: var(--text-muted); white-space: nowrap; }
  .ct-refresh { flex-shrink: 0; width: 28px; height: 28px; min-height: 0; min-width: 0;
                display: grid; place-items: center; border-radius: var(--radius-full);
                border: 1px solid var(--border-subtle); background: transparent;
                color: var(--text-muted); cursor: pointer; }
  .ct-refresh:hover { color: var(--text-primary); border-color: var(--border-default); }
  /* `spin` é o keyframes global do app.css. Gira só o SVG: o botão parado com o ícone
     girando lê como "trabalhando" sem mexer o layout do cabeçalho. */
  .ct-refresh svg.girando { animation: spin 0.8s linear infinite; }

  .ct-legenda { margin: 0 var(--space-2) var(--space-3); color: var(--text-muted);
                font-size: var(--text-xs); line-height: 1.45; }
  .ct-sep { height: 1px; background: var(--border-subtle); margin: var(--space-4) 0 var(--space-3); }
  .ct-aviso { margin: var(--space-2); color: var(--text-muted); font-size: var(--text-sm); }
  .ct-aviso.erro { color: var(--error); }

  .ct-cartao { background: var(--surface-card); border: 1px solid var(--border-subtle);
               border-radius: var(--radius-md); overflow: hidden; }
  /* A linha é uma GRADE de quatro colunas — ícone · texto · limite · ações — e não mais um flex
     que embrulha. Em flex com `align-items: center`, uma linha com três janelas de cota ficava
     mais alta que a de duas e o kebab boiava no meio dela; e as colunas do limite não caíam no
     mesmo x entre linhas vizinhas, que é justo o que se quer comparar de relance. Com colunas
     explícitas e `align-items: start`, tudo começa na mesma altura e o limite fica numa coluna
     de verdade. Os blocos de largura cheia (menu, confirmação, cookie) usam `grid-column: 1/-1`,
     que é o que `flex-basis: 100%` fazia antes. */
  /* Densidade (18/08, pedido do usuário: "cada card de conta poderia diminuir"): a linha tinha
     ~110px de altura — 12px de padding, avatar de 34, três linhas de texto empilhadas e a coluna
     do limite gastando DUAS linhas por janela (número em cima, barra embaixo). Agora são ~64px,
     sem tirar nenhuma informação da tela: o padding cai pra 8, o avatar pra 28, e-mail e caminho
     dividem uma linha, e cada janela vira uma linha só (rótulo · barra · número). */
  .ct-linha { display: grid; grid-template-columns: auto minmax(0, 1fr) auto auto;
              align-items: center; gap: var(--space-3); padding: var(--space-2) var(--space-3);
              position: relative; }
  .ct-linha + .ct-linha { border-top: 1px solid var(--border-subtle); }
  .ct-linha:hover { background: var(--bg-hover); }
  /* Realce de estado é tinta por cima da linha, não superfície nova — por isso --bg-hover cru
     aqui, e não um --surface-*: o véu do papel de parede continua vindo do cartão. */
  .ct-ico { display: grid; place-items: center; }
  .ct-acoes { display: flex; align-items: center; gap: var(--space-2); }

  .ct-av { flex-shrink: 0; width: 28px; height: 28px; border-radius: var(--radius-full);
           display: grid; place-items: center; font-size: var(--text-sm); font-weight: 600;
           background: var(--accent-dim); color: var(--accent); }
  .ct-linha.fora .ct-av { background: var(--bg-elevated); color: var(--text-muted); }

  .ct-txt { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
  /* A linha do nome NÃO quebra: com um rótulo longo (o motor "DeepSeek · opencode direto (sem
     gateway)") o nome descia pra duas linhas e empurrava a linha inteira, e a etiqueta ao lado
     quebrava no meio de "chave de API". Quem cede é o NOME (elipse), nunca a etiqueta — o tipo da
     credencial é o que distingue as duas metades da lista. */
  .ct-nome-l { display: flex; align-items: center; gap: var(--space-2); min-width: 0; }
  .ct-nome {
    color: var(--text-primary); font-size: var(--text-sm); font-weight: 600;
    min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .ct-emuso { flex-shrink: 0; height: 18px; padding: 0 var(--space-2); border-radius: var(--radius-full);
              background: var(--accent-dim); color: var(--accent); font-size: 11px; line-height: 18px;
              letter-spacing: 0.02em; }
  .ct-sub { flex-shrink: 0; color: var(--text-secondary); font-size: var(--text-xs); }
  .ct-sub.fraco { color: var(--text-muted); }
  /* O que a credencial serve ("roda o Claude Code", "cota pelo painel"): texto, não pílula. */
  .ct-marcas { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4; }
  .ct-sub-l { display: flex; align-items: baseline; gap: var(--space-2); min-width: 0; }
  /* O caminho cede primeiro (elipse) em vez de quebrar em várias linhas: `word-break: break-all`
     num caminho longo devolvia justamente as duas linhas que esta densidade veio tirar. */
  .ct-dir { font-family: var(--font-mono); font-size: var(--text-2xs); color: var(--text-muted);
            min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* Coluna do limite — o número/Nome é velho de propósito e tem que PARECER velho (régua
     "dado velho parece velho"; a faixa de cota da Task 9 vive aqui dentro). */
  .ct-cota { display: flex; flex-direction: column; gap: 3px; width: 190px; }
  .ct-idade { font-size: 11px; color: var(--text-muted); opacity: 0.9; }
  .ct-cota.velha .ct-idade { opacity: 0.75; }
  .ct-cota.velha { opacity: 0.55; }
  .ct-semleitura { width: 190px; font-size: var(--text-2xs); color: var(--text-muted);
                   line-height: 1.4; }

  .ct-acao { flex-shrink: 0; height: 30px; min-height: 0; padding: 0 var(--space-3);
             border-radius: var(--radius-sm); border: 1px solid var(--border-subtle);
             background: var(--surface-raised); color: var(--text-primary); font-size: var(--text-xs);
             font-family: inherit; cursor: pointer; }
  .ct-acao.primaria { background: var(--accent); border-color: var(--accent); color: #fff; }
  /* Kebab fantasma (19/08): a ação é rara e o círculo com borda/fundo disputava a linha com o
     nome da conta. Vira ícone solto como o lápis; o alvo de toque de 44px no estreito continua
     (container query abaixo). */
  .ct-kebab { flex-shrink: 0; width: 28px; height: 28px; min-height: 0; min-width: 0;
              display: grid; place-items: center;
              border-radius: var(--radius-full); border: none;
              background: transparent; color: var(--text-muted); font-size: var(--text-xs);
              cursor: pointer; }
  .ct-kebab:hover { color: var(--text-primary); background: var(--bg-hover); }

  .ct-menu { grid-column: 1 / -1; display: flex; justify-content: flex-end; }
  .ct-menu-item { height: 30px; min-height: 0; padding: 0 var(--space-3); border-radius: var(--radius-sm);
                  border: 1px solid var(--border-subtle); background: var(--surface-raised);
                  color: var(--text-primary); font-size: var(--text-xs); font-family: inherit;
                  cursor: pointer; }

  .ct-confirma { grid-column: 1 / -1; display: flex; align-items: center; gap: var(--space-2);
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

  /* Passo a passo do login remoto (mock estado 2) — mesmas medidas do mock, tokens reais. */
  .ct-login { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-4);
              background: var(--surface-card); border: 1px solid var(--border-subtle);
              border-radius: var(--radius-md); }
  .ct-passo { display: flex; gap: var(--space-3); align-items: flex-start; }
  .ct-num { flex-shrink: 0; width: 20px; height: 20px; border-radius: var(--radius-full);
            background: var(--accent-dim); color: var(--accent); font-size: 11px; font-weight: 600;
            display: grid; place-items: center; margin-top: 1px; }
  .ct-passo.feito .ct-num { background: rgba(52, 199, 89, 0.16); color: var(--success); }
  .ct-passo.espera .ct-num { background: var(--bg-elevated); color: var(--text-muted); }
  .ct-passo-txt { font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.5;
                  min-width: 0; }
  .ct-passo-txt b { color: var(--text-primary); font-weight: 600; }
  .ct-link { display: inline-block; margin-top: var(--space-1); font-family: var(--font-mono);
             font-size: 11px; color: var(--accent); word-break: break-all; }
  .ct-campo-cod { width: 100%; height: 38px; margin-top: var(--space-2); padding: 0 var(--space-3);
              background: var(--surface-inset); border: 1px solid var(--border-default);
              border-radius: var(--radius-sm); color: var(--text-primary);
              font-family: var(--font-mono); font-size: var(--text-sm); box-sizing: border-box; }
  .ct-btn.primario { background: var(--accent); border-color: var(--accent); color: #fff; }

  /* Teclado: quem chega no Tab tem de VER onde está. Sem isto o lápis e o kebab (fundo
     transparente / sutil) só mostravam o anel padrão do navegador, que some no fundo escuro. */
  .ct-lapis:focus-visible, .ct-kebab:focus-visible, .ct-acao:focus-visible,
  .ct-menu-item:focus-visible, .ct-btn:focus-visible, .ct-mini:focus-visible,
  .ct-confirma-btn:focus-visible, .ct-refresh:focus-visible {
    outline: 2px solid var(--accent); outline-offset: 2px;
  }
  /* Desabilitado PARECE desabilitado — o Entrar fica inerte enquanto há outro login em curso, e
     os botões do formulário do cookie enquanto ele salva. */
  .ct-lapis:disabled, .ct-kebab:disabled, .ct-acao:disabled, .ct-menu-item:disabled,
  .ct-btn:disabled, .ct-mini:disabled, .ct-confirma-btn:disabled, .ct-refresh:disabled {
    opacity: .55; cursor: default;
  }

  /* Tangível no celular: target de toque >= 44px quando o painel aperta (o mock é desktop 1440px,
     onde 30px é confortável em mouse). O limite desce para uma faixa de largura cheia embaixo do
     texto — a grade perde a coluna dele e as ações continuam à direita da primeira linha. */
  @container (max-width: 620px) {
    .ct-linha { grid-template-columns: auto minmax(0, 1fr) auto; }
    .ct-ico   { grid-column: 1; grid-row: 1; }
    .ct-txt   { grid-column: 2; grid-row: 1; }
    .ct-acoes { grid-column: 3; grid-row: 1; }
    .ct-cota, .ct-semleitura { grid-column: 1 / -1; grid-row: 2; width: auto; }
    .ct-acao, .ct-kebab, .ct-menu-item, .ct-confirma-btn { height: 44px; min-height: 44px; }
    /* O lápis também: o `min-height: 0` lá em cima é o que dá a densidade no desktop (mouse),
       e sem esta linha ele descia a 18px no celular — abaixo do alvo tangível, justo num botão
       que fica colado no nome. A linha do nome volta a 44px aqui, e é o certo: quem lê no
       celular precisa acertar o dedo, não caber mais uma conta na tela. */
    .ct-lapis { min-height: 44px; min-width: 44px; }
    .ct-kebab { width: 44px; }
    /* O refresh do cabeçalho também é alvo de dedo no estreito. */
    .ct-refresh { width: 36px; height: 36px; }
    .ct-btn { height: 44px; }
  }

  /* ---------------------------------------------------------------- lista unificada (18/08)
     A linha é a MESMA pros dois tipos — conta do Claude e chave de API. O que muda é a etiqueta
     e o subtítulo; desenhar duas linhas diferentes traria de volta os dois vocabulários que a
     unificação veio matar. */
  .ct-av.chave { background: var(--warning-dim, rgba(232,145,45,.16)); color: var(--warning); }
  .ct-lapis {
    /* Sobrescreve o alvo de toque global de 44px (app.css): sem isto o BOTÃO define a altura da
       linha do nome — 44px de linha para um lápis de 14px, e a linha da conta inteira herdava
       isso (medido: linha de 98px, sendo 44 só a do nome). Mesmo remédio do .ct-kebab e da
       árvore de arquivos. O container query estreito devolve os 44px de alvo (regra lá embaixo). */
    min-height: 0; min-width: 0;
    flex-shrink: 0; display: grid; place-items: center;
    background: transparent; border: none; padding: 0 2px; cursor: pointer;
    color: var(--text-muted); line-height: 1; opacity: .75;
  }
  .ct-lapis:hover { opacity: 1; color: var(--text-secondary); }
  .ct-tipo {
    flex-shrink: 0; white-space: nowrap;
    font-size: 10px; padding: 1px 7px; border-radius: 999px;
    background: var(--accent-dim); color: var(--accent);
  }
  .ct-tipo.chave { background: rgba(232,145,45,.16); color: var(--warning); }
  .ct-campo-nome { max-width: 22ch; }
  .ct-mini {
    background: var(--surface-raised); border: 1px solid var(--border-subtle);
    color: var(--text-secondary); border-radius: 6px; padding: 2px 8px;
    font: inherit; font-size: 11px; cursor: pointer;
  }
  /* Coluna do limite: as janelas do provedor, empilhadas. Números tabulares pra coluna não dançar
     entre as linhas — é uma tabela, mesmo sem ser <table>. Cada janela é rótulo+número numa linha
     e o medidor embaixo, ocupando a coluna inteira: assim as barras de linhas vizinhas começam e
     terminam no mesmo x e dá pra comparar duas credenciais sem ler dígito. */
    /* Uma linha por janela: rótulo à esquerda, barra ocupando o vão, número à direita — a barra
     no meio é o que deixa as leituras comparáveis de relance entre as contas. O número é
     metadado (12px, 19/08): no tamanho do corpo ele disputava a linha com o nome da conta —
     referência: tela Usage do app do Claude, onde o % é leitura, não título. */
  .ct-jan { display: flex; align-items: center; gap: var(--space-2);
            font-variant-numeric: tabular-nums; }
  .ct-jan-rot { color: var(--text-muted); font-size: 10px; }
  .ct-jan b { min-width: 4ch; text-align: right; font-weight: var(--fw-semibold);
              color: var(--text-secondary); font-size: var(--text-xs); }
  .ct-jan b.alerta { color: var(--warning); }
  .ct-jan b.cheio { color: var(--error); }
  /* Trilho em --surface-raised (superfície dentro de painel, nunca --bg-elevated cru: com papel
     de parede ligado o cru vira retângulo chapado sobre a foto). A cor do preenchimento é a MESMA
     que nivelDePct dá ao número — se um dia divergirem, a barra estaria contando outra história. */
  .ct-barra { flex: 1; min-width: 32px; height: 3px; border-radius: var(--radius-full);
              background: var(--surface-raised); overflow: hidden; }
  .ct-barra i { display: block; height: 100%; border-radius: var(--radius-full);
                background: var(--accent); }
  .ct-barra i.alerta { background: var(--warning); }
  .ct-barra i.cheio { background: var(--error); }
  .ct-escolha-txt { color: var(--text-secondary); font-size: 12px; align-self: center; }
  /* Formulário da chave: superfície própria (é área de entrada), por isso --surface-inset e não
     --bg-base cru — com papel de parede ligado, o cru vira retângulo chapado sobre a foto. */
  .ct-form {
    margin-top: var(--space-3); padding: var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: 10px;
    background: var(--surface-inset);
  }
  .ct-form-leg { color: var(--text-muted); font-size: 12px; margin: 0 0 var(--space-3); }
  /* Container query, não media query: quem aperta a linha é a largura do PAINEL. */
  .ct-form-linha { display: flex; gap: var(--space-3); }
  @container (max-width: 460px) { .ct-form-linha { flex-direction: column; } }
  .ct-campo-l { display: flex; flex-direction: column; gap: 4px; margin-bottom: var(--space-3); min-width: 0; }
  .ct-campo-l.larga { flex: 1; }
  .ct-campo-l > span { font-size: 11.5px; color: var(--text-secondary); }
  /* Formulário do cookie: mora DENTRO da linha da credencial (largura cheia, abaixo dela) porque
     é configuração daquela credencial, não uma tela nova. Mesma superfície do formulário da chave. */
  .ct-cookie {
    grid-column: 1 / -1; margin-top: var(--space-2); padding: var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: 10px;
    background: var(--surface-inset);
  }
</style>