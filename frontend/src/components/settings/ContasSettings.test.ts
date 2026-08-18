// @vitest-environment happy-dom
// Aba Contas: a lista lê a fonte única (/api/conta-estado via lib/contaEstado), criar/apagar
// reusam as rotas de sempre (lib/api), conta deslogada NUNCA some da lista.
// (B6) Os 7 casos abaixo foram RESTAURADOS do pai f3189f6b (os dois describe da Task 4),
// e os 6 casos do login (Task 7) ficam AO LADO num terceiro describe — estender, não
// substituir. Regressão de uma Task aprovada e mergeada: o código que eles cobriam
// (novaConta/apagar/lista) continua vivo e ligado aos botões.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ContasSettings from './ContasSettings.svelte';
import { criarProps } from './props-reativas.svelte';
import * as m from '../../paraglide/messages';
import * as contaEstadoLib from '../../lib/contaEstado';
import * as apiLib from '../../lib/api';
import * as loginLib from '../../lib/loginConta';
import { mensagemDeErro } from '../../lib/errosApi';
import type { ContaEstado } from '../../lib/contaEstado';
import type { Server } from '../../lib/auth';

vi.mock('../../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/contaEstado')>();
  return { ...real, listarEstadosDeConta: vi.fn() };
});
vi.mock('../../lib/api', () => ({
  criarConta: vi.fn(async () => ({ path: '/x', label: 'x', active: false })),
  apagarConta: vi.fn(async () => {}),
}));
vi.mock('../../lib/loginConta', () => ({
  iniciarLogin: vi.fn(async () => ({ ok: true })),
  passoLogin: vi.fn(async () => ({ etapa: 'aguardando', url: 'https://claude.com/cai/oauth/authorize' })),
  confirmarLogin: vi.fn(async () => ({ ok: true, email: 'u@example.com', plano: 'max' })),
  cancelarLogin: vi.fn(async () => ({ ok: true })),
}));

const estadoMock = vi.mocked(contaEstadoLib);
const apiMock = vi.mocked(apiLib);
const loginMock = vi.mocked(loginLib);

const LOGADA: ContaEstado = {
  path: '/home/u/.claude-jefferson', label: 'jefferson', active: true,
  login: { estado: 'ok', loggedIn: true, email: 'jefferson@example.com', plano: 'max' },
  limite: { estado: 'lido', linha: '⚡5h:64% 📅7d:83%', ts: 1, idade_s: 5 },
};
const DESLOGADA: ContaEstado = {
  path: '/home/u/.claude-testes', label: 'testes', active: false,
  login: { estado: 'ok', loggedIn: false },
  limite: { estado: 'sem_leitura' },
};

// Alvo EXPLÍCITO (outra máquina): é o contrato do parecer — a aba tem de afirmar o alvo, não
// só que a função foi chamada. Os casos abaixo passam a exigir `ALVO` como primeiro argumento;
// o caminho global (null) tem caso próprio no fim.
const ALVO: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 't-b' };

function montar(contas: ContaEstado[], alvo: Server | null = ALVO) {
  estadoMock.listarEstadosDeConta.mockResolvedValue(contas);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ContasSettings, { target: el, props: { apiTarget: alvo } });
  return { el, comp: comp as never };
}

beforeEach(() => { vi.clearAllMocks(); });

