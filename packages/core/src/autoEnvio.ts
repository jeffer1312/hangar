/** Por que a gravacao terminou. Compartilhado com o Composer de proposito: uniao declarada duas
 *  vezes deixa um typo compilar e virar "nunca envia", calado. */
export type MotivoFim = 'silencio' | 'botao' | 'teto' | 'escondeu';

// Uma expressao so, de proposito: cinco regras separadas convidam a cinco caminhos de codigo, e
// basta um deles esquecer uma condicao pra sair uma mensagem que ninguem revisou.
// SO envia sozinho quem foi encerrado pelo SILENCIO — todo outro motivo de fim significa que nao
// houve sinal de "terminei de falar". Erro de transcricao nao aparece aqui porque cai no catch do
// chamador, que nem chega a perguntar.
export function podeEnviarSozinho(p: {
  motivo: MotivoFim | null;
  texto: string;
  aviso?: string | null;
  rascunhoAntes: boolean;
}): boolean {
  return p.motivo === 'silencio' && !p.aviso && !p.rascunhoAntes && p.texto.trim().length > 0;
}
