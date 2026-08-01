// Preferencia do APARELHO, nao do servidor: ligar maos-livres no celular nao pode ligar no desktop.
// Por isso localStorage e nao runtime_config, no mesmo padrao das prefs de aparencia (prefixo cp_).
const KEY = 'cp_ditado_maos_livres';

export function lerMaosLivres(): boolean {
  try {
    return localStorage.getItem(KEY) === '1';
  } catch {
    return false;   // modo privado / storage bloqueado: o modo simplesmente nao existe
  }
}

export function setMaosLivres(v: boolean): void {
  try {
    localStorage.setItem(KEY, v ? '1' : '0');
  } catch { /* modo privado */ }
}
