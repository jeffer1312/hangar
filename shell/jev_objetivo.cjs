// jev_objetivo — laço de navegação decidido pelo Jev (typesafe.ai).
//
// Desenho copiado do browser-use/jev-ultrafast: uma requisição por ciclo devolve a OPERAÇÃO e,
// especulativamente, o alvo de cada operação que precisa de um. Só o alvo que casa com a operação
// escolhida é usado; os outros são descartados. Assim operação e alvo saem juntos sem que a
// segunda pergunta precise esperar a primeira — as perguntas de uma requisição são respondidas em
// paralelo, então uma NÃO pode depender da resposta da outra.
//
// O Jev não escreve texto. Quando a operação é TYPE_TEXT, um LLM pequeno escreve o valor na hora,
// já vendo o campo e o objetivo — não existe dicionário de campos preparado por site.
//
// As funções puras são o que o teste cobre; quem fala com rede, navegador e LLM é injetado.

const PAPEIS_DE_TEXTO = new Set(['textbox', 'searchbox']);
// `option` não entra em CLICK: num `<select>` nativo, clicar na opção não muda o valor do select —
// o próprio `fill` do hangar recusa com "o clique nao deixou um campo de texto com foco". Ela é
// alvo da operação SELECT, que define o valor e dispara input/change, como o Playwright faz.
const PAPEL_DE_OPCAO = 'option';
const PAPEIS_INTERATIVOS = new Set([
  'button', 'link', 'checkbox', 'radio', 'menuitem', 'tab', 'switch', 'slider',
  PAPEL_DE_OPCAO, 'combobox',
  ...PAPEIS_DE_TEXTO,
]);
// Teto do próprio `choice` do Jev. A doc manda mandar a lista inteira em vez de uma pré-seleção,
// porque opção omitida é opção que o modelo não pode escolher.
const MAX_CANDIDATOS = 255;
const NENHUM = '_nenhum';

/** O rótulo da página traz o asterisco de obrigatório, o da árvore de acessibilidade não. */
const normalizar = (nome) => String(nome).replace(/\s*\*\s*$/, '').trim().toLowerCase();

const OPERACOES = {
  CLICK: 'click an element to move one step toward the goal',
  TYPE_TEXT: 'type a value into a text field',
  SELECT: 'choose an option in a dropdown',
  SCROLL_DOWN: 'scroll down because what the goal needs is not visible yet',
  // "Parece pronto" nao basta: com o formulario todo preenchido o Jev dava DONE enquanto a tela
  // ainda dizia "Alteracoes nao salvas". O criterio pede a confirmacao da propria pagina.
  DONE: 'the goal is fully accomplished and the page itself confirms it — nothing is left to submit, save or confirm',
  BLOCKED: 'the goal cannot be accomplished from this page',
};

// A operação tem piso BAIXO de propósito: quando dois caminhos plausíveis dividem a probabilidade
// (clicar na aba certa ou digitar na busca), qualquer um dos dois anda, e o ciclo seguinte
// re-observa e corrige. O alvo é que precisa de piso alto — clicar no elemento errado não corrige.
// DONE e BLOCKED tem piso PROPRIO, e alto: um CLICK errado o ciclo seguinte corrige, uma parada
// errada encerra o trabalho. Com o piso da operacao o laco declarou pronto com 0,50, sem ter salvo.
const LIMIARES = { operacao: 0.3, alvo: 0.6, arriscado: 0.5, conclusao: 0.6 };

const distribuicao = (resposta) => Object.entries(resposta?.probabilities ?? {})
  .sort((a, b) => b[1] - a[1]).slice(0, 3)
  .map(([k, p]) => `${k} ${p.toFixed(2)}`).join(', ');

/**
 * A força de uma escolha é a probabilidade da opção vencedora, não o campo `confidence` — o Jev
 * calibra os dois de formas diferentes, e já barrei um alvo que tinha 0.64 contra 0.17 do segundo
 * porque o `confidence` vinha abaixo do limiar.
 */
function forca(resposta) {
  const p = resposta?.probabilities?.[resposta?.choice];
  if (typeof p === 'number') return p;
  return typeof resposta?.confidence === 'number' ? resposta.confidence : 0;
}

/**
 * Quando o vencedor tem GÊMEOS na página — mesmo papel e mesmo nome acessível —, devolve o
 * primeiro deles e a massa somada de todos. Fora disso, null.
 *
 * O limiar do alvo existe porque clicar no elemento errado não se corrige no ciclo seguinte. Mas
 * dois botões "Nova sessão" (o `+` do topo e o da barra lateral) abrem a MESMA folha: ali não há
 * elemento errado, e mesmo assim a probabilidade se dividia (0.48 contra 0.31) e o laço desistia
 * no primeiro passo. Só o rótulo IDÊNTICO entra: dois rótulos diferentes que o Jev não soube
 * separar continuam sendo ambiguidade de verdade, e continuam parando.
 */
