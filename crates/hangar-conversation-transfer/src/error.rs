use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct TransferError {
    pub code: String,
    pub status: u16,
    pub params: Value,
}
impl TransferError {
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_owned(),
            status: 409,
            params: json!({}),
        }
    }
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
    pub fn with_params(mut self, params: Value) -> Self {
        self.params = params;
        self
    }
    pub fn public(&self) -> Value {
        let mut message=match self.code.as_str() {
            "session_transfer_source_changed" => "A conversa mudou; abra a seleção de conta novamente.",
            "session_transfer_restore_failed" => "A troca falhou e o Claude não voltou. Use Recarregar para tentar recuperar.",
            "session_transfer_busy" => "A sessão está trocando de agente; tente novamente quando terminar.",
            "session_transfer_gate_unavailable" => "A sessão está terminando uma escrita; tente de novo em instantes.",
            "session_transfer_context_budget_exceeded" => "O histórico completo excede a capacidade disponível do modelo escolhido. Escolha outro modelo.",
            "session_transfer_model_capacity_unknown" => "Não foi possível confirmar a capacidade de contexto do modelo escolhido.",
            "session_transfer_model_media_unsupported" => "O modelo escolhido não aceita as imagens presentes na conversa.",
            "session_transfer_codex_version_unsupported" => "A versão instalada do Codex não oferece suporte a esta transferência.",
            "session_transfer_login_required" => "Entre na conta Codex escolhida antes de transferir a conversa.",
            "session_transfer_unknown_account" => "A conta Codex escolhida não está cadastrada neste servidor.",
            "session_transfer_account_full" => "A conta Codex escolhida está sem cota disponível.",
            "session_transfer_account_unavailable" => "Não foi possível consultar a conta Codex escolhida.",
            "session_transfer_invalid_model_choice" => "O modelo ou esforço escolhido não está disponível no catálogo desta conta.",
            "session_transfer_native_unavailable" => "Não foi possível acessar o Codex instalado neste servidor.",
            "session_transfer_queue_pending" => "Há mensagens na fila ou entregas ainda não confirmadas. Aguarde a confirmação antes de transferir.",
            "session_transfer_source_busy" => "A sessão tem trabalho em execução ou uma pergunta ou permissão pendente.",
            "session_transfer_source_state_unknown" => "Não foi possível confirmar que a sessão de origem está parada.",
            "session_transfer_invalid_permission_mode" => "Não foi possível comprovar a permissão da origem ou a permissão anterior ao modo Plano.",
            "session_transfer_read_only" => "Uma sessão protegida para somente leitura não pode ser transferida.",
            "session_transfer_source_item_too_large" => "Um item da conversa excede o limite de transporte do Codex; a transferência integral foi recusada.",
            "session_transfer_source_media_missing" => "Uma imagem necessária do histórico não está disponível na origem.",
            "session_transfer_unsupported_media_mime" => "O formato de uma mídia da conversa não é aceito pelo Codex.",
            "session_transfer_invalid_media_schema" => "Uma mídia da origem está incompleta ou tem formato inválido.",
            "session_transfer_invalid_source_json" => "O histórico da origem contém um registro JSON inválido.",
            "session_transfer_invalid_source_schema" => "O histórico da origem contém um registro de contexto inválido.",
            "session_transfer_source_partial_line" => "O histórico da origem termina com um registro incompleto.",
            "session_transfer_source_unreadable" => "Não foi possível ler o histórico da origem.",
            "session_transfer_no_source_context" => "Não há contexto disponível na origem para transferir.",
            "session_transfer_no_source_leaf" => "Não foi possível identificar a conversa ativa no histórico da origem.",
            "session_transfer_source_leaf_invalid" => "A referência à conversa ativa na origem é inválida.",
            "session_transfer_source_parent_missing" => "Falta um registro anterior necessário para reconstruir o histórico completo.",
            "session_transfer_source_cycle" => "As referências do histórico da origem formam um ciclo inválido.",
            "session_transfer_source_tool_id_ambiguous" => "Não foi possível associar com segurança uma chamada ao resultado histórico da ferramenta.",
            "session_transfer_unsupported_context" => "A origem contém um tipo de contexto que ainda não pode ser preservado no Codex.",
            "session_transfer_unsupported_source_topology" => "A organização do histórico da origem não permite reconstruir a conversa completa.",
            "session_transfer_native_config_changed" => "A configuração do Codex mudou durante o preparo. Abra a seleção novamente.",
            "session_transfer_native_settings_mismatch" => "As escolhas carregadas pelo Codex não correspondem ao destino confirmado.",
            "session_transfer_import_persistence_mismatch" => "O histórico persistido no Codex diverge da origem; a transferência foi recusada.",
            "session_transfer_import_persistence_unconfirmed" => "Não foi possível confirmar a gravação integral do histórico no Codex.",
            "session_transfer_native_import_failed" => "O Codex recusou a importação do histórico.",
            "session_transfer_history_invalid" => "O histórico da transferência está indisponível ou não passou na conferência de integridade.",
            "session_transfer_invalid_record" => "O registro da transferência não corresponde à conversa solicitada.",
            _ => "Não foi possível transferir a conversa.",
        }.to_owned();
        if self.code == "session_transfer_restore_failed" {
            message.push(' ');
            message.push_str(match self.params["phase"].as_str().unwrap_or_default() {
                "session_transfer_source_changed" => "A identidade da origem mudou.",
                "session_transfer_process_identity_changed" => {
                    "O processo agora pertence a outra execução."
                }
                "session_transfer_process_identity_unknown" => {
                    "Não foi possível confirmar o dono do processo."
                }
                "session_transfer_source_not_stopped" => "O processo da origem ainda não encerrou.",
                "session_transfer_runtime_not_stopped" => {
                    "O processo de destino ainda não encerrou."
                }
                "session_transfer_import_not_stopped" => "O importador ainda não encerrou.",
                "session_transfer_import_cleanup_unconfirmed" => {
                    "A queda interrompeu a confirmação dos processos filhos do importador."
                }
                "session_transfer_runtime_cleanup_unconfirmed" => {
                    "A queda interrompeu a confirmação dos processos filhos do destino."
                }
                _ => "Não foi possível confirmar o carregamento da origem.",
            });
        }
        json!({"code":self.code,"msg":message,"params":self.params})
    }
}
impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)
    }
}
impl std::error::Error for TransferError {}
impl From<std::io::Error> for TransferError {
    fn from(_: std::io::Error) -> Self {
        Self::new("session_transfer_history_invalid")
    }
}
impl From<serde_json::Error> for TransferError {
    fn from(_: serde_json::Error) -> Self {
        Self::new("session_transfer_invalid_record")
    }
}
