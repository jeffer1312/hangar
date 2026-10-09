import type { Register } from "claude-code";
import { registerAsk } from "./ask";
import { registerBtw } from "./btw";
import { registerInput } from "./input";
import { registerPerm } from "./perm";
import { registerRate } from "./rate";
import { registerState } from "./state";
import { registerSuggest } from "./suggest";
import { registerUi } from "./ui";

// Um módulo por plugin é regra do engine (`hooks.json` recusa um segundo em
// `modules`), então a divisão por assunto é por ARQUIVO, composta aqui. Dois
// arquivos não podem hookar o mesmo evento sem matcher, e `$` não atravessa um
// import: cada arquivo registra os SEUS eventos e fala com a ponte sozinho.
export const register: Register = (on) => {
  registerState(on);
  registerInput(on);
  registerSuggest(on);
  registerAsk(on);
  registerPerm(on);
  registerRate(on);
  registerUi(on);
  // Depois do `ui.ts`: o painel do `/btw` fica por dentro da cadeia que ele espelha no app.
  registerBtw(on);
};