describe('ContasSettings — a lista', () => {
  it('mostra conta com em uso e e-mail (a frase do plano ficou de fora por decisão do árbitro — Task de ajuste serial quando o Lote A mergear)', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-linha')!;
    expect(linha.querySelector('.ct-nome')!.textContent).toBe('jefferson');
    expect(linha.querySelector('.ct-emuso')!.textContent).toBe(m.contas_em_uso());
    expect(linha.querySelector('.ct-sub')!.textContent).toContain('jefferson@example.com');
    expect(linha.querySelector('.ct-cota')!.textContent).toContain(m.cota_lido_agora());
    unmount(t.comp);
  });

  it('conta deslogada CONTINUA na lista, com Entrar (inerte) e estado de leitura explícito', async () => {
    const t = montar([LOGADA, DESLOGADA]);
    await tick(); await tick();
    const linhas = t.el.querySelectorAll<HTMLElement>('.ct-linha');
    expect(linhas.length).toBe(2);
    const fora = linhas[1];
    expect(fora.textContent).toContain('testes');
    expect(fora.textContent).toContain(m.contas_nao_conectada());
    expect(fora.textContent).toContain(m.contas_sem_leitura());
    expect(fora.querySelector('.ct-acao.primaria')!.textContent).toBe(m.contas_entrar());
    unmount(t.comp);
  });

  it('conta logada por chave de API (sem e-mail) NAO pode dizer que nao esta conectada', async () => {
    // Shape REAL medido em 17/08 na máquina do usuário, conta before-merge-20260418-185405:
    // `claude auth status --json` -> {loggedIn:true, authMethod:"api_key"} SEM email.
    const API_KEY: ContaEstado = {
      path: '/home/u/.claude-before-merge', label: 'before-merge', active: false,
      login: { estado: 'ok', loggedIn: true, email: null, plano: null },
      limite: { estado: 'sem_leitura' },
    };
    const t = montar([API_KEY]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-linha')!;
    expect(linha.textContent).not.toContain(m.contas_nao_conectada());
    expect(linha.querySelector('.ct-acao.primaria')).toBeNull();
    unmount(t.comp);
  });

  it('estado indisponivel nao pode ser confundido com deslogada', async () => {
    const INDISP: ContaEstado = {
      path: '/home/u/.claude-x', label: 'x', active: false,
      login: { estado: 'indisponivel', motivo: 'cli-indisponivel' },
      limite: { estado: 'sem_leitura' },
    };
    const t = montar([INDISP]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-linha')!;
    expect(linha.textContent).not.toContain(m.contas_nao_conectada());
    expect(linha.querySelector('.ct-acao.primaria')).toBeNull();
    unmount(t.comp);
  });

  it('limite velho aparece com a idade e sem parecer fresco', async () => {
    const t = montar([{ ...LOGADA, active: false, limite: { estado: 'lido', linha: 'x', ts: 1, idade_s: 7200 } }]);
    await tick(); await tick();
    const cota = t.el.querySelector<HTMLElement>('.ct-cota')!;
    expect(cota.textContent).toContain(m.cota_ultima_leitura({ n: '2 h' }));
    expect(cota.classList.contains('velha')).toBe(true);
    unmount(t.comp);
  });
});

describe('ContasSettings — criar e apagar reusam as rotas de sempre', () => {
  it('criar: campo inline + criarConta + recarrega a lista', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-btn')!.click();   // + Nova conta
    await tick();
    const input = t.el.querySelector<HTMLInputElement>('.ct-campo')!;
    input.value = 'minha-conta';
    input.dispatchEvent(new Event('input'));
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-rodape .ct-btn:nth-of-type(2)')!.click(); // criar
    await tick(); await tick();
    expect(apiMock.criarConta).toHaveBeenCalledWith(ALVO, 'minha-conta');
    expect(estadoMock.listarEstadosDeConta).toHaveBeenCalledTimes(2);   // montagem + pós-criar
    unmount(t.comp);
  });

  it('apagar: kebab → menu → confirmação → apagarConta com o NOME (label) da conta', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-kebab')!.click();
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-menu-item')!.click();
    await tick();
    expect(t.el.querySelector('.ct-confirma')!.textContent).toContain(m.criar_apagar_fim());
    t.el.querySelector<HTMLButtonElement>('.ct-confirma-btn.perigo')!.click();
    await tick(); await tick();
    expect(apiMock.apagarConta).toHaveBeenCalledWith(ALVO, 'jefferson');
    expect(estadoMock.listarEstadosDeConta).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });

  it('falha ao apagar mostra a mensagem traduzida do backend e nao engana com sucesso (Task 4)', async () => {
    // O usuário viu "não foi" sem mensagem nenhuma ao apagar pasta de backup: o aviso sumia
    // com o erro engolido. O detalhe do DELETE vira envelope {code, params, msg}; a mensagem
    // tem que chegar à tela TRADUZIDA. Este teste cai se o erro voltar a ser engolido
    // (sem .ct-aviso.erro, ou com o texto cru do servidor no lugar da chave).
    apiMock.apagarConta.mockRejectedValueOnce(
      Object.assign(new Error(mensagemDeErro('erro_conta_inexistente', { nome: 'backup' })!),
        { status: 404 }));
    const t = montar([LOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-kebab')!.click();
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-menu-item')!.click();
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-confirma-btn.perigo')!.click();
    await tick(); await tick();
    const aviso = t.el.querySelector<HTMLElement>('.ct-aviso.erro');
    expect(aviso).not.toBeNull();
    expect(aviso!.textContent).toContain(m.erro_conta_inexistente({ nome: 'backup' }));
    // Sem recarga da lista (não há sucesso pra recarregar) e sem fechar a confirmação.
    expect(estadoMock.listarEstadosDeConta).toHaveBeenCalledTimes(1);
    expect(t.el.querySelector('.ct-confirma')).not.toBeNull();
    unmount(t.comp);
  });
});

describe('ContasSettings — o botão Entrar (Task 7)', () => {
  it('mostra os quatro passos do mock estado 2 quando começa o login', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith(ALVO, 'testes');
    expect(t.el.querySelector('.ct-login')).not.toBeNull();
    const passos = [...t.el.querySelectorAll<HTMLElement>('.ct-passo')];
    expect(passos.length).toBe(4);
    expect(passos[0].textContent).toContain(m.contas_passo1());
    expect(passos[1].textContent).toContain(m.contas_passo2());
    expect(passos[2].textContent).toContain(m.contas_passo3());
    expect(passos[3].textContent).toContain(m.contas_passo4());
    // O link de autorização (quando o poll devolve a URL) é um <a> de verdade.
    const link = t.el.querySelector<HTMLAnchorElement>('.ct-link')!;
    expect(link).not.toBeNull();
    expect(link.getAttribute('href')).toContain('https://claude.com/cai/oauth/authorize');
    unmount(t.comp);
  });

  it('confirmar o código chama confirmarLogin e recarrega a lista', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = t.el.querySelector<HTMLInputElement>('.ct-campo-cod')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    // O botão Confirmar código fica no SEGUNDO .ct-rodape (o primeiro é o da lista).
    const rodapeLogin = [...t.el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick();
    expect(loginMock.confirmarLogin).toHaveBeenCalledWith(ALVO, 'testes', 'CODE-123');
    // O pós-login recarrega a lista (o poll do passo não a recarrega — só o /passo).
    expect(estadoMock.listarEstadosDeConta.mock.calls.length).toBe(2);
    unmount(t.comp);
  });

  it('desmontar durante o login cancela a tentativa e para o poll (B8)', async () => {
    // B8 — desmontar por QUALQUER porta (trocar de aba, fechar o modal, janela cruzar
    // 820px) não pode deixar o poll órfão nem a tentativa presa no servidor: o próximo
    // Entrar começaria do zero, sem 409.
    vi.useFakeTimers();
    try {
      const t = montar([DESLOGADA]);
      await tick(); await tick();
      t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
      await tick(); await tick(); await tick(); await tick();
      expect(loginMock.iniciarLogin).toHaveBeenCalledWith(ALVO, 'testes');
      unmount(t.comp);
      expect(loginMock.cancelarLogin).toHaveBeenCalledWith(ALVO, 'testes');
      const passosAntes = loginMock.passoLogin.mock.calls.length;
      // Depois do unmount o poll não pode mais bater em /login/passo.
      await vi.advanceTimersByTimeAsync(6000);
      expect(loginMock.passoLogin.mock.calls.length).toBe(passosAntes);
    } finally {
      vi.useRealTimers();
    }
  });

  it('desmontar ENQUANTO o iniciarLogin esta em voo cancela e nao arma poll (B4)', async () => {
    // B8 fechou a porta "desmontar com o painel jah aberto"; esta e a porta que o
    // onDestroy nao via: desmontar ENTRE o clique e a resposta do servidor. O loginDe so
    // e escrito depois do await — sem a flag `destruido`, o onDestroy nao cancela nada e
    // o setInterval fica orfao batendo em /login/passo (e a tentativa presa no servidor).
    vi.useFakeTimers();
    try {
      let resolver!: () => void;
      loginMock.iniciarLogin.mockReturnValue(
        new Promise<void>((r) => { resolver = r; }) as never);
      const t = montar([DESLOGADA]);
      await tick(); await tick();
      t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
      await tick(); await tick(); await tick(); await tick();
      expect(loginMock.iniciarLogin).toHaveBeenCalledWith(ALVO, 'testes');
      unmount(t.comp);      // desmonta ANTES de o iniciarLogin resolver
      resolver();           // a resposta chega num componente morto
      await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
      await tick();
      expect(loginMock.cancelarLogin).toHaveBeenCalledWith(ALVO, 'testes');
      const passosAntes = loginMock.passoLogin.mock.calls.length;
      await vi.advanceTimersByTimeAsync(6000);
      expect(loginMock.passoLogin.mock.calls.length).toBe(passosAntes);
    } finally {
      vi.useRealTimers();
    }
  });

  it('alvo null (sem ?srv=) segue pelo caminho global: chamadas com null (parecer)', async () => {
    // Comportamento final do parecer: sem ?srv= (ou srv == ativo) o alvo é null e tudo segue
    // como hoje — o caminho global com self-heal de 401.
    const t = montar([DESLOGADA], null);
    await tick(); await tick();
    expect(estadoMock.listarEstadosDeConta).toHaveBeenCalledWith(null);
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith(null, 'testes');
    unmount(t.comp);
  });

  it('trocar de alvo com login aberto cancela no alvo ANTIGO e limpa o painel (guard do parecer)', async () => {
    // A tentativa em voo vive no processo do backend do alvo antigo: trocar o ?srv= no meio
    // tem de cancelar LÁ (no capturado), nunca no alvo novo.
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    estadoMock.listarEstadosDeConta.mockResolvedValue([DESLOGADA]);
    await tick(); await tick(); await tick();
    el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith(A, 'testes');
    props.apiTarget = B;   // ?srv= trocou com a tentativa em voo
    await tick(); await tick(); await tick();
    expect(loginMock.cancelarLogin).toHaveBeenCalledWith(A, 'testes');
    expect(el.querySelector('.ct-login')).toBeNull();
    unmount(comp as never);
  });

  it('trocar de alvo com carga em voo descarta a resposta do alvo antigo (guard do parecer)', async () => {
    // A lista do alvo A (pendurada) chega ATRASADA, depois da troca pra B: não pode escrever
    // na tela nem sobrescrever a lista de B — o apagar clicado na lista errada sairia para a
    // máquina errada (o defeito que a Task 5 antiga levou duas rodadas pra fechar).
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    let resolverA!: (v: ContaEstado[]) => void;
    estadoMock.listarEstadosDeConta.mockImplementation((alvo: Server | null) =>
      alvo?.id === 'srv-b'
        ? Promise.resolve([{ ...LOGADA, label: 'bbbb', path: '/home/u/.claude-b' }])
        : new Promise<ContaEstado[]>((r) => { resolverA = r; }));
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    await tick(); await tick();
    props.apiTarget = B;
    await tick(); await tick(); await tick();
    resolverA([LOGADA]);   // resposta atrasada do alvo que saiu da tela
    await tick(); await tick();
    // A lista na tela é a de B; a resposta atrasada de A (label 'jefferson') foi descartada.
    expect([...el.querySelectorAll('.ct-nome')].map((n) => n.textContent)).toEqual(['bbbb']);
    unmount(comp as never);
  });

  it('objeto NOVO com a MESMA identidade não é troca de alvo: login em voo sobrevive (R1)', async () => {
    // O App reconstrói o Server a cada listServers() (JSON.parse do localStorage) e o sync sobe
    // versaoServidores sem o usuário tocar na aba — comparar o OBJETO mataria o login em voo com
    // o servidor sendo o mesmo. O guard compara a identidade composta (id+label+baseUrl+token).
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    estadoMock.listarEstadosDeConta.mockResolvedValue([DESLOGADA]);
    await tick(); await tick(); await tick();
    el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith(A, 'testes');
    const chamadas = estadoMock.listarEstadosDeConta.mock.calls.length;
    props.apiTarget = { ...A };   // MESMO servidor, objeto reconstruído
    await tick(); await tick(); await tick();
    expect(loginMock.cancelarLogin).not.toHaveBeenCalled();
    expect(el.querySelector('.ct-login')).not.toBeNull();
    expect(estadoMock.listarEstadosDeConta.mock.calls.length).toBe(chamadas);
    unmount(comp as never);
  });

  it('trocar de alvo com confirmar em voo: erro atrasado do alvo ANTIGO não pinta a tela da nova (rodada 2)', async () => {
    // O efeito limpou a tela na troca; a resposta atrasada de A não pode repintar o erro. O
    // `{#if loginErro}` é FORA do `{#if loginDe}` de propósito — por isso a ausência é o alvo.
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    let rejeitar!: (e: Error) => void;
    loginMock.confirmarLogin.mockReturnValue(
      new Promise((_r, j) => { rejeitar = j; }) as never);
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    estadoMock.listarEstadosDeConta.mockResolvedValue([DESLOGADA]);
    await tick(); await tick(); await tick();
    el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = el.querySelector<HTMLInputElement>('.ct-campo-cod')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    const rodapeLogin = [...el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick();
    expect(loginMock.confirmarLogin).toHaveBeenCalledWith(A, 'testes', 'CODE-123');
    props.apiTarget = B;   // ?srv= trocou com o confirmar em voo
    await tick(); await tick(); await tick();
    rejeitar(Object.assign(new Error('ERRO-DA-MAQUINA-A'), { status: 0 }));   // resposta atrasada de A
    await tick(); await tick();
    expect(el.querySelector('.ct-aviso.erro')).toBeNull();
    unmount(comp as never);
  });

  it('trocar de alvo com confirmar em voo: sucesso atrasado do alvo ANTIGO não anuncia login na tela da nova (rodada 2)', async () => {
    // A pior forma do bloqueador: a aba de B anuncia, com e-mail e plano, um login que aconteceu
    // em A — o usuário age nisso (abre sessão em B esperando aquela conta).
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    let resolver!: (v: { ok: boolean; email: string; plano: string }) => void;
    loginMock.confirmarLogin.mockReturnValue(
      new Promise((r) => { resolver = r; }) as never);
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    estadoMock.listarEstadosDeConta.mockResolvedValue([DESLOGADA]);
    await tick(); await tick(); await tick();
    el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = el.querySelector<HTMLInputElement>('.ct-campo-cod')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    const rodapeLogin = [...el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick();
    props.apiTarget = B;   // ?srv= trocou com o confirmar em voo
    await tick(); await tick(); await tick();
    resolver({ ok: true, email: 'conta-da-MAQUINA-A@x.com', plano: 'max' });   // resposta atrasada de A
    await tick(); await tick();
    // Nenhum aviso nenhum: o efeito limpou na troca e a resposta atrasada não repintou.
    expect([...el.querySelectorAll<HTMLElement>('.ct-aviso')]).toHaveLength(0);
    unmount(comp as never);
  });

  it('confirmar com erro ainda recarrega a lista — o login pode ter completado no servidor (parecer passo 4)', async () => {
    // Teto estourado corta o cliente mas o backend SEGUE: a conta acaba logada. A lista tem de
    // recarregar mesmo no ramo de erro, senão a tela mente sozinha dizendo deslogada.
    loginMock.confirmarLogin.mockRejectedValueOnce(
      Object.assign(new Error(m.erro_login_timeout()), { status: 0 }));
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = t.el.querySelector<HTMLInputElement>('.ct-campo-cod')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    const rodapeLogin = [...t.el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick(); await tick();
    // montagem + recarga do ramo de erro: a lista não fica presa no estado antigo.
    expect(estadoMock.listarEstadosDeConta.mock.calls.length).toBe(2);
    expect(t.el.querySelector<HTMLElement>('.ct-aviso.erro')!.textContent).toContain(m.erro_login_timeout());
    unmount(t.comp);
  });

  it('confirmar com campo vazio não chama a API', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const confirmar = t.el.querySelector<HTMLButtonElement>('.ct-rodape .ct-btn.primario')!;
    expect(confirmar.disabled).toBe(true);
    expect(loginMock.confirmarLogin).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('cancelar chama cancelarLogin, limpa o painel e reabilita o Entrar', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    // O rodapé do login é o SEGUNDO .ct-rodape (o primeiro é o rodapé da lista, "+ Nova conta").
    const rodapeLogin = [...t.el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('button:not(.primario)')!.click();
    await tick(); await tick();
    expect(loginMock.cancelarLogin).toHaveBeenCalledWith(ALVO, 'testes');
    expect(t.el.querySelector('.ct-login')).toBeNull();
    expect(t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.disabled).toBe(false);
    unmount(t.comp);
  });

  it('erro do servidor (409 envelope) vira mensagem traduzida no aviso', async () => {
    // O backend manda {code, params, msg} (mensagens.py); o cliente deve devolver a MENSAGEM
    // traduzida pelo id, nunca o texto cru do servidor.
    loginMock.iniciarLogin.mockRejectedValueOnce(
      Object.assign(new Error(mensagemDeErro('erro_login_ja_em_curso')!), { status: 409 }));
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(t.el.querySelector('.ct-login')).toBeNull();
    expect(t.el.querySelector<HTMLElement>('.ct-aviso.erro')!.textContent)
      .toContain(m.erro_login_ja_em_curso());
    unmount(t.comp);
  });

  it('erro de rede nao aparece como Failed to fetch cru', async () => {
    loginMock.iniciarLogin.mockRejectedValueOnce(new Error('Failed to fetch'));
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const aviso = t.el.querySelector<HTMLElement>('.ct-aviso.erro')!;
    expect(aviso.textContent).toContain(m.falha_conexao());
    expect(aviso.textContent).not.toContain('Failed to fetch');
    unmount(t.comp);
  });
});