function sinonimos(head, candidatos) {
  const vencedor = candidatos.find((c) => c.ref === head?.choice);
  if (!vencedor) return null;
  const iguais = candidatos.filter((c) => c.papel === vencedor.papel && c.nome === vencedor.nome);
  if (iguais.length < 2) return null;
  // Sem distribuição não há massa a somar, e somar `{}` daria ZERO — trocando a `confidence` que o
  // `forca()` usaria por um piso que nada alcança, e travando o laço justamente onde esta função
  // existe pra destravar. Sem `probabilities`, o caminho é o de sempre.
  const p = head.probabilities;
  if (!p) return null;
  return { ref: iguais[0].ref, massa: iguais.reduce((s, c) => s + (p[c.ref] ?? 0), 0) };
}

const CHAVE_SECRETA = /senha|password|passwd|token|secret|api[-_ ]?key|cartao|cvv/i;

/** Extrai os elementos acionáveis da árvore de acessibilidade do `snapshot`. */
function parsarSnapshot(texto, max = MAX_CANDIDATOS) {
  const achados = [];
  for (const linha of String(texto).split('\n')) {
    const m = linha.match(/^\s*-\s+(\w+)(?:\s+"([\s\S]*?)")?\s+\[ref=@(e\d+)\]/);
    if (!m) continue;
    const [, papel, nome, ref] = m;
    if (!PAPEIS_INTERATIVOS.has(papel)) continue;
    // Elemento sem nome acessível não dá ao Jev nada para julgar, e ocupa uma vaga do choice.
    if (!nome || !nome.trim()) continue;
    achados.push({ ref, papel, nome: nome.trim().slice(0, 120) });
    if (achados.length >= max) break;
  }
  return achados;
}

const rotulo = (c) => `${c.papel} "${c.nome}"`;

/** Um head de alvo por operação que precisa de um; vazio quando a página não oferece aquele tipo. */
function headDeAlvo(nome, instrucao, candidatos) {
  if (!candidatos.length) return {};
  return {
    [nome]: {
      type: 'choice',
      instructions: instrucao,
      criteria: {
        ...Object.fromEntries(candidatos.map((c) => [c.ref, rotulo(c)])),
        [NENHUM]: 'no element here fits',
      },
    },
  };
}

/** O que o Jev vê de um valor: sem ele, casar a chave do chamador com o rótulo da página vira
 * adivinhação de sinônimo — é "Unimed" que diz que aquilo é um convênio. Segredo não sai daqui. */
function amostraDoValor(chave, valor) {
  if (CHAVE_SECRETA.test(chave)) return '(valor sensivel, nao enviado)';
  return `"${String(valor).slice(0, 60)}"`;
}

const cabecaDoValor = (ref) => `valor_${ref}`;

/**
 * Uma pergunta de valor POR CAMPO, não uma só. A pergunta "qual valor vai neste campo" depende de
 * qual campo venceu, e as perguntas de uma requisição são respondidas em paralelo — então nenhuma
 * pode depender da resposta de outra. Especular por campo mantém tudo em uma requisição só.
 */
function cabecasDeValor(campos, dados) {
  const chaves = Object.keys(dados);
  if (!chaves.length) return {};
  return Object.fromEntries(campos.map((c) => [cabecaDoValor(c.ref), {
    type: 'choice',
    instructions: `Which of these values is the right kind of value for the field "${c.nome}"? `
      + 'Judge by the field itself, not by whether the goal mentions it.',
    criteria: {
      ...Object.fromEntries(chaves.map((k) => [k, `${k} = ${amostraDoValor(k, dados[k])}`])),
      // Falar do CAMPO, não do objetivo: escrita em função do objetivo, esta saída ganhava de um
      // valor que casava exatamente com o rótulo, só porque o objetivo não citava aquele campo.
      [NENHUM]: 'none of these values is the kind of value this field expects',
    },
  }]));
}

/** Combobox que so mostra a lista depois de digitar: e campo de texto E botao ao mesmo tempo. */
const ehEditavel = (c, editaveis) => c.papel === 'combobox' && editaveis.has(normalizar(c.nome));

/**
 * O corpo de perguntas de um ciclo: operação, os alvos especulativos e o risco. Uma requisição.
 *
 * Com uma lista ABERTA os alvos encolhem para ela. Lista aberta é estado modal: digitar noutro
 * campo ou clicar noutro lugar a fecha e joga fora o passo. Isso o código sabe, então não vira
 * pergunta — perguntando, o laço alternava entre abrir a lista e digitar noutro campo sem fim.
 */
