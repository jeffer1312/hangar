import { describe, it, expect } from 'vitest';
import { lerRecadoBastao } from './bastaoRecado';

// O texto REAL do `bastao.kickoff` (backend), não uma paráfrase: o parser existe pra ler aquele
// formato, e um fixture inventado passaria verde contra um formato que ninguém manda.
const KICKOFF = [
  '[hangar: passagem de bastão] Você continua o trabalho da sessão `melhorar-skills` — não é tarefa nova, é a mesma, no ponto em que ela parou.',
  'Comece lendo, com um `Read`, o dossiê em `/home/u/.claude/.hangar-bastao/melhorar-skillsb.md`: onde o trabalho está, o que já está no disco e por que as decisões foram tomadas.',
  'Leia o plano e o contrato citados no dossiê ANTES de mexer em qualquer arquivo — o dossiê diz onde parou, o plano diz o que vem em seguida.',
  'A sessão `melhorar-skills` continua VIVA, mas parou de escrever: daqui pra frente quem escreve no diretório é você (um escritor por árvore — as duas compartilham o mesmo cwd).',
  'Se o dossiê mostrar par ou grupo, a passagem NÃO move esses vínculos: troque a linha da tabela de papéis para o SEU nome e avise o par (`hangar-send`) que o endereço agora é você.',
  'Ela vinha de conta `Felizardo e Batista` · modelo `Opus5 (high✦)` — você pode estar em outra.',
].join('\n');

describe('lerRecadoBastao', () => {
  it('tira origem, dossiê, conta e modelo do kick-off real', () => {
    expect(lerRecadoBastao(KICKOFF)).toEqual({
      origem: 'melhorar-skills',
      dossie: '/home/u/.claude/.hangar-bastao/melhorar-skillsb.md',
      conta: 'Felizardo e Batista',
      modelo: 'Opus5 (high✦)',
    });
  });

  it('sem conta/modelo conhecidos, o resto continua valendo', () => {
    const sem = KICKOFF.replace(
      /Ela vinha de.*$/,
      'A conta e o modelo de onde ela vinha estão na primeira seção do dossiê — você pode estar em outros.',
    );
    expect(lerRecadoBastao(sem)).toMatchObject({ origem: 'melhorar-skills', conta: '', modelo: '' });
  });

  it('mensagem normal do usuário não vira cartão', () => {
    expect(lerRecadoBastao('passagem de bastão pra outra sessão, como faço?')).toBeNull();
  });

  it('recado de outra sessão (prefixo `[de: x]`) não vira cartão', () => {
    expect(lerRecadoBastao('[de: hangar-b2] Você continua o trabalho da sessão `x`')).toBeNull();
  });

  it('kick-off sem os avisos que o cartão AFIRMA volta null (o texto muda, o cartão sai)', () => {
    const mudou = KICKOFF.replace('continua VIVA', 'já morreu');
    expect(lerRecadoBastao(mudou)).toBeNull();
  });

  it('sem o caminho do dossiê não há cartão — é o que ele existe pra dar', () => {
    const semDossie = KICKOFF.replace(/o dossiê em `[^`]+`/, 'o dossiê da sessão');
    expect(lerRecadoBastao(semDossie)).toBeNull();
  });
});
