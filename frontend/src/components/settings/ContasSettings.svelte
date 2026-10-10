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
  import { onDestroy, tick, untrack } from 'svelte';
import { apagarConta, claudeAccountFolder, sairConta, apagarProvedorKimi, deleteEngine, deleteEngineForServer, deleteCodexAccountForServer, isAbortError, isTimeoutError, accountDeletedNotice, type AccountDeleteResult, type Motor, type EnginesResponse } from '@hangar/core';
  import { formatarIntervalo } from '../../lib/contaEstado';
  import { listarCredenciais, definirApelido, definirCookie, consumirRedefinicaoCodex,
    novaChaveIdempotente, type Credencial } from '../../lib/credenciais';
  import { iniciarLogin, passoLogin, confirmarLogin, cancelarLogin, type PassoLogin, type ResultadoLogin } from '../../lib/loginConta';
  import { copyText } from '../../lib/clipboard';
  import { initials } from '@hangar/core';
  import { diaDoReset, faltaPara, janelaLonga, nivelDePct, VELHA_APOS_S, motivoParado,
    motivoSessaoViva } from '../../lib/cota';
  import NovaCredencialSheet from './NovaCredencialSheet.svelte';
  import CodexContaLogin from './CodexContaLogin.svelte';
  import { credentialAuth, credentialGroup, codexCliAusente } from '@hangar/core';
  import { listServers, getActiveId } from '../../lib/auth';
  import ProvedorIcone from '../icons/ProvedorIcone.svelte';
  import { serverIdentidade, type Server } from '../../lib/auth';
  import { createQuery } from '@tanstack/svelte-query';
  import { clienteQuery, credenciais, motores as qMotoresDef } from '../../lib/queries';
  import MotorForm from './MotorForm.svelte';
  import EscopoChip from './EscopoChip.svelte';
  import * as m from '../../paraglide/messages';

  // Contrato do apiTarget (o mesmo de ServidoresSettings): null = servidor ATIVO (API global com
  // self-heal de 401); Server explícito = a máquina que o ?srv= escolheu. Quem resolve é o App
  // (targetConfig): com ?srv=B a aba tem de falar com B, não com o ativo — o bloqueador da
  // revisão final (apagar/Entrar agiam na máquina errada, com o cabeçalho nomeando outra).
  interface Props {
    apiTarget: Server | null;
  }
  let { apiTarget }: Props = $props();
  const codexServer = $derived(apiTarget ?? listServers().find((s) => s.id === getActiveId()) ?? null);
  let codexLogin = $state<string | null>(null);
  let codexHerdar = $state<string | null>(null);

  // A lista vem do cache compartilhado: reabrir Contas entrega o que já estava lá e revalida por
  // baixo, em vez de esvaziar a tela e esperar a leitura das cotas (medida em ~2,5s com o cache do
  // backend frio).
  const qContas = createQuery(() => credenciais(apiTarget), () => clienteQuery);
  const contas = $derived(qContas.data ?? []);
  const carregando = $derived(qContas.isPending);
  // Conta-base do app ainda sem login: a folha de conta nova manda entrar nela em vez de criar pasta.
  const baseDeslogada = $derived(contas.find((c) => c.tipo === 'claude' && c.ativa
    && c.login?.estado === 'ok' && !c.login.loggedIn) ?? null);
  const erro = $derived(qContas.error ? ((qContas.error as Error).message || String(qContas.error)) : '');

  // O engines.json entra na mesma tela: é o que faz uma chave de API mostrar o modelo no card e
  // abrir "Modelo e opções" ali mesmo, sem uma segunda tela. Só quem está no mapa tem motor —
  // credencial de cota do Kimi CLI/Codex (id `kimi:`/`codex:`) não está, e com o arquivo
  // corrompido o mapa vem vazio de propósito.
  const qMotores = createQuery(() => qMotoresDef(apiTarget), () => clienteQuery);
  const motoresMapa = $derived(qMotores.data?.motores ?? {});
  const erroMotores = $derived(qMotores.error ? ((qMotores.error as Error).message || String(qMotores.error)) : '');
  const nomeMotorDe = (c: Credencial) => (c.id.startsWith('chave:') ? c.id.slice('chave:'.length) : null);
  const motorDe = (c: Credencial): Motor | undefined => {
    const n = nomeMotorDe(c);
    return n ? motoresMapa[n] : undefined;
  };
  // id da credencial com o bloco de motor aberto (um por vez, como o cookie e o renomear).
  let motorAberto = $state<string | null>(null);

  // Três seções, não uma lista de doze linhas: conta do Claude, modelo pro Claude Code (chave que
  // ESTÁ no engines.json) e chave de outro agente. A cópia que o `agentes_sync` grava no
  // config.toml do Kimi tem o mesmo nome do motor — daí o id `kimi:<nome>` que `cotas.py` monta —
  // e não é uma credencial a mais: ela aparece como linha dentro do card do modelo.
  const ehCopiaSync = (c: Credencial) => c.id.startsWith('kimi:') && c.id.slice('kimi:'.length) in motoresMapa;
  const secaoClaude = $derived(contas.filter((c) => credentialGroup(c) === 'subscription'
    || (c.tipo === 'codex' && credentialAuth(c) === 'none')));
  const secaoModelos = $derived(contas.filter((c) => {
    const n = nomeMotorDe(c);
    return !!n && n in motoresMapa;
  }));
  const secaoOutros = $derived(contas.filter(
    (c) => !secaoClaude.includes(c) && !secaoModelos.includes(c) && !ehCopiaSync(c),
  ));
  const copiaKimiDe = (c: Credencial) => {
    const n = nomeMotorDe(c);
    return n ? contas.some((x) => x.id === `kimi:${n}`) : false;
  };
  // Dias até o refresh token vencer (teto, como o CLI conta). null = arquivo sem o prazo.
  const diasLoginDe = (c: Credencial): number | null => {
    const exp = c.login?.refreshExpiresAt;
    if (exp == null || !c.login?.loggedIn) return null;
    return Math.ceil((exp - Date.now() / 1000) / 86400);
  };
  // Provedor = o HOST do endereço, não a URL inteira: o caminho (/coding/v1) é ruído na linha e
  // URL inválida não pode derrubar a tela.
  const hostDe = (url: string | null | undefined) => {
    try { return url ? new URL(url).host : ''; } catch { return ''; }
  };
  // Os três chips saem do motor. Janela e subagente têm texto próprio pro caso "não definido" —
  // um campo vazio ali significa "o padrão do provedor" / "o mesmo do principal", não "nada".
  const chipsDe = (mo: Motor) => [
    mo.context_window ? m.contas_chip_janela({ n: `${Math.round(mo.context_window / 1000)}k` })
                      : m.contas_chip_janela_padrao(),
    mo.subagent_model ? m.contas_chip_subagente({ id: mo.subagent_model })
                      : m.contas_chip_subagente_igual(),
    // O default do backend é ligado: só `false` explícito desliga.
    mo.adaptive_thinking !== false ? m.contas_chip_raciocinio_on() : m.contas_chip_raciocinio_off(),
  ];
  // `alvo` é a máquina que RESPONDEU o PUT (o MotorForm o captura antes do await): a resposta
  // entra sob a chave dela, esteja ela na tela ou não — nunca sob a do alvo atual.
  function motorSalvo(novos: Record<string, Motor>, alvo: Server | null) {
    // O PUT devolve o mapa inteiro: escrever no cache poupa o GET e o card muda na hora. Não fecha
    // o bloco — a sincronização nos outros agentes roda depois e o resultado dela mora lá.
    clienteQuery.setQueryData(qMotoresDef(alvo).queryKey, (velho: EnginesResponse | undefined) => ({
      motores: novos, arquivo_corrompido: false, arquivo_caminho: velho?.arquivo_caminho ?? '',
    }));
    // Rótulo e endereço podem ter mudado — e eles saem da lista de credenciais. Só ela: refazer
    // os motores aqui jogaria fora o que o PUT acabou de devolver. E só se a máquina que
    // respondeu ainda é a da tela: a lista do alvo novo já foi pedida pela troca.
    if (serverIdentidade(alvo) === serverIdentidade(apiTarget)) void qContas.refetch();
  }

  // Criar: um botão só ("+ Nova conta") e a escolha do TIPO acontece depois do clique — pedido
  // do usuário: "eu seleciono qual vou criar na hora". Dois botões lado a lado obrigavam a
  // decidir antes de saber que existiam duas coisas.
  let novo = $state<null | 'escolha'>(null);
  // Renomear: o apelido é do app, não do disco — renomear pasta mexeria em caminho que um CLI
  // vivo tem aberto, e renomear motor quebraria o `hangar-engine --exec <nome>` de sessão rodando.
  let renomeando = $state<string | null>(null);   // id da credencial em edição
  let apelidoTexto = $state('');
  let salvandoApelido = $state(false);
  // Cookie do painel do OpenCode: ele não tem rota de cota (ver backend/app/opencode_cota.py),
  // então a leitura é a página do painel. É uma linha do card, só na credencial que aceita —
  // oferecer o campo pra quem tem rota de verdade seria prometer trabalho inútil.
  let cookieDe = $state<string | null>(null);   // id da credencial com o formulário aberto
  let cookieWs = $state('');
  let cookieValor = $state('');
  let salvandoCookie = $state(false);
  // Apagar: "Remover" nomeado no card abre a confirmação inline; confirmar apaga e recarrega.
  let confirmando = $state<string | null>(null);  // id da credencial com a confirmação aberta
  let apagando = $state(false);
  // Guardar as conversas vem marcado; só a conta em que a pessoa desmarcou apaga tudo.
  let apagarConversasDe = $state<string | null>(null);
  // Abrir, trocar ou fechar a confirmação volta a guardar: a escolha vale só para aquela abertura.
  $effect.pre(() => {
    void confirmando;
    apagarConversasDe = null;
  });
  let saindoDe = $state<string | null>(null);  // id da conta Claude com a confirmação de Sair aberta
  let saindo = $state(false);
  let sairErro = $state('');
  let aviso = $state('');
  let avisoErro = $state(false);
  let resetConfirmando = $state<string | null>(null);
  let resetTentativa = $state<{
    conta: string; credito: string | null; chave: string;
  } | null>(null);
  let resetConsumindo = $state(false);

  // Densidade da lista: completa (cards com e-mail, disco e barras) ou compacta (uma linha por
  // credencial, % de cota sem barra). Preferência do aparelho, não do servidor — como o tema.
  let compacta = $state(false);
  try { compacta = localStorage.getItem('cp_contas_compacta') === '1'; } catch { /* storage bloqueado */ }
  function alternarDensidade() {
    compacta = !compacta;
    try { localStorage.setItem('cp_contas_compacta', compacta ? '1' : '0'); } catch { /* idem */ }
  }

  // Botão de atualizar do cabeçalho: `atualizadoEm` alimenta o "atualizado há X", e o relógio
  // de 30 s faz o texto envelhecer sozinho. O intervalo morre no onDestroy junto com o poll
  // do login — nada fica rodando depois que a aba desmonta.
  let atualizando = $state(false);
  // Quando o dado da chave atual foi lido — vem da própria query, não de um state à parte: assim o
  // "atualizado há X" pertence ao alvo, e trocar de máquina já mostra o carimbo daquela, sem reset
  // manual. 0 = ainda não há dado.
  const atualizadoEm = $derived(qContas.dataUpdatedAt || null);
  let agora = $state(Date.now());
  const agoraCota = $derived(agora / 1000);
  const relogio = setInterval(() => { agora = Date.now(); }, 30_000);
  const idadeAtualizacao = $derived(
    atualizadoEm == null ? '' : formatarIntervalo(Math.max(0, (agora - atualizadoEm) / 1000)),
  );

  // Assinatura preservada: as mutações (apelido, cookie, criar, apagar, login) seguem chamando
  // `carregar(geracao)`. O guard de geração saiu porque a chave da query já isola o alvo — uma
  // resposta do servidor anterior não tem onde escrever na tela do novo.
  function carregar(_meu?: number) {
    void qContas.refetch();
    // Também os motores: é ele quem roda depois de criar e de apagar uma chave, e o botão
    // "Modelo e opções" tem de nascer junto com a credencial, não 60 s depois.
    void qMotores.refetch();
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
    const alvo = apiTarget;
    atualizando = true;
    aviso = '';
    avisoErro = false;
    try {
      const lista = await listarCredenciais(alvo, true);
      if (meu !== geracao) return;
      // Escreve no cache em vez de num state: a leitura forçada é a mais nova que existe, e quem
      // reabrir a aba depois tem de ver ELA, não a de antes do botão.
      clienteQuery.setQueryData(credenciais(alvo).queryKey, lista);
      agora = Date.now();
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
  let primeiraIdentidade = true;

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
    loginConta = null; loginSucesso = null; loginCopiado = false; loginConsultaErro = false; loginFalhou = false;
    ultimaContaConectada = null;
    loginErro = ''; loginEnviando = false; loginIniciando = false; loginParado = false;
    aviso = ''; avisoErro = false;
    confirmando = null;
    saindoDe = null; saindo = false; sairErro = '';
    resetConfirmando = null; resetTentativa = null; resetConsumindo = false;
    renomeando = null; apelidoTexto = ''; salvandoApelido = false;
    cookieDe = null; cookieWs = ''; cookieValor = ''; salvandoCookie = false;
    motorAberto = null;
    novo = null;
    apagando = false;
    // Refresh em voo pertence ao alvo que saiu: o finally de atualizar() só limpa o flag se a
    // geração for a mesma, então sem este reset o botão ficava desabilitado PRA SEMPRE no alvo
    // novo (achado da revisão). O carimbo "atualizado há" não precisa mais de reset: ele sai da
    // query, que é por alvo.
    atualizando = false;
    // Na MONTAGEM não força nada: quem decide buscar é o cache (staleTime), senão reabrir a aba
    // pagava um GET mesmo com o dado fresco na mão — o spinner some, o pedido não. O refetch fica
    // pro que este efeito também cobre: mesmo servidor com TOKEN novo (credencial consertada),
    // que cai na mesma chave e portanto não seria rebuscado sozinho.
    if (!primeiraIdentidade) carregar(meu);
    primeiraIdentidade = false;
  });

  // O backend relê a cota a cada 5 min, então "velha" aqui é o mesmo corte da faixa do rodapé
  // (10 min = duas tentativas falhadas), não "não é deste segundo": com o corte de 1 minuto a
  // coluna inteira nasceria esmaecida em toda montagem da tela.
  const leituraFresca = (c: Credencial) =>
    c.cota?.estado === 'lida' && (c.cota.idade_s == null || c.cota.idade_s <= VELHA_APOS_S);
  const resetDaJanela = (resetTs?: number | null) => {
    const texto = janelaLonga(resetTs ?? null, agoraCota)
      ? diaDoReset(resetTs ?? null, agoraCota)
      : faltaPara(resetTs ?? null, agoraCota);
    return texto ? m.cota_reinicia({ n: texto }) : '';
  };
  const semanalDe = (c: Credencial) => c.cota?.janelas.find((j) => j.rotulo === '7d');
  const creditoDe = (c: Credencial) =>
    c.cota?.reset_credits?.credits?.find((credito) => credito.status === 'available') ?? null;
  const expiracaoDosCreditos = (c: Credencial) => {
    const datas = (c.cota?.reset_credits?.credits ?? [])
      .filter((credito) => credito.status === 'available' && credito.expires_at != null)
      .map((credito) => credito.expires_at as number);
    if (!datas.length) return '';
    const expira = Math.min(...datas);
    return janelaLonga(expira, agoraCota) ? diaDoReset(expira, agoraCota) : faltaPara(expira, agoraCota);
  };

  function abrirRedefinicao(c: Credencial) {
    if (!c.codex_account || (semanalDe(c)?.pct ?? -1) < 100) return;
    resetConfirmando = c.id;
    resetTentativa = {
      conta: c.codex_account,
      credito: creditoDe(c)?.id ?? null,
      chave: novaChaveIdempotente(),
    };
    aviso = '';
    avisoErro = false;
  }

  async function consumirRedefinicao(c: Credencial) {
    if (!resetTentativa || resetTentativa.conta !== c.codex_account || resetConsumindo) return;
    const g = geracao;
    const alvo = apiTarget;
    resetConsumindo = true;
    aviso = '';
    avisoErro = false;
    try {
      const resultado = await consumirRedefinicaoCodex(
        alvo, resetTentativa.conta, resetTentativa.credito, resetTentativa.chave);
      if (g !== geracao) return;
      const aplicada = resultado.outcome === 'reset' || resultado.outcome === 'alreadyRedeemed';
      if (resultado.outcome === 'reset') {
        aviso = m.codex_reset_success();
      } else if (resultado.outcome === 'alreadyRedeemed') {
        aviso = m.codex_reset_already();
      } else {
        aviso = resultado.outcome === 'nothingToReset'
          ? m.codex_reset_nothing() : m.codex_reset_no_credit();
        avisoErro = true;
      }
      resetConfirmando = null;
      resetTentativa = null;
      try {
        const lista = await listarCredenciais(alvo, true);
        if (g === geracao) {
          clienteQuery.setQueryData(credenciais(alvo).queryKey, lista);
          agora = Date.now();
        }
      } catch {
        if (g === geracao) {
          aviso = `${aviso} ${aplicada
            ? m.codex_reset_refresh_failed()
            : m.codex_reset_refresh_failed_neutral()}`;
          avisoErro = true;
        }
      }
    } catch (e) {
      if (g !== geracao) return;
      aviso = e instanceof Error && e.message ? e.message : String(e);
      avisoErro = true;
      // A confirmação e a chave ficam: repetir é a mesma tentativa idempotente.
    } finally {
      if (g === geracao) resetConsumindo = false;
    }
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

  async function apagar() {
    const alvo = confirmando;
    if (!alvo || apagando) return;
    // `confirmando` guarda o ID (chave única da lista); a rota espera o nome NO DISCO. Derivar
    // do estado atual evita mandar um nome velho se a lista mudou entre o clique e o fim da
    // operação. Conta Claude sai da pasta `.claude-<nome>`: o `nome_natural` dela já vem com o
    // apelido, e o DELETE com o apelido não acha conta nenhuma.
    const conta = contas.find((x) => x.id === alvo);
    if (!conta) return;
    const pasta = conta.tipo === 'claude' ? claudeAccountFolder(conta.path) : null;
    const idDisco = conta.id.startsWith('chave:') ? conta.id.slice('chave:'.length) : pasta ?? conta.nome_natural;
    // Geração desta operação: "conta X apagada" pertence à máquina que recebeu o DELETE — troca
    // de ?srv= no meio do voo não deixa o relato da máquina antiga na tela da nova (rodada 2).
    const g = geracao;
    const manter = apagarConversasDe !== conta.id;
    apagando = true;
    aviso = '';
    avisoErro = false;
    // Kimi e chave não têm conversas no servidor: o aviso é só o de conta apagada.
    const guarda = conta.tipo !== 'chave' && !conta.id.startsWith('kimi:');
    try {
      let resultado: AccountDeleteResult | null = null;
      if (conta.id.startsWith('kimi:')) {
        await apagarProvedorKimi(apiTarget, conta.id.slice('kimi:'.length));
      } else if (conta.tipo === 'chave') {
        if (apiTarget) await deleteEngineForServer(apiTarget, idDisco);
        else await deleteEngine(idDisco);
      } else if (conta.tipo === 'codex') {
        if (!codexServer || !conta.codex_account) throw new Error(m.falha_conexao());
        resultado = await deleteCodexAccountForServer(codexServer, conta.codex_account, manter);
      } else {
        resultado = await apagarConta(apiTarget, idDisco, manter);
      }
      if (g !== geracao) return;
      confirmando = null;
      aviso = accountDeletedNotice(conta.nome, guarda && manter, resultado);
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

  // Uma coluna por janela de cota na lista compacta, a mesma em todo card: 5h e 7d primeiro.
  function rotulosDaLista(itens: Credencial[]): string[] {
    const vistos = new Set<string>();
    for (const c of itens) if (c.cota?.estado === 'lida') for (const j of c.cota.janelas) vistos.add(j.rotulo);
    const ordem = (r: string) => (r === '5h' ? 0 : r === '7d' ? 1 : 2);
    return [...vistos].sort((a, b) => ordem(a) - ordem(b));
  }

  async function sair() {
    const conta = contas.find((x) => x.id === saindoDe);
    if (!conta || saindo) return;
    const g = geracao;
    saindo = true;
    sairErro = '';
    aviso = '';
    avisoErro = false;
    try {
      await sairConta(apiTarget, conta.nome_natural);
      if (g !== geracao) return;
      saindoDe = null;
      aviso = m.contas_saiu({ nome: conta.nome });
      // O backend já releu a conta deslogada; a lista comum viria do cache de 5 min.
      clienteQuery.setQueryData<Credencial[]>(credenciais(apiTarget).queryKey, lista => lista?.map<Credencial>(c =>
        c.id === conta.id ? { ...c, login: { estado: 'ok', loggedIn: false }, cota: null } : c));
      const alvo = apiTarget;
      void listarCredenciais(alvo, true).then(lista => {
        if (g === geracao) clienteQuery.setQueryData(credenciais(alvo).queryKey, lista);
      }).catch(() => {
        if (g === geracao) { aviso = m.contas_saiu_atualizar_erro({ nome: conta.nome }); avisoErro = true; }
      });
    } catch (e) {
      if (g !== geracao) return;
      // Na linha da confirmação: o aviso geral fica no pé da lista, fora da vista.
      sairErro = e instanceof Error && e.message ? e.message : m.contas_sair_erro();
    } finally {
      if (g === geracao) saindo = false;
    }
  }

  // ----------------------------------------------------------- login remoto (Task 7)
  // Estado da tentativa em voo: `loginDe` é o LABEL da conta (o nome ~/.claude-<nome>, a
  // chave estável do fluxo). Uma tentativa por conta; o botão Entrar de outra conta fica
  // desabilitado enquanto uma está em voo (uma janela escondida por vez).
  let loginDe = $state<string | null>(null);
  let loginConta = $state.raw<Credencial | null>(null);
  let loginSucesso = $state<ResultadoLogin | null>(null);
  let loginCopiado = $state(false);
  let loginConsultaErro = $state(false);
  let loginFalhou = $state(false);
  let ultimaContaConectada = $state<string | null>(null);
  let superficie: HTMLDivElement;
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
    const tentativa = { ...conta };
    loginConta = tentativa;
    loginSucesso = null;
    loginCopiado = false;
    loginConsultaErro = false;
    loginFalhou = false;
    ultimaContaConectada = null;
    aviso = ''; avisoErro = false;
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
      if (destruido || g !== geracao || loginConta !== tentativa) {
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
      let lendo = false;
      const atual = () => !destruido && g === geracao && loginConta === tentativa && loginDe === conta.nome_natural;
      const consultar = async () => {
        if (!atual() || lendo) return;
        lendo = true;
        try {
          const passo = await passoLogin(alvo, conta.nome_natural);
          if (!atual()) return;
          loginConsultaErro = false;
          if (passo.etapa === 'concluido') {
            await concluirLogin(alvo, tentativa, g, { ok: true, email: passo.email, plano: passo.plano });
          } else {
            loginPasso = passo;
          }
        } catch (e) {
          if (!atual()) return;
          // 409 = o servidor desistiu da tentativa; repetir a consulta só mostraria "aguardando".
          if ((e as { status?: unknown }).status === 409 && !loginSucesso) {
            pararPoll();
            loginFalhou = true;
            loginPasso = { etapa: 'idle' };
            loginErro = e instanceof Error && e.message ? e.message : m.contas_login_nao_confirmado();
          } else {
            loginConsultaErro = true;
          }
        } finally { lendo = false; }
      };
      await consultar();
      if (atual()) loginPoll = setInterval(consultar, 2000);
    } catch (e) {
      if (destruido || g !== geracao || loginConta !== tentativa) return;
      // O que chega: 401 com token (sessao_expirada), o texto do envelope do backend
      // traduzido (mensagemDeErro) ou erro de rede. 'Failed to fetch' cru (fetch abortado,
      // sem resposta) NAO vai pra tela — vira falha de conexao generica. Erro com status
      // (409/404/504) carrega a mensagem traduzida do servidor; o resto e rede.
      const comStatus = e instanceof Error && typeof (e as { status?: unknown }).status === 'number';
      loginErro = comStatus && e instanceof Error && e.message
        ? e.message
        : m.falha_conexao();
    } finally {
      if (loginConta === tentativa) loginIniciando = false;
    }
  }

  function pararPoll() {
    if (loginPoll) {
      clearInterval(loginPoll);
      loginPoll = null;
    }
  }

  // Ponto único de sucesso: código confirmado OU autorização que voltou sozinha pelo navegador.
  async function concluirLogin(alvo: Server | null, tentativa: Credencial, g: number, r: ResultadoLogin) {
    if (loginSucesso) return;
    pararPoll();
    await clienteQuery.cancelQueries({ queryKey: credenciais(alvo).queryKey });
    if (destruido || g !== geracao || loginConta !== tentativa || loginSucesso) return;
    loginDe = null;
    loginCodigo = '';
    loginPasso = { etapa: 'idle' };
    loginSucesso = r;
    ultimaContaConectada = tentativa.id;
    // A confirmação já releu a credencial; a cota anterior não pode pedir outro login.
    clienteQuery.setQueryData<Credencial[]>(credenciais(alvo).queryKey, lista => lista?.map<Credencial>(c =>
      c.id === tentativa.id ? { ...c, login: { estado: 'ok', loggedIn: true, email: r.email, plano: r.plano }, cota: null } : c));
    void listarCredenciais(alvo, true).then(lista => {
      if (!destruido && g === geracao && ultimaContaConectada === tentativa.id) clienteQuery.setQueryData(credenciais(alvo).queryKey, lista);
    }).catch(() => {
      if (!destruido && g === geracao && ultimaContaConectada === tentativa.id) { aviso = m.contas_login_atualizar_erro(); avisoErro = true; }
    });
  }

  async function confirmarEntrar() {
    const conta = loginDe;
    const tentativa = loginConta;
    const alvo = apiTarget;
    if (!conta || !tentativa || loginEnviando || loginParado || loginFalhou || !loginCodigo.trim()) return;
    // Geração desta tentativa: a resposta de um alvo que saiu da tela não escreve aviso/erro nela
    // (o molde é o iniciarEntrar, 60 linhas acima). O teto do confirmar (310s) deixa o voo aberto
    // por minutos — justo a janela em que o usuário espera o OAuth e pode trocar o ?srv=
    // (parecer da rodada 2).
    const g = geracao;
    loginEnviando = true;
    loginErro = '';
    try {
      const r = await confirmarLogin(alvo, conta, loginCodigo);
      if (destruido || g !== geracao || loginConta !== tentativa) return;
      if (!r.ok) throw new Error(m.contas_login_nao_confirmado());
      await concluirLogin(alvo, tentativa, g, r);
    } catch (e) {
      // A autorização pode ter chegado pelo navegador enquanto o código era conferido.
      if (destruido || g !== geracao || loginConta !== tentativa || loginSucesso) return;
      pararPoll();
      loginFalhou = true;
      loginPasso = { etapa: 'idle' };
      loginErro = e instanceof Error && e.message ? e.message : m.falha_conexao();
      // O erro NÃO prova que o login falhou: o teto pode ter cortado com o backend SEGUINDO
      // (a conta acaba logada de verdade). Recarregar a lista para de mentir sozinha — a tela
      // mostra a conta como o servidor a vê (parecer da rodada 1, passo 4).
      await carregar(geracao);
    } finally {
      if (loginConta === tentativa) loginEnviando = false;
    }
  }

  function focarLogin(node: HTMLElement) {
    node.focus({ preventScroll: true });
    node.scrollIntoView?.({ block: 'start' });
  }

  async function copiarLinkLogin() {
    if (!loginPasso.url) return;
    const tentativa = loginConta;
    try {
      await copyText(loginPasso.url);
      if (loginConta === tentativa) loginCopiado = true;
    } catch {
      if (loginConta === tentativa) loginErro = m.contas_login_copiar_erro();
    }
  }

  async function voltarContas() {
    const id = loginConta?.id;
    loginConta = null; loginSucesso = null; loginCodigo = ''; loginErro = '';
    loginConsultaErro = false; loginCopiado = false; loginFalhou = false;
    loginEnviando = false; loginIniciando = false;
    await tick();
    const cartao = [...(superficie?.querySelectorAll<HTMLElement>('[data-conta-id]') ?? [])]
      .find(node => node.dataset.contaId === id);
    cartao?.focus({ preventScroll: true });
    cartao?.scrollIntoView?.({ block: 'nearest' });
  }

  async function cancelarEntrar() {
    const conta = loginDe;
    const tentativa = loginConta;
    if (!conta) { await voltarContas(); return; }
    loginParado = true;
    loginErro = '';
    try {
      await cancelarLogin(apiTarget, conta);
      if (destruido || loginConta !== tentativa) return;
      pararPoll();
      loginDe = null;
      loginPasso = { etapa: 'idle' };
      await voltarContas();
    } catch (e) {
      if (!destruido && loginConta === tentativa) loginErro = e instanceof Error ? e.message : m.falha_conexao();
    } finally {
      if (!destruido && (loginConta === tentativa || !loginConta)) loginParado = false;
    }
  }

  async function recomecarEntrar() {
    const tentativa = loginConta;
    if (!tentativa || loginIniciando || loginEnviando || loginParado) return;
    if (loginDe) {
      loginParado = true;
      try {
        // Falha de rede não comprova que o backend encerrou a tentativa anterior.
        await cancelarLogin(apiTarget, loginDe);
        if (destruido || loginConta !== tentativa) return;
        loginDe = null;
      } catch (e) {
        if (!destruido && loginConta === tentativa) loginErro = e instanceof Error ? e.message : m.falha_conexao();
        return;
      } finally { if (loginConta === tentativa) loginParado = false; }
    }
    await iniciarEntrar(tentativa);
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

<div class="ct-superficie" bind:this={superficie}>
  {#if loginConta}
    <section class="ct-login" aria-labelledby="contas-login-titulo">
      <div class="ct-login-identidade">
        <ProvedorIcone tipo="claude" iniciais={initials(loginConta.nome)} size={30} />
        <span>{codexServer?.label ? m.contas_login_servidor({ nome: codexServer.label }) : m.contas_login_servidor_atual()}</span>
      </div>
      {#if loginSucesso}
        <div class="ct-login-sucesso" role="status">
          <span class="ct-login-check" aria-hidden="true">✓</span>
          <h2 id="contas-login-titulo" tabindex="-1" use:focarLogin>{m.contas_login_conectada()}</h2>
          <p class="ct-login-conta">{loginConta.nome}</p>
          <p class="ct-login-descricao">{m.contas_login_pronta()}</p>
          <dl class="ct-login-dados">
            {#if loginSucesso.email}<div><dt>{m.contas_login_email()}</dt><dd>{loginSucesso.email}</dd></div>{/if}
            {#if loginSucesso.plano}<div><dt>{m.contas_login_plano()}</dt><dd>{loginSucesso.plano}</dd></div>{/if}
          </dl>
        </div>
        {#if avisoErro}<p class="ct-aviso erro" role="alert">{aviso}</p>{/if}
        <button type="button" class="ct-btn primario ct-login-concluir" onclick={voltarContas}>{m.contas_login_concluir()}</button>
      {:else}
        <h2 id="contas-login-titulo" tabindex="-1" use:focarLogin>{m.contas_login_titulo({ nome: loginConta.nome })}</h2>
        {#if loginErro}<p class="ct-aviso erro" role="alert">{loginErro}</p>{/if}
        {#if loginConsultaErro}<p class="ct-aviso erro" role="alert">{m.falha_conexao()}</p>{/if}
        {#if loginIniciando || (loginDe && !loginPasso.url && !loginFalhou)}
          <p class="ct-login-progresso" role="status"><span class="ct-login-spinner" aria-hidden="true"></span>{m.contas_login_preparando()}</p>
        {:else if loginDe && !loginFalhou}
          <form onsubmit={(e) => { e.preventDefault(); confirmarEntrar(); }}>
            <div class="ct-passo">
              <span class="ct-num" aria-hidden="true">1</span>
              <div class="ct-passo-txt">
                <b>{m.contas_login_autorizar()}</b>
                <p>{m.contas_login_autorizar_ajuda()}</p>
                <div class="ct-login-links">
                  <a class="ct-link ct-btn primario" href={loginEnviando || loginParado ? undefined : loginPasso.url ?? undefined}
                    aria-disabled={loginEnviando || loginParado} target="_blank" rel="noopener noreferrer">{m.contas_login_abrir()}</a>
                  <button type="button" class="ct-btn" onclick={copiarLinkLogin} disabled={loginEnviando || loginParado}>
                    {loginCopiado ? m.toast_copiado() : m.contas_login_copiar_link()}
                  </button>
                </div>
              </div>
            </div>
            <div class="ct-passo">
              <span class="ct-num" aria-hidden="true">2</span>
              <div class="ct-passo-txt">
                <label for="contas-login-codigo">{m.contas_login_codigo()}</label>
                <p>{m.contas_passo3()}</p>
                <input id="contas-login-codigo" class="ct-campo-cod" type="text" autocomplete="one-time-code"
                  autocapitalize="none" spellcheck={false} placeholder={m.contas_codigo_placeholder()}
                  bind:value={loginCodigo} disabled={loginEnviando || loginParado} />
              </div>
            </div>
            {#if loginEnviando}
              <p class="ct-login-progresso" role="status"><span class="ct-login-spinner" aria-hidden="true"></span>{m.contas_login_confirmando()}</p>
            {/if}
            <div class="ct-rodape login">
              <button type="button" class="ct-btn" onclick={cancelarEntrar} disabled={loginParado}>{m.comum_cancelar()}</button>
              <button type="submit" class="ct-btn primario" disabled={loginEnviando || loginParado || !loginCodigo.trim()}>{m.contas_confirmar_codigo()}</button>
            </div>
          </form>
        {/if}
        {#if !loginDe || !loginPasso.url || loginFalhou}
          <div class="ct-rodape login">
            <button type="button" class="ct-btn" onclick={cancelarEntrar} disabled={loginIniciando || loginParado}>{m.comum_voltar()}</button>
            {#if !loginIniciando && (!loginDe || loginFalhou)}
              <button type="button" class="ct-btn primario" onclick={recomecarEntrar} disabled={loginParado}>{m.lista_tentar_novamente()}</button>
            {/if}
          </div>
        {/if}
      {/if}
    </section>
  {:else}
  <!-- Cabeçalho da coleção: título + "atualizado há X" + botão de atualizar na mesma linha
       (referência Cloudscape/AWS: refresh no cabeçalho, timestamp ao lado, lista visível
       durante a busca). O ícone é SVG traçado 2, como o lápis. -->
  <div class="ct-cab">
    <p class="st-secao ct-topo">{m.contas_secao_lista()} <EscopoChip escopo="servidor" /></p>
    {#if atualizadoEm != null}
      <span class="ct-atualizado" aria-live="polite">{m.contas_atualizado_ha({ n: idadeAtualizacao })}</span>
    {/if}
    <!-- Criar é a ação primária da tela e vive no cabeçalho: no rodapé ela era o 13º item, depois
         de todas as credenciais. Com o engines.json quebrado fica inerte — a folha abre POR CIMA
         da lista e esconderia o aviso que explica por que criar agora apagaria os outros motores.
         Pendente ou com erro fica inerte pelo mesmo motivo: sem a lista de nomes na mão o
         formulário não tem como recusar um nome curto já ocupado, e o PUT substitui calado. -->
    <button type="button" class="ct-add" onclick={() => (novo = 'escolha')}
      aria-label={m.contas_add_aria()}
      disabled={!!novo || qMotores.isPending || !!qMotores.error || !!qMotores.data?.arquivo_corrompido}>+ {m.contas_add()}</button>
    <button type="button" class="ct-refresh" onclick={alternarDensidade}
      aria-pressed={compacta}
      aria-label={compacta ? m.contas_ver_completa() : m.contas_ver_compacta()}
      title={compacta ? m.contas_ver_completa() : m.contas_ver_compacta()}>
      {#if compacta}
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
          stroke-width="2" stroke-linecap="round" aria-hidden="true">
          <rect x="3" y="3.5" width="18" height="7" rx="2" /><rect x="3" y="13.5" width="18" height="7" rx="2" />
        </svg>
      {:else}
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
          stroke-width="2" stroke-linecap="round" aria-hidden="true">
          <path d="M4 5h16" /><path d="M4 10h16" /><path d="M4 15h16" /><path d="M4 20h16" />
        </svg>
      {/if}
      <!-- Texto ao lado do ícone, nas duas larguras: o que existia era `title`, e no toque não há
           hover — quem usa o celular nunca via o que o botão faz. -->
      <span class="ct-refresh-txt">{compacta ? m.contas_densidade_completa() : m.contas_densidade_compacta()}</span>
    </button>
    <button type="button" class="ct-refresh" onclick={atualizar}
      disabled={atualizando || carregando} aria-label={m.contas_atualizar()}
      title={m.contas_atualizar()}>
      <svg class:girando={atualizando} width="15" height="15" viewBox="0 0 24 24" fill="none"
        stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M21 12a9 9 0 1 1-2.64-6.36" /><path d="M21 3v6h-6" />
      </svg>
      <span class="ct-refresh-txt">{m.cota_atualizar()}</span>
    </button>
  </div>
  <p class="ct-legenda">{m.contas_legenda()}</p>

  {#if qMotores.data?.arquivo_corrompido}
    <!-- Não é "nenhum motor": o arquivo existe e não pôde ser lido — pode estar escondendo motores
         reais atrás do erro. Sem este aviso, apagar e recriar uma chave apagaria os outros calado. -->
    <p class="ct-aviso erro" role="alert">
      {m.config_motores_nao_consegui_1()} <code>{qMotores.data.arquivo_caminho}</code>{m.config_motores_nao_consegui_2()}
    </p>
  {:else if erroMotores}
    <p class="ct-aviso erro" role="alert">{m.config_motores_erro_carregar()}: {erroMotores}</p>
  {/if}

  {#if carregando}
    <!-- Esqueleto na altura do card real (ícone + nome + duas sublinhas): a lista nasce no lugar
         em vez de pular quando o dado chega. -->
    <div class="ct-skel" aria-busy="true" aria-label={m.comum_carregando()}>
      {#each [0, 1, 2, 3] as k (k)}
        <div class="ct-card ct-skel-item">
          <div class="ct-top">
            <span class="ct-skel-ico"></span>
            <span class="ct-skel-txt">
              <span class="ct-skel-bar" style="width: 42%"></span>
              <span class="ct-skel-bar ct-skel-bar--sub" style="width: 66%"></span>
              <span class="ct-skel-bar ct-skel-bar--sub" style="width: 28%"></span>
            </span>
          </div>
        </div>
      {/each}
    </div>
  {:else if erro}
    <p class="ct-aviso erro" role="alert">{erro}</p>
  {:else}
    <!-- As três seções aparecem SEMPRE depois da carga, mesmo vazias: uma seção que some deixa
         a pessoa sem saber que aquele lugar existe (é onde a coisa nova vai aparecer). -->
    {@render secao(m.contas_secao_claude(), m.contas_secao_claude_leg(), secaoClaude, m.comum_nada_encontrado())}
    {@render secao(m.contas_secao_modelos(), m.contas_secao_modelos_leg(), secaoModelos, m.contas_secao_modelos_vazio())}
    {@render secao(m.contas_secao_outros(), m.contas_secao_outros_leg(), secaoOutros, m.contas_secao_outros_vazio())}
  {/if}

  {#snippet secao(titulo: string, legenda: string, itens: Credencial[], vazio: string)}
    <section class="ct-grupo">
      <p class="st-secao">{titulo}</p>
      <p class="ct-legenda">{legenda}</p>
      {#if itens.length}
        <!-- Cards separados (não uma caixa com divisórias): cada credencial é uma unidade que se
             lê de uma vez — nome, e-mail, nome no disco e as barras de limite. `compacta` troca o
             card por uma linha de escaneamento (sem sublinhas nem barras, só o %). -->
        {@const rotulos = rotulosDaLista(itens)}
        <div class="ct-lista" class:compacta style:--slots={Math.max(1, rotulos.length)}>
          {#each itens as conta (conta.id)}{@render cartao(conta, rotulos)}{/each}
        </div>
      {:else}
        <p class="ct-vazio">{vazio}</p>
      {/if}
    </section>
  {/snippet}

  {#snippet cartao(conta: Credencial, rotulos: string[])}
        <!-- Marcas de uso ("roda o Claude Code", "cota pelo painel") saem da linha do NOME e viram
             texto na linha do subtítulo. Pílula fica só para o TIPO da credencial e para "em uso":
             com quatro pílulas na mesma linha, o nome — que é o que distingue uma linha da outra —
             sumia no meio do enfeite. Mesmas chaves de sempre, nenhum texto novo. -->
        {@const marcas = [
          ...(conta.usos.includes('claude_code') ? [m.contas_usa_claude_code()] : []),
          ...(conta.cookie_definido ? [m.contas_cookie_definido()] : []),
          ...(conta.gerenciada === false ? [m.contas_chave_do_agente()] : []),
        ]}
        <!-- Só o NOME NO DISCO, não o caminho inteiro: o prefixo (/home/jefferson/) é igual em
             todas e a elipse cortava justamente o final — que é o que distingue uma conta da
             outra, e é o nome que --conta/hangar-conta aceitam. Igual ao nome exibido (sem
             apelido) some: seria o mesmo texto duas vezes. -->
        {@const dir = conta.tipo === 'chave'
          ? (conta.nome !== conta.nome_natural ? conta.nome_natural : '')
          : (() => {
              const b = (conta.path ?? '').split('/').filter(Boolean).pop() ?? '';
              return b && b !== conta.nome ? b : '';
            })()}
        {@const motor = motorDe(conta)}
        {@const motorEmEdicao = motorAberto === conta.id}
        <div class="ct-card" tabindex="-1" data-conta-id={conta.id}
          class:recem-conectada={ultimaContaConectada === conta.id}
          class:fora={conta.login?.estado === 'ok' && !conta.login.loggedIn}>
          <div class="ct-top">
          <span class="ct-ico">
            <ProvedorIcone tipo={conta.tipo} baseUrl={conta.base_url} iniciais={initials(conta.nome)}
              size={compacta ? 24 : 30} />
          </span>
          <span class="ct-txt">
            <span class="ct-nome-l">
              {#if renomeando === conta.id}
                <!-- A frase vive AQUI, não num `title` do lápis: quem precisa saber que o nome é só
                     local é quem está digitando o nome, e no toque não há hover. Mesmo desenho da
                     instrução do cookie, que também mora dentro do formulário dela. -->
                <p class="ct-form-leg ct-renomear-leg" id="rn-como-{conta.id}">{m.contas_renomear_so_aqui()}</p>
                <!-- svelte-ignore a11y_autofocus -->
                <input class="ct-campo ct-campo-nome" type="text" autofocus bind:value={apelidoTexto}
                  aria-label={m.contas_renomear({ nome: conta.nome })} aria-describedby="rn-como-{conta.id}"
                  disabled={salvandoApelido}
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
                <!-- Lápis em SVG traçado 2, como o resto do app (components/icons,
                     DesktopSessionContext): glifo de texto (✎) no meio de uma UI de ícone
                     desenhado muda de peso e de linha de base conforme a fonte do sistema.
                     O rótulo visível ao lado vale nas duas larguras: o aria-label sozinho não
                     chega a quem enxerga, e no toque não há hover pra um `title`. -->
                <button type="button" class="ct-lapis" aria-label={m.contas_renomear({ nome: conta.nome })}
                  title={m.contas_renomear_so_aqui()}
                  onclick={() => { renomeando = conta.id; apelidoTexto = conta.apelido ?? ''; }}>
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                    stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="M12 20h9" />
                    <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" />
                  </svg>
                  <span class="ct-lapis-txt">{m.ctx_renomear()}</span>
                </button>
                <!-- Sem selo "Claude · assinatura" em toda linha: o ícone já diz o tipo.
                     Etiqueta só pra chave de API (a exceção), na ponta da linha. "em uso" é
                     pontinho verde, não pílula — um selo a menos disputando o nome. -->
                {#if conta.ativa}<span class="ct-emuso">{m.contas_em_uso()}</span>{/if}
                {#if conta.tipo === 'claude' && conta.login?.estado === 'ok' && conta.login.loggedIn && conta.cota?.estado !== 'expirada'}
                  <span class="ct-conectada">{m.contas_conectada()}</span>
                {/if}
              {/if}
            </span>
            {#if conta.tipo === 'codex'}
              {#if credentialAuth(conta) !== 'none' || (conta.login?.loggedIn && conta.login.plano)}
                <span class="ct-sub-l ct-codex-meta">
                  {#if credentialAuth(conta) !== 'none'}
                    <span class="ct-sub">{credentialAuth(conta) === 'oauth' ? m.codex_ui_oauth()
                      : credentialAuth(conta) === 'api_key' ? m.contas_tipo_chave()
                      : codexCliAusente(conta) ? m.codex_ui_cli_ausente()
                      : m.codex_ui_unknown()}</span>
                  {/if}
                  {#if conta.login?.loggedIn && conta.login.plano}
                    <span class="ct-sub">· {conta.login.plano}</span>
                  {/if}
                </span>
              {/if}
              {#if conta.login?.loggedIn && conta.login.email}
                <span class="ct-sub ct-codex-meta">{conta.login.email}</span>
              {:else if conta.login?.estado === 'ok' && !conta.login.loggedIn}
                <span class="ct-sub fraco ct-codex-meta">{m.contas_nao_conectada()}</span>
              {/if}
              {#if conta.ativa || conta.codex_account || dir}
                <span class="ct-sub-l ct-codex-meta">
                  {#if conta.ativa}<span class="ct-sub fraco">{m.criar_padrao()}</span>
                  {:else if conta.codex_sync === 'ready'}<span class="ct-sub fraco">{m.codex_ui_inherited()}</span>
                  {:else if conta.codex_sync === 'running'}<span class="ct-sub fraco">{m.codex_ui_preparing()}</span>
                  {:else if conta.codex_sync === 'idle'}<span class="ct-sub fraco">{m.codex_ui_nao_herdada()}</span>
                  {:else if conta.codex_sync}<span class="ct-sub fraco">{m.codex_ui_prepare_error()}</span>{/if}
                  {#if dir}<span class="ct-dir">{conta.ativa || conta.codex_account ? '· ' : ''}{dir}</span>{/if}
                </span>
              {/if}
            {/if}
            {#if conta.tipo === 'chave' && !motorEmEdicao}
              <!-- A chave NUNCA volta inteira do servidor (credenciais._mascarar): o que a tela
                   mostra é o rabicho, o bastante pra saber QUAL chave é sem expor a chave. Com o
                   bloco aberto some daqui: endereço, chave e modelo já estão nos campos abaixo, e
                   repeti-los seria o mesmo dado em dois lugares. -->
              <span class="ct-sub">{conta.base_url ?? ''}{conta.chave_mascarada ? ` · ${conta.chave_mascarada}` : ''}</span>
              {#if motor}
                <!-- Quem é o provedor: o host, não a URL de novo. É o que responde "de quem é
                     esse modelo?" sem obrigar a ler o caminho da API. -->
                {#if hostDe(motor.base_url)}<span class="ct-sub ct-provedor">{hostDe(motor.base_url)}</span>{/if}
                <!-- Só o modelo: a janela do contexto mora no chip abaixo, e ter as duas era o
                     mesmo número duas vezes na mesma caixa. -->
                <span class="ct-sub ct-modelo">{motor.model}</span>
                <span class="ct-chips">
                  {#each chipsDe(motor) as chip (chip)}<span class="ct-chip">{chip}</span>{/each}
                </span>
                <!-- A cópia que o sync gravou no Kimi mora AQUI, não num card vazio ao lado: ela
                     não é outra credencial, é este motor visto de dentro do outro agente. -->
                {#if copiaKimiDe(conta)}
                  <span class="ct-sub fraco">{m.contas_sync_kimi({ nome: nomeMotorDe(conta) ?? '' })}</span>
                {/if}
              {/if}
            {:else if conta.tipo !== 'codex' && conta.login?.estado === 'ok' && conta.login.loggedIn && conta.login.email}
              {@const dias = diasLoginDe(conta)}
              {#if dias == null}
                <span class="ct-sub">{conta.login.email}</span>
              {:else}
                <!-- O prazo mora na linha do e-mail, não em linha própria: é um detalhe da
                     conexão, não outra identidade. O "?" abre a explicação — o vencimento é
                     regra do Claude Code, não algo que o app pudesse renovar sozinho. -->
                <details class="ct-vence-l">
                  <summary class="ct-sub-l" aria-label={m.contas_login_vence_ajuda_aria()}>
                    <!-- O separador mora com o e-mail: no compacto os dois somem juntos e o prazo
                         não fica com um "·" pendurado na frente. -->
                    <span class="ct-sub">{conta.login.email} ·</span>
                    <span class="ct-sub ct-vence-txt" class:fraco={dias > 3} class:ct-vence={dias <= 3}>
                      {dias > 0 ? m.contas_login_vence({ n: dias }) : m.contas_login_vencido()}
                    </span>
                    <span class="ct-ajuda-q" aria-hidden="true">?</span>
                  </summary>
                  <span class="ct-ajuda">{m.contas_login_vence_hint()}</span>
                </details>
              {/if}
            {:else if conta.tipo !== 'codex' && conta.login?.estado === 'ok' && !conta.login.loggedIn}
              <span class="ct-sub fraco">{m.contas_nao_conectada()}</span>
            {/if}
            <!-- Marcas e caminho dividem UMA linha (densidade, 18/08): eram duas, e nenhuma das
                 duas é a identidade da conta — quem identifica é o nome e o e-mail acima. -->
            {#if conta.tipo !== 'codex' && (marcas.length || dir)}
              <span class="ct-sub-l">
                {#if marcas.length}<span class="ct-marcas">{marcas.join(' · ')}</span>{/if}
                {#if dir}<span class="ct-dir">{dir}</span>{/if}
              </span>
            {/if}
          </span>

          <!-- A etiqueta só onde ela informa: na seção de outros agentes, onde uma chave de API
               convive com o login do Codex. Dentro de "Modelos pro Claude Code" (o card COM motor)
               toda linha é uma chave, e a etiqueta repetia o título da seção. -->
          {#if conta.tipo === 'chave' && !motor}<span class="ct-tag">{m.contas_tipo_chave()}</span>{/if}

          <!-- Modo compacto: a cota vira rótulo+% na MESMA linha do nome, sem barra. -->
          {#if compacta && conta.cota && conta.cota.estado === 'lida' && conta.cota.janelas.length}
            <span class="ct-mini-cotas" class:velha={!leituraFresca(conta)}>
              {#each conta.cota.janelas as j (j.rotulo)}
                <span class="ct-mini-jan" style:grid-column={rotulos.indexOf(j.rotulo) + 1}>{j.rotulo} <b class={nivelDePct(j.pct)}>{Math.round(j.pct)}%</b>
                  {#if resetDaJanela(j.reset_ts)}<small class="ct-mini-reset">{resetDaJanela(j.reset_ts)}</small>{/if}
                </span>
              {/each}
              <!-- Dado velho com sinal TEXTUAL também: opacidade sozinha não chega a leitor de
                   tela e some em tela clara (achado da revisão do commit). -->
              {#if !leituraFresca(conta)}
                <span class="ct-mini-idade">{m.cota_ultima_leitura({ n: formatarIntervalo(conta.cota.idade_s) })}</span>
              {/if}
            </span>
          {/if}

          <!-- Um envelope só para as ações: Entrar, Editar e Remover, todos nomeados. -->
          <span class="ct-acoes">
            {#if conta.tipo === 'codex' && conta.codex_account && credentialAuth(conta) === 'none'}
              <button type="button" class="ct-acao primaria" onclick={() => { codexHerdar = null; codexLogin = conta.codex_account ?? null; }}>{m.contas_entrar()}</button>
            {:else if conta.tipo === 'codex' && conta.codex_account && conta.codex_sync === 'idle' && conta.login?.loggedIn}
              <!-- O "Depois" do login desembarca aqui: herdar da padrão quando quiser. -->
              <button type="button" class="ct-acao" onclick={() => { codexHerdar = conta.codex_account ?? null; codexLogin = codexHerdar; }}>{m.codex_ui_herdar_botao()}</button>
            {/if}
            <!-- Só a conta adicional (~/.codex-<nome>) sai; a padrão é o ~/.codex da máquina. -->
            <!-- O `aria-label` diz QUAL credencial: a lista tem vários "Remover" idênticos, e quem
                 navega por elementos ouviria só "Remover" em todos (mesmo padrão do lápis). -->
            {#if conta.tipo === 'codex' && conta.codex_account && !conta.ativa}
              <button type="button" class="ct-acao" aria-label={m.contas_remover_aria({ nome: conta.nome })}
                onclick={() => (confirmando = conta.id)}>{m.lista_remover()}</button>
            {/if}
            {#if conta.tipo === 'claude' && ((conta.login?.estado === 'ok' && !conta.login.loggedIn) || conta.cota?.estado === 'expirada')}
              <button type="button" class="ct-acao primaria"
                aria-label={m.contas_entrar_titulo({ nome: conta.nome })}
                disabled={!!loginDe || loginIniciando}
                onclick={() => iniciarEntrar(conta)}>{m.contas_entrar()}</button>
            {/if}
            <!-- Mesmo fluxo do Entrar (login_conta compara o token anterior e confirma pela
                 credencial nova): renovar É entrar de novo, só muda o momento. -->
            {#if conta.tipo === 'claude' && conta.login?.estado === 'ok' && conta.login.loggedIn
                 && conta.cota?.estado !== 'expirada' && (diasLoginDe(conta) ?? Infinity) <= 3}
              <button type="button" class="ct-acao primaria"
                aria-label={m.contas_entrar_titulo({ nome: conta.nome })}
                disabled={!!loginDe || loginIniciando}
                onclick={() => iniciarEntrar(conta)}>{m.contas_renovar_login()}</button>
            {/if}

            {#if motor}
              <!-- Editar e Remover NOMEADOS: as duas ações do dia a dia de um modelo estavam
                   escondidas atrás de um kebab e de um rótulo ("Modelo e opções") que não dizia
                   qual delas ele abria. -->
              <button type="button" class="ct-acao ct-modelo-btn" aria-expanded={motorEmEdicao}
                onclick={() => (motorAberto = motorEmEdicao ? null : conta.id)}
                >{motorEmEdicao ? m.sessao_fechar() : m.contas_motor_editar()}</button>
              <button type="button" class="ct-acao" aria-label={m.contas_remover_aria({ nome: conta.nome })}
                onclick={() => (confirmando = conta.id)}>{m.lista_remover()}</button>
            {/if}

            <!-- O kebab saiu: eram no máximo três ações, e uma delas (apagar) é a mesma que o card
                 do modelo e a conta Codex adicional já mostram nomeada. A conta do Claude
                 gerenciada passa a usar ESSE botão, com a mesma confirmação inline. -->
            {#if conta.tipo === 'claude' && conta.gerenciada !== false && conta.login?.estado === 'ok' && conta.login.loggedIn && conta.cota?.estado !== 'expirada'}
              <button type="button" class="ct-acao" aria-label={m.contas_sair_aria({ nome: conta.nome })}
                disabled={saindo} onclick={() => { saindoDe = conta.id; sairErro = ''; confirmando = null; }}>{m.contas_sair()}</button>
            {/if}
            {#if conta.tipo !== 'codex' && !motor && conta.gerenciada !== false}
              <button type="button" class="ct-acao" aria-label={m.contas_remover_aria({ nome: conta.nome })}
                onclick={() => { confirmando = conta.id; saindoDe = null; }}>{m.lista_remover()}</button>
            {/if}
          </span>
          </div>
          {#if conta.tipo === 'codex' && codexLogin === conta.codex_account && codexServer}
            <div class="ct-form">
              <CodexContaLogin server={codexServer} accountId={conta.codex_account ?? undefined}
                herdar={codexHerdar === conta.codex_account}
                oncomplete={() => { codexLogin = null; codexHerdar = null; carregar(); }} />
              <button type="button" class="ct-acao" onclick={() => { codexLogin = null; codexHerdar = null; }}>{m.sessao_fechar()}</button>
            </div>
          {/if}

          <!-- As barras do limite em largura cheia, abaixo do nome: a MESMA leitura da faixa do
               rodapé (uma fonte só). O medidor é o MESMO dado do número, em forma de comprimento:
               um dígito só se compara lendo, uma barra se compara de relance. Credencial sem
               número aparece dizendo por quê — some-la esconderia justo a que precisa de atenção
               (nos dois modos, compacta inclusive). Por ser repetição do número, é aria-hidden. -->
          {#if !compacta && conta.cota && conta.cota.estado === 'lida' && conta.cota.janelas.length}
            <span class="ct-cota" class:velha={!leituraFresca(conta)}>
              {#each conta.cota.janelas as j (j.rotulo)}
                <span class="ct-jan">
                  <span class="ct-jan-rot">{j.rotulo}</span>
                  {#if resetDaJanela(j.reset_ts)}<span class="ct-jan-reset">{resetDaJanela(j.reset_ts)}</span>{/if}
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
          {:else if !(conta.cota && conta.cota.estado === 'lida' && conta.cota.janelas.length)}
            {#if conta.tipo === 'codex' && codexCliAusente(conta)}
              <!-- Sem o CLI não há login possível: "precisa entrar" mandaria fazer o impossível. -->
            {:else if conta.cota && (conta.cota.estado === 'expirada' || conta.cota.estado === 'sem_credencial')}
              <!-- sessao-viva NÃO é "abra uma sessão" — a sessão já está aberta (foi como o
                   usuário leu a frase estando dentro dela, 19/08). Quem renova é o CLI dela. -->
              <span class="ct-semleitura"
                >{motivoSessaoViva(conta.cota.motivo) ? m.cota_sessao_viva()
                  : motivoParado(conta.cota.motivo) ? m.cota_conta_parada() : m.cota_precisa_entrar()}</span>
            {:else}
              <span class="ct-semleitura">{m.contas_sem_cota()}</span>
            {/if}
          {/if}

          {#if conta.tipo === 'codex' && conta.codex_account
            && (conta.cota?.reset_credits?.available_count ?? 0) > 0}
            {@const quantidade = conta.cota?.reset_credits?.available_count ?? 0}
            {@const semanal = semanalDe(conta)}
            {@const expira = expiracaoDosCreditos(conta)}
            <div class="ct-reset">
              <span class="ct-reset-info">
                {quantidade === 1 ? m.codex_reset_one() : m.codex_reset_many({ n: quantidade })}
                {#if expira}<small>{m.codex_reset_expires({ n: expira })}</small>{/if}
              </span>
              <button type="button" class="ct-acao ct-reset-btn"
                disabled={!semanal || semanal.pct < 100 || resetConsumindo}
                onclick={() => abrirRedefinicao(conta)}>{m.codex_reset_use()}</button>
              {#if !semanal}
                <span class="ct-reset-reason">{m.codex_reset_weekly_unavailable()}</span>
              {:else if semanal.pct < 100}
                <span class="ct-reset-reason">{m.codex_reset_weekly_remaining({ pct: Math.round(semanal.pct) })}</span>
              {/if}
            </div>
            {#if resetConfirmando === conta.id}
              <div class="ct-reset-confirm">
                <p>{m.codex_reset_confirm()}</p>
                <div>
                  <button type="button" class="ct-confirma-btn primario"
                    disabled={resetConsumindo} onclick={() => consumirRedefinicao(conta)}>
                    {resetConsumindo ? '…' : m.codex_reset_confirm_action()}
                  </button>
                  <button type="button" class="ct-confirma-btn" disabled={resetConsumindo}
                    onclick={() => { resetConfirmando = null; resetTentativa = null; }}>
                    {m.comum_cancelar()}
                  </button>
                </div>
              </div>
            {/if}
          {/if}

          <!-- Cookie: linha do card, não item de menu — quem cai aqui precisa saber ONDE copiar o
               cookie, e a instrução não cabe num item de menu. Só na credencial que aceita. -->
          {#if conta.aceita_cookie && cookieDe !== conta.id}
            <div class="ct-cookie-linha">
              <span class="ct-cookie-como">{m.contas_cookie_como()}</span>
              <span class="ct-cookie-acoes">
                <button type="button" class="ct-acao"
                  onclick={() => { cookieDe = conta.id; cookieWs = ''; cookieValor = ''; }}
                  >{m.contas_cookie_acao()}</button>
                {#if conta.cookie_definido}
                  <button type="button" class="ct-acao" disabled={salvandoCookie}
                    onclick={() => salvarCookie(conta, true)}>{m.contas_cookie_apagar()}</button>
                {/if}
              </span>
            </div>
          {/if}

          {#if cookieDe === conta.id}
            <div class="ct-cookie">
              <p class="ct-form-leg">{m.contas_cookie_legenda()}</p>
              <!-- O "onde copiar" fica aqui dentro também: a linha do card some quando o formulário
                   abre, e era justamente ao preencher o campo que a instrução fazia falta. -->
              <p class="ct-form-leg" id="ck-como-{conta.id}">{m.contas_cookie_como()}</p>
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
                    aria-describedby="ck-como-{conta.id}"
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

          {#if motor && motorEmEdicao}
            {@const nomeMotor = nomeMotorDe(conta) ?? ''}
            <MotorForm {apiTarget} nome={nomeMotor} {motor}
              onSalvo={motorSalvo} onFechar={() => (motorAberto = null)} />
          {/if}

          {#if confirmando === conta.id}
            {@const guardaConversas = conta.tipo !== 'chave' && !conta.id.startsWith('kimi:')}
            <div class="ct-confirma">
              {#if guardaConversas}
                <span class="ct-confirma-txt">{m.contas_apagar_pergunta({ nome: conta.nome })}</span>
                <label class="ct-confirma-manter">
                  <input type="checkbox" checked={apagarConversasDe !== conta.id} disabled={apagando}
                    aria-describedby={apagarConversasDe === conta.id ? `ct-perde-conversas-${conta.id}` : undefined}
                    onchange={(e) => (apagarConversasDe = e.currentTarget.checked ? null : conta.id)} />
                  {m.contas_juntar_conversas()}
                </label>
                {#if apagarConversasDe === conta.id}
                  <span class="ct-confirma-aviso" id={`ct-perde-conversas-${conta.id}`} role="status">{m.contas_apagar_conversas_aviso()}</span>
                {/if}
              {:else}
                <span class="ct-confirma-txt">
                  {m.comum_apagar()} <strong>{conta.nome}</strong> {m.criar_apagar_fim()}
                </span>
              {/if}
              {#if conta.tipo === 'chave'}<span class="ct-confirma-aviso">{m.config_motores_sessoes_abertas()}</span>{/if}
              <button type="button" class="ct-confirma-btn perigo" onclick={apagar}
                disabled={apagando}>{apagando ? '…' : m.comum_apagar()}</button>
              <button type="button" class="ct-confirma-btn"
                onclick={() => (confirmando = null)} disabled={apagando}>{m.comum_cancelar()}</button>
            </div>
          {/if}

          {#if saindoDe === conta.id}
            <div class="ct-confirma">
              <span class="ct-confirma-txt">{m.contas_sair_pergunta({ nome: conta.nome })}</span>
              <button type="button" class="ct-confirma-btn perigo" onclick={sair}
                disabled={saindo}>{saindo ? '…' : m.contas_sair()}</button>
              <button type="button" class="ct-confirma-btn"
                onclick={() => (saindoDe = null)} disabled={saindo}>{m.comum_cancelar()}</button>
              {#if sairErro}<span class="ct-confirma-aviso erro" role="alert">{sairErro}</span>{/if}
            </div>
          {/if}
        </div>
  {/snippet}

  {#if novo}
    <NovaCredencialSheet {apiTarget} nomesExistentes={Object.keys(motoresMapa)}
      baseDeslogada={baseDeslogada?.nome ?? null}
      onEntrarBase={() => { const b = baseDeslogada; novo = null; if (b) void iniciarEntrar(b); }}
      onFechar={() => { novo = null; carregar(); }}
      onCriada={() => { void carregar(geracao); }} />
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
  {/if}
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
  /* Ícone + rótulo: a pílula deixou de ser redonda porque agora carrega texto. Altura e cor
     continuam as mesmas do cabeçalho. */
  .ct-refresh { flex-shrink: 0; height: 28px; min-height: 0; min-width: 0;
                display: inline-flex; align-items: center; gap: 5px;
                padding: 0 var(--space-2); border-radius: var(--radius-full);
                border: 1px solid var(--border-subtle); background: transparent;
                color: var(--text-muted); cursor: pointer; }
  .ct-refresh-txt { font-size: var(--text-2xs); white-space: nowrap; }
  @media (hover: hover) and (pointer: fine) {
    .ct-refresh:hover { color: var(--text-primary); border-color: var(--border-default); }
  }
  /* `spin` é o keyframes global do app.css. Gira só o SVG: o botão parado com o ícone
     girando lê como "trabalhando" sem mexer o layout do cabeçalho. */
  .ct-refresh svg.girando { animation: spin 0.8s linear infinite; }

  .ct-legenda { margin: 0 var(--space-2) var(--space-3); color: var(--text-muted);
                font-size: var(--text-xs); line-height: 1.45; }
  /* Uma seção por natureza de credencial. O respiro entre elas é o que separa "conta do Claude"
     de "modelo" sem precisar de linha divisória. */
  .ct-grupo { margin-top: var(--space-4); }
  .ct-grupo .st-secao { margin: 0 var(--space-2) var(--space-1); }
  /* Seção vazia não some — mas o vazio dela não é aviso: é o lugar reservado. */
  .ct-vazio { margin: 0 var(--space-2); color: var(--text-muted); font-size: var(--text-xs);
              line-height: 1.45; }
  /* Botão de criar no cabeçalho: mesma altura dos redondos ao lado, para a linha não crescer. */
  .ct-add { flex-shrink: 0; height: 28px; min-height: 0; padding: 0 var(--space-3);
            border-radius: var(--radius-full); border: 1px solid var(--border-default);
            background: var(--surface-raised); color: var(--text-primary);
            font-size: var(--text-xs); font-family: inherit; cursor: pointer;
            transition: transform 160ms ease-out; }
  .ct-add:not(:disabled):active { transform: scale(0.97); }
  .ct-add:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .ct-add:disabled { opacity: .55; cursor: default; }
  @media (hover: hover) and (pointer: fine) {
    .ct-add:hover:not(:disabled) { border-color: var(--accent); color: var(--accent); }
  }
  /* Os três parâmetros que definem como o motor roda: janela, subagente e raciocínio. Chip, e não
     mais uma linha de texto, porque são três valores curtos que se leem juntos. */
  .ct-chips { display: flex; flex-wrap: wrap; gap: var(--space-1); margin-top: var(--space-1); }
  .ct-chip { padding: 1px 7px; border-radius: var(--radius-full);
             background: var(--surface-raised); color: var(--text-muted);
             font-size: var(--text-3xs); white-space: nowrap; }
  .ct-default { align-self: flex-start; }
  .ct-sep { height: 1px; background: var(--border-subtle); margin: var(--space-4) 0 var(--space-3); }
  .ct-aviso { margin: var(--space-2); color: var(--text-muted); font-size: var(--text-sm); }
  .ct-aviso.erro { color: var(--error); }

  /* Cards separados por credencial (não uma caixa com divisórias): cada um é uma unidade de
     leitura — identidade em cima (ícone · nome · ações) e as barras de limite em largura cheia
     embaixo. `.compacta` troca o card por uma linha de escaneamento. */
  .ct-skel { display: flex; flex-direction: column; gap: var(--space-2); }
  .ct-skel-item { pointer-events: none; }
  .ct-skel-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 7px; }
  .ct-skel-ico, .ct-skel-bar {
    display: block; border-radius: 6px;
    background: linear-gradient(90deg, color-mix(in srgb, var(--text-muted) 14%, transparent) 25%, color-mix(in srgb, var(--text-muted) 28%, transparent) 50%, color-mix(in srgb, var(--text-muted) 14%, transparent) 75%);
    background-size: 200% 100%;
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .ct-skel-ico { width: 30px; height: 30px; border-radius: var(--radius-full); flex-shrink: 0; }
  .ct-skel-bar { height: 12px; }
  .ct-skel-bar--sub { height: 9px; }
  .ct-lista { display: flex; flex-direction: column; gap: var(--space-2); }
  .ct-lista.compacta { gap: var(--space-1); }
  .ct-card { position: relative; background: var(--surface-card);
             border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
             padding: var(--space-3); }
  .compacta .ct-card { padding: var(--space-2) var(--space-3); }
  /* Hover só onde hover EXISTE: no toque ele gruda no último elemento tocado e lê como estado
     ativo falso (regra do app: @media (hover) and (pointer), não detecção de SO). */
  @media (hover: hover) and (pointer: fine) {
    .ct-card:hover { border-color: var(--border-default); }
  }
  .ct-top { display: flex; align-items: flex-start; gap: var(--space-3); }
  /* No compacto a linha pode embrulhar: conta do Claude cabe numa linha (sem etiqueta); chave de
     API com 2-3 janelas de cota não — sem o wrap, o NOME era esmagado a uma coluna de 1 caractere
     (o flex encolhe o texto antes de quebrar a linha). A cota que não coube desce inteira pra
     segunda linha, que é melhor que um nome ilegível. */
  .compacta .ct-top { align-items: center; flex-wrap: wrap; row-gap: 2px; }
  /* Quem embrulha primeiro é a COTA, nunca as ações: medido no celular, o botão sozinho caía pra
     uma linha de 44px no pé do card e a linha lia quebrada. Ordem explícita: ações ficam na 1ª
     linha (canto direito), a mini-cota desce inteira quando não cabe. */
  .compacta .ct-acoes { order: 2; }
  .compacta .ct-tag { order: 3; }
  .compacta .ct-mini-cotas { order: 4; }
  /* Piso de largura pro nome: com `overflow-wrap: anywhere` o min-content do texto é UM
     caractere, então o flex não disparava o wrap e o nome virava uma coluna vertical. Com piso,
     quem desce pra linha de baixo é a cota/etiqueta que não coube. */
  .compacta .ct-txt { min-width: 14ch; }
  .compacta .ct-sub, .compacta .ct-sub-l { display: none; }
  /* No compacto some a INSTRUÇÃO, não a ação: o modo é de escaneamento (mesma regra dos
     subtítulos acima), mas o botão do cookie continua alcançável — esconder a ação de novo
     seria o kebab com outro nome. */
  .compacta .ct-cookie-como { display: none; }
  .ct-ico { display: grid; place-items: center; }
  .ct-card.fora .ct-ico { opacity: .55; }
  .ct-acoes { display: flex; align-items: center; gap: var(--space-2); flex-shrink: 0; }

  .ct-txt { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
  .ct-nome-l { display: flex; align-items: center; flex-wrap: wrap; row-gap: 1px;
               column-gap: var(--space-2); min-width: 0; }
  /* O nome é a IDENTIDADE da linha e não trunca mais: com elipse, cinco contas viravam
     "claude-200-…" indistinguíveis (e a pergunta da tela — "qual é ESSA credencial?" — ficava
     sem resposta). Quebra pra segunda linha quando precisa; a etiqueta segue na primeira linha
     pelo flex-wrap da mãe. Quem continua sem quebrar é a ETIQUETA. */
  .ct-nome {
    color: var(--text-primary); font-size: var(--text-sm); font-weight: 600;
    min-width: 0; overflow-wrap: anywhere;
  }
  /* "em uso": pontinho verde + texto, não pílula — um selo a menos disputando o nome. */
  .ct-emuso { flex-shrink: 0; display: inline-flex; align-items: center; gap: 5px;
              color: var(--success); font-size: var(--text-2xs); }
  .ct-emuso::before { content: ''; width: 5px; height: 5px; border-radius: 50%;
                      background: var(--success); }
  /* Etiqueta só da chave de API (a exceção da lista): o ícone do Claude já diz o tipo das
     demais, e um selo em TODA linha era ruído repetido. */
  .ct-tag { flex-shrink: 0; margin-top: 3px; font-size: var(--text-3xs); white-space: nowrap;
            padding: 2px 7px; border-radius: var(--radius-full);
            background: rgba(232,145,45,.16); color: var(--warning); }
  /* Cota do modo compacto: rótulo+% na linha do nome, sem barra, números tabulares. */
  .ct-mini-cotas { flex-shrink: 0; align-self: center; display: flex; gap: var(--space-3);
                   font-size: var(--text-2xs); color: var(--text-muted); white-space: nowrap;
                   font-variant-numeric: tabular-nums; }
  .ct-mini-cotas.velha { opacity: .55; }
  .ct-mini-jan b { font-weight: 600; color: var(--text-secondary); }
  .ct-mini-jan b.alerta { color: var(--warning); }
  /* Cheio: cor E sublinhado — no compacto não há barra como segundo canal. */
  .ct-mini-jan b.cheio { color: var(--error); text-decoration: underline; }
  .ct-mini-idade { font-size: var(--text-3xs); color: var(--text-muted); }
  .ct-mini-jan { display: inline-flex; align-items: baseline; gap: 4px; }
  .ct-mini-reset { font-size: var(--text-3xs); font-weight: 400; color: var(--text-muted); }
  /* Compacta com largura: uma grade só pra lista inteira (subgrid), então nome, botões e cada
     janela de cota ficam na mesma coluna em todo card, qualquer que seja o tamanho do texto. */
  @container (min-width: 621px) {
    .ct-lista.compacta { display: grid; row-gap: var(--space-1); column-gap: var(--space-3);
      grid-template-columns: auto minmax(14ch, 1fr) auto auto repeat(var(--slots), auto); }
    .ct-lista.compacta .ct-card { display: grid; grid-column: 1 / -1; grid-template-columns: subgrid; }
    .ct-lista.compacta .ct-card > * { grid-column: 1 / -1; }
    .ct-lista.compacta .ct-top { display: grid; grid-template-columns: subgrid; align-items: center; }
    .ct-lista.compacta .ct-ico { grid-column: 1; }
    .ct-lista.compacta .ct-txt { grid-column: 2; }
    .ct-lista.compacta .ct-tag { grid-column: 3; margin-top: 0; }
    .ct-lista.compacta .ct-acoes { grid-column: 4; justify-content: flex-end; }
    .ct-lista.compacta .ct-mini-cotas { grid-column: 5 / -1; display: grid; grid-template-columns: subgrid; }
    .ct-lista.compacta .ct-mini-idade { grid-column: 1 / -1; grid-row: 2; }
    /* Nome sempre sozinho na 1ª linha: Renomear, "em uso" e Conectada descem juntos em todo card. */
    .ct-lista.compacta .ct-nome { flex-basis: 100%; }
  }
  .ct-sub { flex-shrink: 0; color: var(--text-secondary); font-size: var(--text-xs); }
  .ct-sub.fraco { color: var(--text-muted); }
  .ct-sub.ct-vence { color: var(--warning); }
  /* <details> nativo, como a ajuda do MotorForm: teclado e estado de graça. A linha inteira é o
     summary; o "?" só marca que há explicação atrás. */
  .ct-vence-l { min-width: 0; }
  /* No compacto o subtítulo some (modo de escaneamento), MENOS o prazo do login: é o único dado
     dali que pede uma ação com data marcada. Fica só ele — e-mail e explicação continuam sendo
     detalhe do modo completo. */
  .compacta .ct-vence-l > .ct-sub-l { display: flex; }
  .compacta .ct-vence-l .ct-vence-txt { display: inline; }
  /* Sem a explicação, o "?" seria um botão morto: os dois somem juntos. */
  .compacta .ct-ajuda, .compacta .ct-ajuda-q { display: none; }
  .compacta .ct-vence-l > summary { cursor: default; }
  .ct-vence-l > summary { list-style: none; cursor: pointer; }
  .ct-vence-l > summary::-webkit-details-marker { display: none; }
  .ct-ajuda-q {
    display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0;
    width: 16px; height: 16px; border: 1px solid var(--border-subtle); border-radius: var(--radius-full);
    color: var(--text-muted); font-size: 10px; line-height: 1;
  }
  .ct-vence-l[open] .ct-ajuda-q { color: var(--text-primary); }
  .ct-ajuda { display: block; margin-top: var(--space-1); font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; }
  /* O modelo é um id de máquina (`kimi-k3`), como o caminho no disco: monoespaçado. */
  .ct-modelo { font-family: var(--font-mono); }
  /* O que a credencial serve ("roda o Claude Code", "cota pelo painel"): texto, não pílula. */
  .ct-marcas { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4; }
  .ct-sub-l { display: flex; align-items: baseline; gap: var(--space-2); min-width: 0; }
  /* O caminho cede primeiro (elipse) em vez de quebrar em várias linhas: `word-break: break-all`
     num caminho longo devolvia justamente as duas linhas que esta densidade veio tirar. */
  .ct-dir { font-family: var(--font-mono); font-size: var(--text-2xs); color: var(--text-muted);
            min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* Barras do limite em largura cheia abaixo do nome — o dado velho tem que PARECER velho
     (régua "dado velho parece velho"; a faixa de cota da Task 9 vive aqui dentro). */
  .ct-cota { display: flex; flex-direction: column; gap: 3px; margin-top: var(--space-2); }
  .ct-idade { font-size: 11px; color: var(--text-muted); opacity: 0.9; }
  .ct-cota.velha .ct-idade { opacity: 0.75; }
  .ct-cota.velha { opacity: 0.55; }
  .ct-semleitura { display: block; margin-top: var(--space-1); font-size: var(--text-2xs);
                   color: var(--text-muted); line-height: 1.4; }

  .ct-acao { flex-shrink: 0; height: 30px; min-height: 0; padding: 0 var(--space-3);
             border-radius: var(--radius-sm); border: 1px solid var(--border-subtle);
             background: var(--surface-raised); color: var(--text-primary); font-size: var(--text-xs);
             font-family: inherit; cursor: pointer; }
  .ct-acao.primaria { background: var(--accent); border-color: var(--accent); color: #fff; }
  .ct-modelo-btn { white-space: nowrap; }

  /* Feedback de toque (escala sutil no :active): todo botão da tela confirma na hora que o dedo
     chegou — 160ms ease-out, curva de saída, nada de ease-in. Desabilitado não responde, porque
     inerte não pode parecer vivo. */
  .ct-btn, .ct-acao, .ct-confirma-btn, .ct-mini, .ct-refresh, .ct-lapis {
    transition: transform 160ms ease-out;
  }
  .ct-btn:not(:disabled):active, .ct-acao:not(:disabled):active,
  .ct-confirma-btn:not(:disabled):active,
  .ct-mini:not(:disabled):active, .ct-refresh:not(:disabled):active,
  .ct-lapis:not(:disabled):active {
    transform: scale(0.97);
  }

  /* Linha do cookie: instrução à esquerda, ações à direita, largura cheia abaixo da identidade —
     mesma posição do formulário que ela abre. */
  .ct-cookie-linha { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2);
                     margin-top: var(--space-2); }
  .ct-cookie-como { flex: 1; min-width: 12ch; font-size: var(--text-2xs);
                    color: var(--text-muted); line-height: 1.4; }
  .ct-cookie-acoes { display: flex; gap: var(--space-2); flex-shrink: 0; }

  .ct-confirma { display: flex; align-items: center; gap: var(--space-2);
                 flex-wrap: wrap; margin-top: var(--space-2); }
  .ct-confirma-txt { font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.4; }
  .ct-confirma-txt strong { color: var(--text-primary); }
  .ct-confirma-btn { height: 30px; min-height: 0; padding: 0 var(--space-3); border-radius: var(--radius-sm);
                     border: 1px solid var(--border-subtle); background: var(--surface-raised);
                     color: var(--text-primary); font-size: var(--text-xs); font-family: inherit;
                     cursor: pointer; }
  .ct-confirma-btn.perigo { color: var(--error); border-color: var(--border-default); }
  .ct-confirma-btn.primario { color: var(--text-inverse); background: var(--accent);
                             border-color: var(--accent); }
  /* Linha inteira própria (o `.ct-confirma` embrulha): o aviso é ressalva, não parte da pergunta. */
  .ct-confirma-aviso { flex-basis: 100%; font-size: var(--text-2xs); color: var(--text-muted); }
  .ct-confirma-manter { flex-basis: 100%; display: flex; align-items: center; gap: var(--space-2);
                        font-size: var(--text-xs); color: var(--text-secondary); }
  .ct-confirma-aviso.erro { font-size: var(--text-xs); color: var(--error); }

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

  .ct-login { display: flex; flex-direction: column; gap: var(--space-5); box-sizing: border-box;
              width: 100%; max-width: 560px; margin: 0 auto; padding: var(--space-6) var(--space-3);
              background: transparent; }
  .ct-login-identidade { display: flex; align-items: center; justify-content: center;
                         gap: var(--space-2); color: var(--text-muted); font-size: var(--text-xs); }
  .ct-login h2 { margin: 0; text-align: center; font-size: var(--text-xl); line-height: 1.3; }
  .ct-login form { min-width: 0; }
  .ct-passo { display: flex; gap: var(--space-3); align-items: flex-start; }
  .ct-passo + .ct-passo { margin-top: var(--space-6); }
  .ct-num { flex-shrink: 0; width: 28px; height: 28px; border-radius: var(--radius-full);
            background: var(--accent-dim); color: var(--accent); font-size: var(--text-sm); font-weight: 600;
            display: grid; place-items: center; margin-top: 1px; }
  .ct-passo-txt { flex: 1; min-width: 0; font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.5; }
  .ct-passo-txt b, .ct-passo-txt label { color: var(--text-primary); font-weight: 600; }
  .ct-passo-txt p { margin: var(--space-1) 0 0; }
  .ct-login-links { display: flex; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-3); }
  .ct-login .ct-btn { display: inline-flex; align-items: center; justify-content: center;
                     min-height: 44px; height: auto; padding: var(--space-2) var(--space-3); }
  .ct-link { text-decoration: none; }
  .ct-link[aria-disabled="true"] { opacity: .55; pointer-events: none; }
  .ct-campo-cod { width: 100%; height: 44px; margin-top: var(--space-2); padding: 0 var(--space-3);
              background: var(--surface-inset); border: 1px solid var(--border-default);
              border-radius: var(--radius-sm); color: var(--text-primary);
              font-family: var(--font-mono); font-size: var(--text-sm); box-sizing: border-box; }
  .ct-btn.primario { background: var(--accent); border-color: var(--accent); color: var(--text-inverse); }
  .ct-login-progresso { display: flex; align-items: center; justify-content: center; gap: var(--space-2);
                       margin: var(--space-4) 0; color: var(--text-secondary); font-size: var(--text-sm); }
  .ct-login-spinner { width: 16px; height: 16px; border: 2px solid var(--border-default);
                      border-top-color: var(--accent); border-radius: var(--radius-full); animation: spin .8s linear infinite; }
  .ct-login-sucesso { text-align: center; }
  .ct-login-check { display: grid; place-items: center; width: 64px; height: 64px;
                    margin: 0 auto var(--space-4); border-radius: var(--radius-full); font-size: 32px;
                    color: var(--success); background: color-mix(in srgb, var(--success) 12%, transparent); }
  .ct-login-conta { font-weight: 600; margin: var(--space-3) 0 var(--space-1); }
  .ct-login-descricao { margin: 0; color: var(--text-secondary); font-size: var(--text-sm); }
  .ct-login-dados { display: grid; gap: var(--space-3); margin: var(--space-5) 0 0;
                    padding: var(--space-4) 0; border-top: 1px solid var(--border-subtle); }
  .ct-login-dados div { display: grid; grid-template-columns: 70px 1fr; gap: var(--space-3); text-align: left; }
  .ct-login-dados dt { color: var(--text-muted); font-size: var(--text-sm); }
  .ct-login-dados dd { margin: 0; overflow-wrap: anywhere; font-size: var(--text-sm); }
  .ct-conectada { color: var(--success); font-size: var(--text-2xs); font-weight: 500; }
  .ct-card.recem-conectada { border-color: var(--success); }
  .ct-card:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  @container (max-width: 420px) {
    .ct-login { padding: var(--space-4) 0; }
    .ct-login-links { flex-direction: column; }
    .ct-login-links .ct-btn { width: 100%; box-sizing: border-box; }
  }

  /* Teclado: quem chega no Tab tem de VER onde está. Sem isto o lápis (fundo transparente) só
     mostrava o anel padrão do navegador, que some no fundo escuro. */
  .ct-lapis:focus-visible, .ct-acao:focus-visible,
  .ct-btn:focus-visible, .ct-mini:focus-visible,
  .ct-confirma-btn:focus-visible, .ct-refresh:focus-visible {
    outline: 2px solid var(--accent); outline-offset: 2px;
  }
  /* Desabilitado PARECE desabilitado — o Entrar fica inerte enquanto há outro login em curso, e
     os botões do formulário do cookie enquanto ele salva. */
  .ct-lapis:disabled, .ct-acao:disabled,
  .ct-btn:disabled, .ct-mini:disabled, .ct-confirma-btn:disabled, .ct-refresh:disabled {
    opacity: .55; cursor: default;
  }

  /* Tangível no celular: target de toque >= 44px quando o painel aperta (o mock é desktop
     1440px, onde 30px é confortável em mouse). */
  @container (max-width: 620px) {
    .ct-acao, .ct-confirma-btn { height: 44px; min-height: 44px; }
    /* O lápis também: o `min-height: 0` lá em cima é o que dá a densidade no desktop (mouse),
       e sem esta linha ele descia a 18px no celular — abaixo do alvo tangível, justo num botão
       que fica colado no nome. A linha do nome volta a 44px aqui, e é o certo: quem lê no
       celular precisa acertar o dedo, não caber mais uma conta na tela. */
    .ct-lapis { min-height: 44px; }
    /* O refresh do cabeçalho também é alvo de dedo no estreito. A largura sai do rótulo. */
    .ct-refresh { height: 36px; }
    /* Com rótulo, os três botões do cabeçalho não cabem ao lado do título no estreito: o último
       saía pra fora do painel. O título passa a ocupar a primeira linha inteira e os botões
       descem juntos — mesma saída do `.mq-caixas` da lista de máquinas. */
    .ct-cab { flex-wrap: wrap; }
    .ct-cab .st-secao { flex-basis: 100%; }
    /* Mesmo remédio na linha do cookie: com os dois botões (227px) sobravam 86px pra instrução,
       que virava uma coluna de seis linhas. Em largura cheia ela lê em duas, e os botões descem.
       O `min-width: 12ch` fica: na faixa larga a instrução divide a linha com eles de propósito. */
    .ct-cookie-como { flex-basis: 100%; }
    .ct-btn { height: 44px; }
    /* Com "Modelo e opções" na linha, a chave de API passou a ter etiqueta + Editar + Remover, todos
       `flex-shrink: 0`, e no estreito não sobrava largura pro nome: como o `.ct-nome` tem
       `overflow-wrap: anywhere`, o min-content dele é UM caractere e o flex encolhia até isso —
       "Deepseek Claude" virava uma coluna vertical de letras. Mesma dupla que o modo compacto já
       usa em `.compacta .ct-top` (o wrap) e `.compacta .ct-txt` (o piso de largura), e pelo mesmo
       motivo: com piso, quem desce pra segunda linha é o que não coube, não o nome. */
    .ct-top { flex-wrap: wrap; row-gap: var(--space-2); }
    .ct-txt { min-width: 14ch; }
  }

  /* ------------------------------------------------------------- cards (31/08)
     O card é o MESMO pros dois tipos — conta do Claude e chave de API. O que muda é a etiqueta
     e o subtítulo; desenhar dois cards diferentes traria de volta os dois vocabulários que a
     unificação veio matar. */
  .ct-lapis {
    /* Sobrescreve o alvo de toque global de 44px (app.css): sem isto o BOTÃO define a altura da
       linha do nome — 44px de linha para um lápis de 14px, e a linha da conta inteira herdava
       isso (medido: linha de 98px, sendo 44 só a do nome). Mesmo remédio da árvore de arquivos.
       O container query estreito devolve os 44px de alvo (regra lá embaixo). */
    min-height: 0; min-width: 0;
    flex-shrink: 0; display: inline-flex; align-items: center; gap: 4px;
    background: transparent; border: none; padding: 0 2px; cursor: pointer;
    color: var(--text-muted); line-height: 1; opacity: .75;
  }
  .ct-lapis-txt { font-size: var(--text-2xs); white-space: nowrap; }
  @media (hover: hover) and (pointer: fine) {
    .ct-lapis:hover { opacity: 1; color: var(--text-secondary); }
  }
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
  .ct-jan-reset { color: var(--text-muted); font-size: var(--text-3xs); white-space: nowrap; }
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
  .ct-reset { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2);
              margin-top: var(--space-3); padding-top: var(--space-3);
              border-top: 1px solid var(--border-subtle); }
  .ct-reset-info { flex: 1; min-width: 16ch; color: var(--text-secondary); font-size: var(--text-xs); }
  .ct-reset-info small { display: block; margin-top: 2px; color: var(--text-muted); }
  .ct-reset-reason { flex-basis: 100%; color: var(--text-muted); font-size: var(--text-2xs); }
  .ct-reset-confirm { margin-top: var(--space-3); padding: var(--space-3);
                      border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
                      background: var(--surface-inset); }
  .ct-reset-confirm p { margin: 0 0 var(--space-3); color: var(--text-secondary);
                        font-size: var(--text-xs); line-height: 1.45; }
  .ct-reset-confirm > div { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .ct-escolha-txt { color: var(--text-secondary); font-size: 12px; align-self: center; }
  /* Formulário da chave: superfície própria (é área de entrada), por isso --surface-inset e não
     --bg-base cru — com papel de parede ligado, o cru vira retângulo chapado sobre a foto. */
  .ct-form {
    margin-top: var(--space-3); padding: var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: 10px;
    background: var(--surface-inset);
  }
  .ct-form-leg { color: var(--text-muted); font-size: 12px; margin: 0 0 var(--space-3); }
  /* Linha inteira dentro da faixa flex do nome, senão ela disputa espaço com o campo e os botões. */
  .ct-renomear-leg { flex-basis: 100%; margin-bottom: 2px; }
  /* Container query, não media query: quem aperta a linha é a largura do PAINEL. */
  .ct-form-linha { display: flex; gap: var(--space-3); }
  @container (max-width: 460px) { .ct-form-linha { flex-direction: column; } }
  .ct-campo-l { display: flex; flex-direction: column; gap: 4px; margin-bottom: var(--space-3); min-width: 0; }
  .ct-campo-l.larga { flex: 1; }
  .ct-campo-l > span { font-size: 11.5px; color: var(--text-secondary); }
  /* Formulário do cookie: mora DENTRO da linha da credencial (largura cheia, abaixo dela) porque
     é configuração daquela credencial, não uma tela nova. Mesma superfície do formulário da chave. */
  .ct-cookie {
    margin-top: var(--space-2); padding: var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: 10px;
    background: var(--surface-inset);
  }
</style>