function montarPerguntas(candidatos, dados = {}, editaveis = new Set(), listaAberta = '') {
  const opcoes = candidatos.filter((c) => c.papel === PAPEL_DE_OPCAO);
  // `combobox` ENTRA em clicaveis: o dropdown do Radix e um <button role="combobox"> que so abre
  // com clique, e a lista de opcoes so aparece na arvore depois de aberto. `option` tambem entra:
  // numa listbox ARIA (sem <select> escondido) so o clique escolhe, e SELECT falharia ali.
  const aberta = normalizar(listaAberta);
  const clicaveis = aberta
    ? opcoes
    : candidatos.filter((c) => !PAPEIS_DE_TEXTO.has(c.papel));
  // O combobox editavel fica nos DOIS heads: abrir e digitar sao passos diferentes, e quem decide
  // qual deles vem agora e o Jev, olhando se a lista ja esta na tela.
  const campos = aberta
    ? candidatos.filter((c) => normalizar(c.nome) === aberta)
    : candidatos.filter((c) => PAPEIS_DE_TEXTO.has(c.papel) || ehEditavel(c, editaveis));
  const disponiveis = { ...OPERACOES };
  if (!clicaveis.length) delete disponiveis.CLICK;
  if (!campos.length) delete disponiveis.TYPE_TEXT;
  if (!opcoes.length) delete disponiveis.SELECT;
  return {
    ...cabecasDeValor(campos, dados),
    ...headDeAlvo('select_target', 'If the next operation is SELECT, which option is chosen?', opcoes),
    operacao: {
      type: 'choice',
      // A restricao vive AQUI, na instrucao da pergunta, e nao no criterio de CLICK: escrita la,
      // ela falava em "preencha os campos primeiro" e tornava CLICK atraente para quem quer
      // preencher, que e o oposto do pretendido.
      instructions: 'What is the single next operation that moves toward the goal? Not the whole goal, just the next step. '
        + 'While obrigatorios_ainda_vazios is not empty, filling those fields comes before submitting or saving. '
        // Um combobox nao tem valor legivel por codigo: o que ha e o texto na tela, e so o Jev
        // distingue um valor escolhido de um convite a escolher. Sem esta frase o laco achava que
        // tudo estava preenchido e "Salvar" ganhava do campo que faltava.
        + 'In valores_atuais, a required field whose value reads as an invitation to choose '
        + '("Buscar...", "Selecione...", "Escolha...") has NOT been filled yet, and filling it '
        + 'also comes before submitting or saving.',
      criteria: disponiveis,
    },
    // A ordem "preencher antes de salvar" tem que estar AQUI tambem: a pergunta da operacao nao
    // separa abrir um campo de submeter o formulario, as duas sao CLICK, e quem decide entre elas
    // e este head — que sem a regra escolhia "Salvar" com o campo obrigatorio ainda por escolher.
    ...headDeAlvo('click_target', 'If the next operation is CLICK, which element is clicked? '
      + 'Do not pick a submit or save control while obrigatorios_ainda_vazios is not empty, or while '
      + 'a required field in valores_atuais still shows an invitation to choose ("Buscar...", '
      + '"Selecione..."): pick that field instead.', clicaveis),
    // Sem esta regra o head escolhia um campo que valores_atuais ja mostrava cheio, e o laco
    // reescrevia o mesmo texto enquanto o campo que faltava seguia vazio.
    ...headDeAlvo('type_text_target', 'If the next operation is TYPE_TEXT, which field is typed into? '
      + 'Pick a field that valores_atuais shows as still empty, or as still showing an invitation '
      + 'to choose ("Buscar...", "Selecione..."). Never pick a field that already holds the value '
      + 'it needs.', campos),
    // Pergunta PROPRIA para encerrar, em vez de um limiar mais alto no `choice` da operacao: sao
    // coisas diferentes. "DONE e a melhor das operacoes" nao e "a pagina confirma que acabou" — o
    // laco ja deu DONE 0,59 com "Alteracoes nao salvas" escrito na tela. Esta pergunta olha o fato.
    concluido: {
      type: 'noul',
      instructions: 'The page now shows that the goal was carried out: a success message, a saved or '
        + 'confirmed state, or the new or changed record visible in the page content. '
        + 'Pending changes, unsaved changes or a form still waiting to be submitted mean this is false.',
    },
    arriscado: {
      type: 'noul',
      instructions: 'The next operation would buy, send, publish, or delete something that another person would see or that cannot be undone.',
    },
  };
}

/**
 * A prova de que a ação deu certo pode estar no COMEÇO da página (um aviso no topo) ou no FIM
 * (um toast, a linha nova no fim de uma grade). Cortar só o começo escondia o registro recém-criado
 * — medido nesta tela: o texto tem 2654 caracteres e o registro salvo cai no 1624, fora de um corte
 * de 1500, e o Jev julgava "concluiu?" sem nunca ver o que acabara de criar. Então guarda as duas
 * pontas e joga fora o miolo, que numa grade longa é repetição de linha.
 */
const TETO_DO_TEXTO = 4000;
const CABECA = 2500;

function recortarTexto(texto) {
  const t = String(texto).trim();
  if (t.length <= TETO_DO_TEXTO) return t;
  return `${t.slice(0, CABECA)}\n[...]\n${t.slice(-(TETO_DO_TEXTO - CABECA))}`;
}

