//! Texto de quando delegar a uma filha ligada (lado do pai) e a linha que abre o primeiro prompt da
//! filha. O texto do pai é escrito à mão em cada superfície (descrição do `new_session`, `help.txt` do
//! `hangar-send`, bloco do `CLAUDE.md`): JSON, heredoc e ajuda quebram linha cada um do seu jeito, então
//! o que se confere é a marca `MARKER` que o abre, não o texto inteiro.

/// Abre o texto de quando delegar em toda superfície que o pai lê.
pub const MARKER: &str = "Delegar:";

/// O que a filha ligada precisa saber, depois do nome do pai (`child_prefix`). Sem a linha, a filha
/// manda a resposta final por recado à mão, além da que já volta sozinha.
pub const CHILD: &str = "sua resposta final volta sozinha ao pai; não a mande por `send` nem `hangar-send`. \
Dúvida que trava vira pergunta (AskUserQuestion), que chega ao pai.";

/// Linha única para o começo do `initial_prompt` da filha ligada a `parent`. Vai no prompt, não no
/// `SessionStart`: o vínculo é gravado depois de o processo subir, e o hook pode rodar antes dele.
pub fn child_prefix(parent: &str) -> String {
    format!("Você é filha ligada de '{parent}': {CHILD}")
}
