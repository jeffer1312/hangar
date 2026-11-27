//! Os textos que o Hangar manda sozinho para a sessão continuar. Quem decide se um turno foi pedido por
//! alguém (o retorno da filha ao pai, a corrente do registro) reconhece estes textos e os pula.

/// Queda passageira de rede, stream ou provedor.
pub const TRANSIENT: &str = "A conexão caiu no meio do turno. Continue de onde parou; se um comando estava rodando, confira o estado antes de repetir.";

/// Reboot no meio do turno e o "Reabrir e continuar".
pub const REBOOT: &str = "Continue de onde parou. O turno anterior foi cortado no meio: um comando ou ação que ele começou pode ter terminado ou ainda estar rodando. Antes de repetir algo que muda estado, confirme que não teve efeito e não está rodando; se não der para saber, diga isso em vez de repetir.";

/// A sessão reaberta depois do limite de uso ou do login recusado.
pub const LIMIT: &str = "Continue de onde parou.";

/// Igualdade depois do `trim`: o "continua" que a pessoa escreve nunca é confundido com o automático.
pub fn is_auto_continue(text: &str) -> bool {
    let text = text.trim();
    [TRANSIENT, REBOOT, LIMIT].contains(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_continue_text_is_recognized() {
        for text in [TRANSIENT, REBOOT, LIMIT] {
            assert!(is_auto_continue(text), "{text}");
            assert!(is_auto_continue(&format!("  {text}\n")), "{text}");
        }
        assert!(!is_auto_continue("continua"));
        assert!(!is_auto_continue("Continue de onde parou. E depois rode os testes."));
        assert!(!is_auto_continue(""));
    }
}