/** O estado que acompanha as perguntas. */
function montarEstado(objetivo, url, feito, textoDaPagina = '', anotacao = {}) {
  return {
    objetivo,
    url,
    ja_feito: feito.slice(-8),
    // Lido da página a cada ciclo, não acumulado pelo laço: pega também o que a própria página
    // preencheu sozinha (padrão, derivado de outro campo) e o que um passo anterior desfez.
    valores_atuais: anotacao.valores ?? {},
    obrigatorios_ainda_vazios: anotacao.obrigatorios_vazios ?? [],
    // Sem o texto, DONE fica sem evidência: a tela de sucesso costuma ser só uma frase, sem nenhum
    // elemento acionável para aparecer em `elementos`.
    texto_da_pagina: recortarTexto(textoDaPagina),
    // Os rotulos dos elementos NAO entram aqui: cada um ja e criterio do head que o mira, e estado
    // repetido e o "context rot" que a doc do Jev aponta como causa de queda de acerto.
  };
}

/**
 * A árvore de acessibilidade mostra `textbox "Alíquota do ISS"` e NÃO mostra o que está escrito
 * nele. Sem isso o Jev não distingue campo cheio de vazio e fica re-mirando o que acabou de
 * preencher. Lê por NOME, que é a mesma chave que o Jev usa para decidir — evita manter um segundo
 * sistema de identidade em paralelo às refs.
 */
const JS_VALORES = `(() => {
  const nomeDe = (e) => (
    e.getAttribute('aria-label')
    || (e.labels && e.labels[0] && e.labels[0].innerText)
    || (e.getAttribute('aria-labelledby') && (document.getElementById(e.getAttribute('aria-labelledby')) || {}).innerText)
    || e.getAttribute('placeholder')
    || e.getAttribute('name')
    || ''
  ).trim();
  const visivel = (e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
  // Um combobox do Radix e um <button>: o .value dele e sempre "", e o valor escolhido so existe
  // como texto. Lido pelo .value ele parecia eternamente vazio, e o Jev re-mirava o campo que
  // acabara de preencher. Quando o Radix espelha num <select> escondido, esse select e a verdade.
  const comboEscrito = (e) => e.tagName !== 'INPUT' && e.getAttribute('role') === 'combobox';
  const espelho = (e) => (e.parentElement || {}).querySelector
    ? e.parentElement.querySelector('select') : null;
  const fora = { valores: {}, obrigatorios_vazios: [], editaveis: [], lista_aberta: '' };
  for (const e of document.querySelectorAll('input,textarea,select,[role=switch],[role=checkbox],[role=combobox]')) {
    if (!visivel(e)) continue;
    const nome = nomeDe(e);
    if (!nome) continue;
    // aria-checked="false" PRECISA virar false, nao cair no ramo de baixo: um switch do Radix e
    // um <button>, e o .value dele e "on" ligado ou desligado — desligado lia como ligado.
    const ck = e.getAttribute('aria-checked');
    const marcado = e.type === 'checkbox' || e.type === 'radio' ? e.checked
      : (ck === 'true' ? true : (ck === 'false' ? false : null));
    const combo = comboEscrito(e);
    // Com a lista ABERTA o botao continua mostrando o convite ("Buscar codigo..."), e o laco lia
    // isso como campo vazio e digitava de novo, sem nunca escolher. O estado aberto e o fato que
    // faltava: daqui o passo seguinte e escolher uma opcao, nao digitar.
    const aberto = combo && e.getAttribute('aria-expanded') === 'true';
    const busca = aberto ? document.querySelector('input[role=combobox][aria-expanded=true]') : null;
    const valor = marcado !== null ? (marcado ? 'marcado' : 'desmarcado')
      : (aberto ? 'lista aberta, buscando "' + String((busca || {}).value || '') + '"'
        : (combo ? String(e.innerText || '').replace(/\\s+/g, ' ').trim() : String(e.value ?? '')));
    const chave = nome.slice(0, 80);
    if (valor) fora.valores[chave] = valor.slice(0, 60);
    if (aberto) fora.lista_aberta = chave;
    // Combobox que so revela a lista depois de digitar: o laco precisa abrir e DIGITAR, nao so
    // clicar e escolher. O popover do cmdk se anuncia por haspopup=dialog; a forma ARIA padrao,
    // por aria-autocomplete num input.
    const ac = e.getAttribute('aria-autocomplete');
    if (e.tagName === 'INPUT' && ac && ac !== 'none') fora.editaveis.push(chave);
    else if (combo && e.getAttribute('aria-haspopup') === 'dialog') fora.editaveis.push(chave);
    // Obrigatorio vazio e o que impede o submit. Sem isso o Jev clica em "Salvar" no terceiro
    // passo, porque salvar parece mesmo um passo valido rumo a um objetivo que diz "e salvar".
    const exigido = e.required || e.getAttribute('aria-required') === 'true' || /\\*\\s*$/.test(nome);
    if (!exigido) continue;
    // Num combobox escrito, "texto na tela" nao distingue valor escolhido de frase-convite
    // ("Buscar codigo ou descricao..."). Onde ha <select> espelho, ele responde; onde nao ha,
    // a lista NAO afirma vazio — o texto esta em valores_atuais e quem julga e o Jev.
    if (!combo) { if (!valor) fora.obrigatorios_vazios.push(chave); continue; }
    const s = espelho(e);
    if (s && !s.value) fora.obrigatorios_vazios.push(chave);
  }
  return JSON.stringify(fora);
})()`;

const VAZIO = { valores: {}, obrigatorios_vazios: [], editaveis: new Set(), lista_aberta: '' };

async function valoresAtuais(executar) {
  try {
    const bruto = await executar('eval', [JS_VALORES]);
    // O navegador devolve o resultado JA como literal de string JSON, entao sao DOIS JSON.parse —
    // um tira o literal, o outro le o objeto. Desescapar na mao com regex perdia o par de aspas:
    // qualquer valor com aspas derrubava a leitura inteira no catch, e o laco seguia achando que
    // nao havia campo preenchido nem lista aberta, calado.
    const texto = String(bruto).replace(/^ok:\s*/, '').trim();
    const obj = JSON.parse(texto.startsWith('"') ? JSON.parse(texto) : texto);
    if (!obj || typeof obj !== 'object') return VAZIO;
    return {
      valores: obj.valores ?? {},
      obrigatorios_vazios: obj.obrigatorios_vazios ?? [],
      editaveis: new Set((obj.editaveis ?? []).map(normalizar)),
      lista_aberta: obj.lista_aberta ?? '',
    };
  } catch {
    // Anotação é melhoria, não requisito: sem ela o laço segue como antes.
    return VAZIO;
  }
}

/**
 * Espera a lista do autocomplete aparecer depois de digitar. Piso de dois quadros (um re-render do
 * React não fica pronto no mesmo quadro) e teto de 200ms, como o jev-ultrafast: sem o teto, campo
 * que não tem lista trava o ciclo; sem o piso, a resposta chega antes do primeiro quadro. Medido
 * nesta tela: a primeira opção aparece ~190ms depois da tecla.
 */
const JS_ESPERAR_OPCOES = `new Promise((pronto) => {
  const campo = document.querySelector('[role=combobox][aria-expanded=true]');
  const ids = ((campo && (campo.getAttribute('aria-controls') || campo.getAttribute('aria-owns'))) || '')
    .split(/\\s+/).filter(Boolean);
  const raizes = ids.map((id) => document.getElementById(id)).filter(Boolean);
  const onde = raizes.length ? raizes : [document];
  const visiveis = () => onde
    .flatMap((r) => [...r.querySelectorAll('[role=option]')])
    .filter((e) => { const b = e.getBoundingClientRect(); return b.width > 0 && b.height > 0; });
  let quadros = 0;
  let fim = false;
  const acabar = () => { if (!fim) { fim = true; pronto('opcoes: ' + visiveis().length); } };
  setTimeout(acabar, 200);
  const olhar = () => {
    if (fim) return;
    if (++quadros >= 2 && visiveis().length) acabar();
    else requestAnimationFrame(olhar);
  };
  requestAnimationFrame(olhar);
})`;

/** `aria-expanded` do combobox de nome `rotuloDoCampo` — clicar num ja aberto o FECHA. */
function jsDeEstarAberto(rotuloDoCampo) {
  return `(() => {
    const alvo = ${JSON.stringify(normalizar(rotuloDoCampo))};
    const nome = (e) => ((e.labels && e.labels[0] && e.labels[0].innerText) || e.getAttribute('aria-label') || '')
      .replace(/\\s*\\*\\s*$/, '').trim().toLowerCase();
    const e = [...document.querySelectorAll('[role=combobox]')].find((x) => nome(x) === alvo);
    return e ? String(e.getAttribute('aria-expanded') === 'true') : 'sem campo';
  })()`;
}

const HEAD_DA_OPERACAO = { CLICK: 'click_target', TYPE_TEXT: 'type_text_target', SELECT: 'select_target' };
const TERMINAIS = new Set(['DONE', 'BLOCKED']);

/**
 * Escolhe a opção pelo texto no `<select>` que a contém, definindo o valor e disparando
 * input/change — é o que o `selectOption` do Playwright faz, e a única forma que funciona num
 * dropdown nativo. O rótulo entra por `JSON.stringify`, então texto com aspas não vira código.
 */
const SEM_SELECT = 'sem-select-nativo';

function jsDeSelecionar(rotuloDaOpcao) {
  const alvo = JSON.stringify(rotuloDaOpcao);
  return `(() => {
    const rotulo = ${alvo};
    const casa = (o) => (o.text || '').trim() === rotulo;
    const select = [...document.querySelectorAll('select')].find((s) => [...s.options].some(casa));
    // Nao e erro: numa listbox ARIA (Radix, cmdk) nao existe <select>, e a escolha se faz no
    // clique. Quem chamou troca de caminho em vez de parar.
    if (!select) return ${JSON.stringify(SEM_SELECT)};
    select.value = [...select.options].find(casa).value;
    select.dispatchEvent(new Event('input', { bubbles: true }));
    select.dispatchEvent(new Event('change', { bubbles: true }));
    return select.value;
  })()`;
}

/** Traduz as respostas do Jev num passo a executar, ou numa parada com motivo. */
function decidir(respostas, candidatos = [], limiares = LIMIARES) {
  const operacao = respostas.operacao;
  if (!operacao || typeof operacao.choice !== 'string') {
    return { parar: 'resposta do Jev veio sem operacao' };
  }
  let escolha = operacao.choice;
  let forcaOp = forca(operacao);
  if (forcaOp < limiares.operacao) {
    return { parar: `operacao indecisa (${distribuicao(operacao)})` };
  }
  if (TERMINAIS.has(escolha)) {
    // DONE responde a pagina (`concluido`), BLOCKED responde a propria escolha — nao ha confirmacao
    // de tela para "daqui nao da".
    const confirmado = escolha === 'DONE' ? respostas.concluido?.noul : forcaOp;
    const prova = typeof confirmado === 'number' ? confirmado : 0;
    if (prova < limiares.conclusao) {
      // Encerrar em duvida desperdica a corrida inteira. A segunda colocada e uma acao reversivel:
      // se ela se sustenta sozinha, o laco anda e o ciclo seguinte re-observa.
      const alternativa = Object.entries(operacao.probabilities ?? {})
        .filter(([k]) => !TERMINAIS.has(k))
        .sort((a, b) => b[1] - a[1])[0];
      if (!alternativa || alternativa[1] < limiares.operacao) {
        return {
          parar: escolha === 'DONE'
            ? `a pagina nao confirma que acabou (${prova.toFixed(2)}); nao ha acao melhor (${distribuicao(operacao)})`
            : `BLOCKED fraco demais para encerrar (${distribuicao(operacao)})`,
        };
      }
      [escolha, forcaOp] = alternativa;
    } else if (escolha === 'DONE') return { sucesso: true, parar: `objetivo atingido, pagina confirma (${prova.toFixed(2)})` };
    else return { parar: `o Jev diz que daqui nao da (${forcaOp.toFixed(2)})` };
  }

  const arriscado = respostas.arriscado?.noul;
  if (typeof arriscado === 'number' && arriscado >= limiares.arriscado) {
    return { parar: `operacao arriscada (${arriscado.toFixed(2)}): precisa da ordem do usuario` };
  }
  if (escolha === 'SCROLL_DOWN') return { operacao: 'SCROLL_DOWN' };

  const head = respostas[HEAD_DA_OPERACAO[escolha]];
  const gemeos = sinonimos(head, candidatos);
  const forcaAlvo = gemeos ? gemeos.massa : forca(head);
  if (!head || head.choice === NENHUM || forcaAlvo < limiares.alvo) {
    return { parar: `${escolha} sem alvo confiavel (${distribuicao(head) || forcaAlvo.toFixed(2)})` };
  }
  const alvo = gemeos ? gemeos.ref : head.choice;
  const escolhido = candidatos.find((c) => c.ref === alvo);
  if (!escolhido) return { parar: `o Jev escolheu ${head.choice}, que nao esta na pagina` };
  return { operacao: escolha, alvo, escolhido, confianca: forcaAlvo };
}

/** O que o LLM de texto recebe: só o necessário para escrever um valor de campo. */
const SEM_VALOR = 'SEM_VALOR';

function pedidoDeTexto(objetivo, campo, feito) {
  return [
    `Objetivo do usuario: ${objetivo}`,
    `Ja feito: ${feito.slice(-5).join('; ') || 'nada ainda'}`,
    `Campo a preencher: ${campo}`,
    'Responda APENAS com o valor a digitar nesse campo, sem aspas, sem explicacao, sem prefixo.',
    // Sem esta saida o modelo responde em prosa ("Nao tenho a data de nascimento de...") e a frase
    // inteira vai parar dentro do campo do formulario.
    `Se o objetivo nao contem esse dado, responda exatamente ${SEM_VALOR} e nada mais.`,
  ].join('\n');
}

const ehSegredo = (campo) => CHAVE_SECRETA.test(campo);

/**
 * De onde sai o texto de um campo, nesta ordem: o que o chamador mandou em `dados` (o Jev só
 * escolhe qual chave), depois o LLM pequeno. Sem nenhum dos dois, para dizendo qual campo faltou —
 * quem chamou sabe o valor e pode mandar na próxima.
 */
async function valorDoCampo({ campo, escolha, cofre, objetivo, feito, escreverTexto, limiares = LIMIARES }) {
  const chave = escolha?.choice;
  if (chave && chave !== NENHUM && forca(escolha) >= limiares.alvo && typeof cofre[chave] === 'string') {
    return { valor: cofre[chave], origem: `dados.${chave}` };
  }
  // Por que `dados` nao serviu: sem isso a parada nao distingue "nenhuma chave casou" de "o Jev
  // ficou em duvida entre duas chaves", que pedem correcoes diferentes de quem chamou.
  const porque = Object.keys(cofre).length
    ? `o Jev nao casou nenhum valor com esse campo (${distribuicao(escolha) || 'sem resposta'})`
    : 'nenhum dado foi passado';
  // `falta` é o campo isolado, para quem chamou reinvocar com o dado sem ter que interpretar a
  // frase. O navegador guarda a página, então a segunda chamada continua de onde esta parou.
  // Credencial NUNCA e inventada: o modelo de texto escreveria uma senha plausivel e o laco a
  // digitaria no formulario. Segredo so entra por `dados`, que quem chamou passa sabendo o valor.
  if (ehSegredo(campo.nome)) {
    return { parou: `"${campo.nome}" e campo de credencial: o valor tem que vir em --dados, nao e inventado`, falta: campo.nome };
  }
  if (!escreverTexto) {
    return { parou: `precisa digitar em "${campo.nome}": ${porque}, e nao ha modelo de texto configurado`, falta: campo.nome };
  }
  const valor = (await escreverTexto(pedidoDeTexto(objetivo, campo.nome, feito))).trim();
  if (!valor || valor.includes(SEM_VALOR)) {
    return { parou: `o campo "${campo.nome}" precisa de um dado que nao esta no objetivo: ${porque}`, falta: campo.nome };
  }
  return { valor, origem: 'llm' };
}

/**
 * Roda o laço até concluir, parar por limiar ou estourar `maxPassos`.
 * `perguntar(estado, perguntas)` fala com o Jev, `executar(verbo, args)` com o navegador e
 * `escreverTexto(pedido)` com o LLM pequeno.
 */
async function rodar({ objetivo, dados, maxPassos = 15, perguntar, executar, escreverTexto, log = () => {} }) {
  const feito = [];
  let anterior = null;
  let repetidas = 0;
  for (let passo = 1; passo <= maxPassos; passo += 1) {
    const [snapshot, url, texto] = [
      await executar('snapshot', []),
      await executar('url', []),
      await executar('text', []),
    ];
    // O navegador responde erro como TEXTO. Sem esta checagem, um `erro: a aba nao descongelou`
    // seria lido como uma página sem elementos e o laço reportaria a página errada em vez da falha.
    for (const [verbo, saida] of [['snapshot', snapshot], ['url', url], ['text', texto]]) {
      if (String(saida).startsWith('erro:')) return { feito, sucesso: false, parou: `o navegador falhou no ${verbo}: ${saida}` };
    }
    const candidatos = parsarSnapshot(snapshot);
    const anotacao = await valoresAtuais(executar);
    const estado = montarEstado(objetivo, url, feito, texto, anotacao);
    if (!candidatos.length) {
      // Página de sucesso costuma não ter nada clicável: DONE ainda precisa ser perguntado, e com
      // a mesma prova do laço normal — a página confirmando, não só o Jev preferindo DONE.
      const { concluido } = await perguntar(estado, { concluido: montarPerguntas([]).concluido });
      const prova = typeof concluido?.noul === 'number' ? concluido.noul : 0;
      return {
        feito,
        sucesso: prova >= LIMIARES.conclusao,
        parou: prova >= LIMIARES.conclusao
          ? `objetivo atingido, pagina confirma (${prova.toFixed(2)})`
          : `a pagina nao tem elemento acionavel e nao confirma conclusao (${prova.toFixed(2)})`,
      };
    }

    const cofre = dados && typeof dados === 'object' ? dados : {};
    const respostas = await perguntar(estado, montarPerguntas(candidatos, cofre, anotacao.editaveis, anotacao.lista_aberta));
    const decisao = decidir(respostas, candidatos);
    if (decisao.parar) return { feito, sucesso: decisao.sucesso === true, parou: decisao.parar };

    // As refs sao indice de arvore, nao identidade de no: renumeram a cada re-render (nesta mesma
    // tela, 40 refs com o formulario aberto e 10 com o popover aberto). Entre o snapshot e a
    // decisao passa uma requisicao ao Jev, tempo de sobra para a pagina redesenhar — entao o nome
    // da ref escolhida e reconferido antes de agir.
    if (decisao.escolhido) {
      const agora = parsarSnapshot(await executar('snapshot', []));
      const mesmo = agora.find((c) => c.ref === decisao.alvo);
      if (!mesmo || mesmo.nome !== decisao.escolhido.nome) {
        log(`jev-objetivo passo ${passo}: @${decisao.alvo} mudou de "${decisao.escolhido.nome}" para "${mesmo?.nome ?? '(sumiu)'}" — reobservando`);
        continue;
      }
    }

    // Insistir no mesmo alvo nao leva a lugar nenhum: ou a acao nao pega, ou o alvo esta errado.
    // A assinatura NAO pode incluir o estado da pagina: um dropdown que abre e fecha alterna o
    // snapshot a cada volta e escapava da guarda — 21 cliques seguidos no mesmo combobox.
    const assinatura = `${decisao.operacao}@${decisao.alvo ?? '-'}`;
    repetidas = assinatura === anterior ? repetidas + 1 : 0;
    anterior = assinatura;
    if (repetidas >= 2) {
      return { feito, sucesso: false, parou: `${decisao.operacao} em @${decisao.alvo} repetiu 3x sem avancar; parei` };
    }

    if (decisao.operacao === 'SELECT') {
      log(`jev-objetivo passo ${passo}: SELECT @${decisao.alvo} ("${decisao.escolhido.nome}") conf=${decisao.confianca.toFixed(2)}`);
      const saida = await executar('eval', [jsDeSelecionar(decisao.escolhido.nome)]);
      if (String(saida).includes(SEM_SELECT)) await executar('click', [`@${decisao.alvo}`]);
      else if (String(saida).includes('erro:')) return { feito, sucesso: false, parou: `SELECT falhou: ${saida}` };
      feito.push(`SELECT "${decisao.escolhido.nome}"`);
    } else if (decisao.operacao === 'SCROLL_DOWN') {
      log(`jev-objetivo passo ${passo}: SCROLL_DOWN`);
      await executar('press', ['PageDown']);
      feito.push('SCROLL_DOWN');
    } else if (decisao.operacao === 'TYPE_TEXT') {
      const { valor, origem, parou, falta } = await valorDoCampo({
        campo: decisao.escolhido,
        escolha: respostas[cabecaDoValor(decisao.alvo)],
        cofre,
        objetivo,
        feito,
        escreverTexto,
      });
      if (parou) return { feito, sucesso: false, parou, falta };
      const mostrado = ehSegredo(decisao.escolhido.nome) ? '(oculto)' : valor.slice(0, 40);
      log(`jev-objetivo passo ${passo}: TYPE_TEXT @${decisao.alvo} ("${decisao.escolhido.nome}" <- ${mostrado} via ${origem}) conf=${decisao.confianca.toFixed(2)}`);
      if (ehEditavel(decisao.escolhido, anotacao.editaveis)) {
        // O campo de busca do popover nao tem nome acessivel, entao nao tem ref: o caminho e abrir
        // e digitar no que ganhou foco. Clicar num popover JA aberto o fecharia, dai a conferida.
        const aberto = await executar('eval', [jsDeEstarAberto(decisao.escolhido.nome)]);
        if (!String(aberto).includes('true')) {
          await executar('click', [`@${decisao.alvo}`]);
          await executar('wait', ['--idle']);
        }
        await executar('type', [valor]);
        // Sem esta espera o ciclo seguinte fotografa a lista ainda vazia e o Jev nao tem no que
        // mirar; o proprio laco entao re-mirava o campo e a guarda de repeticao o matava.
        log(`jev-objetivo passo ${passo}: ${await executar('eval', [JS_ESPERAR_OPCOES])}`);
      } else {
        await executar('fill', [`@${decisao.alvo}`, valor]);
      }
      feito.push(`TYPE_TEXT "${decisao.escolhido.nome}"`);
    } else {
      log(`jev-objetivo passo ${passo}: CLICK @${decisao.alvo} (${rotulo(decisao.escolhido)}) conf=${decisao.confianca.toFixed(2)}`);
      await executar('click', [`@${decisao.alvo}`]);
      feito.push(`CLICK ${rotulo(decisao.escolhido)}`);
    }
    await executar('wait', ['--idle']);
  }
  return { feito, sucesso: false, parou: `estourou ${maxPassos} passos` };
}

/** `confere`: uma pergunta sobre o fato que a tela mostra, com o mesmo piso da conclusão do laço. */
function perguntaDeConfere(estado) {
  return {
    chegou: {
      type: 'noul',
      instructions: `The page now shows this state: ${estado}. `
        + 'Loading, a spinner, an error or a different screen mean this is false.',
    },
  };
}

function chegouNoEstado(respostas) {
  const p = respostas?.chegou?.noul;
  return typeof p === 'number' ? { p, ok: p >= LIMIARES.conclusao } : { p: 0, ok: false };
}

module.exports = {
  parsarSnapshot, montarPerguntas, montarEstado, decidir, rodar, pedidoDeTexto,
  valorDoCampo, cabecaDoValor, jsDeSelecionar, jsDeEstarAberto, normalizar,
  perguntaDeConfere, chegouNoEstado,
  LIMIARES, NENHUM, OPERACOES, SEM_SELECT, MAX_CANDIDATOS,
};
