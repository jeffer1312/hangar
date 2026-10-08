//! Tipos do app-server do Codex que o Hangar lê ou escreve. Escritos à mão a partir do schema
//! emitido pelo binário (`codex app-server generate-json-schema --experimental`): só o usado, todo
//! campo opcional e tolerante a campo novo. O teste do recorte (`schema_check.rs`) quebra quando um
//! campo usado some ou muda de nome na versão conferida.
use serde::{Deserialize,Deserializer,Serialize};
use serde_json::Value;

#[derive(Clone,Debug,PartialEq,Eq,Hash,PartialOrd,Ord,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum RequestId { Integer(i64), String(String) }

/// Campo ausente = `None`; `null` = `Some(None)`. O motor distingue "não veio" de "veio vazio".
fn present<'de,D:Deserializer<'de>,T:Deserialize<'de>>(d:D) -> Result<Option<Option<T>>,D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug)]
pub struct DecodeError { pub method:String, pub error:serde_json::Error }

/// Só a categoria do erro: a mensagem do serde pode ecoar valores da conversa.
pub fn error_kind(error:&serde_json::Error) -> &'static str {
    use serde_json::error::Category;
    match error.classify() { Category::Io=>"io",Category::Syntax=>"syntax",Category::Data=>"data",Category::Eof=>"eof" }
}

macro_rules! wire {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        #[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
        #[cfg_attr(test,derive(schemars::JsonSchema))]
        #[serde(rename_all = "camelCase", default)]
        pub struct $name { $($body)* }
    };
}

// ---------- pedidos do cliente ----------

wire!(pub struct ClientInfo { pub name:String, pub title:Option<String>, pub version:String });
wire!(pub struct InitializeCapabilities { pub experimental_api:bool });
wire!(pub struct GetAccountParams { pub refresh_token:bool });
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(tag="type")]
pub enum LoginAccountParams {
    #[serde(rename="chatgptDeviceCode")] ChatgptDeviceCode,
}
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(tag="type")]
pub enum LoginAccountResponse {
    #[serde(rename="chatgptDeviceCode")] ChatgptDeviceCode {
        #[serde(rename="loginId")] login_id:String,
        #[serde(rename="verificationUrl")] verification_url:String,
        #[serde(rename="userCode")] user_code:String,
    },
}
wire!(pub struct CancelLoginAccountParams { pub login_id:String });
wire!(pub struct CancelLoginAccountResponse { pub status:String });
wire!(pub struct AccountLoginCompletedNotification { pub login_id:Option<String>, pub success:bool });
wire!(pub struct InitializeParams { pub client_info:ClientInfo, pub capabilities:Option<InitializeCapabilities> });

wire!(pub struct ThreadStartParams {
    pub cwd:Option<String>,
    pub model:Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub approval_policy:Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub sandbox:Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub service_tier:Option<String>,
});
wire!(pub struct ThreadResumeParams { pub thread_id:String });
wire!(pub struct ThreadReadParams { pub thread_id:String, pub include_turns:bool });
wire!(pub struct ThreadCompactStartParams { pub thread_id:String });
wire!(pub struct ThreadUnsubscribeParams { pub thread_id:String });
wire!(pub struct ThreadBackgroundTerminalsTerminateParams { pub thread_id:String, pub process_id:String });
// `Settings` do `collaborationMode` é snake_case no schema; não usa a macro.
#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(default)]
pub struct CollaborationSettings { pub model:String, pub reasoning_effort:Option<String>, pub developer_instructions:Option<String> }
wire!(pub struct CollaborationMode { pub mode:String, pub settings:CollaborationSettings });
wire!(pub struct ThreadSettingsUpdateParams {
    pub thread_id:String,
    #[serde(skip_serializing_if = "Option::is_none")] pub model:Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub effort:Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub collaboration_mode:Option<CollaborationMode>,
    #[serde(skip_serializing_if = "Option::is_none")] pub service_tier:Option<String>,
});
wire!(pub struct TurnStartParams {
    pub thread_id:String,
    #[serde(skip_serializing_if = "Option::is_none")] pub approval_policy:Option<String>,
    /// Sem ele o pensamento só fica cifrado no rollout.
    #[serde(skip_serializing_if = "Option::is_none")] pub summary:Option<String>,
    /// Montado pelo `prepare_prompt` (texto, skill, imagem); passa como veio.
    pub input:Vec<Value>,
});
wire!(pub struct TurnSteerParams { pub thread_id:String, pub expected_turn_id:String, pub input:Vec<Value> });
wire!(pub struct TurnInterruptParams { pub thread_id:String, pub turn_id:String });
wire!(pub struct ModelListParams {});
wire!(pub struct SkillsListParams { pub cwds:Vec<String> });

#[derive(Clone,Debug,PartialEq,Serialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(tag = "method", content = "params")]
pub enum ClientRequest {
    #[serde(rename = "initialize")] Initialize(InitializeParams),
    #[serde(rename = "thread/start")] ThreadStart(ThreadStartParams),
    #[serde(rename = "thread/resume")] ThreadResume(ThreadResumeParams),
    #[serde(rename = "thread/read")] ThreadRead(ThreadReadParams),
    #[serde(rename = "thread/settings/update")] ThreadSettingsUpdate(ThreadSettingsUpdateParams),
    #[serde(rename = "thread/compact/start")] ThreadCompactStart(ThreadCompactStartParams),
    #[serde(rename = "thread/unsubscribe")] ThreadUnsubscribe(ThreadUnsubscribeParams),
    #[serde(rename = "thread/backgroundTerminals/terminate")] ThreadBackgroundTerminalsTerminate(ThreadBackgroundTerminalsTerminateParams),
    #[serde(rename = "turn/start")] TurnStart(TurnStartParams),
    #[serde(rename = "turn/steer")] TurnSteer(TurnSteerParams),
    #[serde(rename = "turn/interrupt")] TurnInterrupt(TurnInterruptParams),
    #[serde(rename = "model/list")] ModelList(ModelListParams),
    #[serde(rename = "skills/list")] SkillsList(SkillsListParams),
    #[serde(rename = "account/rateLimits/read")] AccountRateLimitsRead,
    #[serde(rename = "account/read")] AccountRead(GetAccountParams),
    #[serde(rename = "account/login/start")] AccountLoginStart(LoginAccountParams),
    #[serde(rename = "account/login/cancel")] AccountLoginCancel(CancelLoginAccountParams),
}

impl ClientRequest {
    pub const METHODS:&[&str] = &["initialize","thread/start","thread/resume","thread/read","thread/settings/update",
        "thread/compact/start","thread/unsubscribe","thread/backgroundTerminals/terminate","turn/start","turn/steer",
        "turn/interrupt","model/list","skills/list","account/rateLimits/read","account/read","account/login/start","account/login/cancel"];

    pub fn into_parts(self) -> (&'static str,Value) {
        let mut value = serde_json::to_value(&self).expect("pedido serializa");
        let method = value["method"].as_str().and_then(|m|Self::METHODS.iter().find(|k|**k == m)).copied().expect("método conhecido");
        let params = value.get_mut("params").map(Value::take).unwrap_or_else(||Value::Object(Default::default()));
        (method,params)
    }
}

// ---------- respostas ----------

wire!(pub struct InitializeResponse { pub user_agent:String });
wire!(pub struct TurnError { pub message:String, pub additional_details:Option<String>, pub codex_error_info:Option<Value> });
// Sem `items`: o histórico pode ter MBs e o motor lê os itens crus, um a um, ao hidratar.
wire!(pub struct Turn { pub id:String, pub status:String, pub error:Option<TurnError> });

#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadStatus {
    Idle,
    Active,
    NotLoaded,
    SystemError,
    #[default] #[serde(other)] Unknown,
}

#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct Thread {
    pub id:String,
    pub path:Option<String>,
    pub status:ThreadStatus,
    pub turns:Vec<Turn>,
    #[serde(deserialize_with = "present")] pub model:Option<Option<String>>,
    #[serde(deserialize_with = "present")] pub reasoning_effort:Option<Option<String>>,
}

#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadStartResponse {
    pub thread:Thread,
    pub model:Option<String>,
    pub reasoning_effort:Option<String>,
    #[serde(deserialize_with = "present")] pub service_tier:Option<Option<String>>,
}
pub type ThreadResumeResponse = ThreadStartResponse;
wire!(pub struct ThreadReadResponse { pub thread:Thread });
wire!(pub struct TurnStartResponse { pub turn:Turn });
wire!(pub struct ReasoningEffortOption { pub reasoning_effort:String, pub description:String });
#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct Model {
    pub model:String,
    pub display_name:String,
    pub description:String,
    pub hidden:bool,
    pub supported_reasoning_efforts:Vec<ReasoningEffortOption>,
    pub default_reasoning_effort:String,
    /// Lido como `Value`: a tela recebe a lista como veio.
    pub service_tiers:Option<Vec<Value>>,
    pub default_service_tier:Option<String>,
}
wire!(pub struct ModelListResponse { pub data:Vec<Model> });
wire!(pub struct RateLimitWindow { pub used_percent:Option<f64>, pub window_duration_mins:Option<i64>, pub resets_at:Option<i64> });
wire!(pub struct RateLimitSnapshot { pub limit_id:Option<String>, pub primary:Option<RateLimitWindow>, pub secondary:Option<RateLimitWindow> });
wire!(pub struct GetAccountRateLimitsResponse { pub rate_limits:RateLimitSnapshot });
wire!(pub struct TokenUsageBreakdown { pub input_tokens:i64, pub output_tokens:i64, pub cached_input_tokens:i64, pub reasoning_output_tokens:i64, pub total_tokens:i64 });
wire!(pub struct ThreadTokenUsage { pub last:TokenUsageBreakdown, pub total:TokenUsageBreakdown, pub model_context_window:Option<i64> });

// ---------- itens ----------

wire!(pub struct AsyncUserInputQuestion { pub title:String, pub options:Option<Vec<Value>> });

#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum UserInput {
    Text { #[serde(default)] text:String },
    #[serde(other)] Unknown,
}

#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ThreadItem {
    AgentMessage { #[serde(default)] id:String, #[serde(default)] text:String, #[serde(default)] delivery:Option<String>,
        #[serde(default)] questions:Option<Vec<AsyncUserInputQuestion>> },
    UserMessage { #[serde(default)] id:String, #[serde(default)] content:Vec<UserInput> },
    Reasoning { #[serde(default)] id:String },
    CommandExecution { #[serde(default)] id:String, #[serde(default)] process_id:Option<String> },
    ContextCompaction { #[serde(default)] id:String },
    #[default] #[serde(other)] Unknown,
}

// ---------- notificações ----------

wire!(pub struct ServerRequestResolvedNotification { pub thread_id:String, pub request_id:Option<RequestId> });
wire!(pub struct TurnStartedNotification { pub thread_id:String, pub turn:Turn });
wire!(pub struct TurnCompletedNotification { pub thread_id:String, pub turn:Turn });
wire!(pub struct ThreadStatusChangedNotification { pub thread_id:String, pub status:ThreadStatus });
#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadSettings {
    pub model:Option<String>,
    #[serde(deserialize_with = "present")] pub effort:Option<Option<String>>,
    #[serde(deserialize_with = "present")] pub collaboration_mode:Option<Option<CollaborationMode>>,
    #[serde(deserialize_with = "present")] pub service_tier:Option<Option<String>>,
}
wire!(pub struct ThreadSettingsUpdatedNotification { pub thread_id:String, pub thread_settings:ThreadSettings });
wire!(pub struct AgentMessageDeltaNotification { pub thread_id:String, pub turn_id:String, pub item_id:String, pub delta:String });
wire!(pub struct ReasoningSummaryTextDeltaNotification { pub thread_id:String, pub turn_id:String, pub delta:String, pub summary_index:i64 });
wire!(pub struct ReasoningTextDeltaNotification { pub thread_id:String, pub turn_id:String, pub delta:String });
wire!(pub struct ReasoningSummaryPartAddedNotification { pub thread_id:String, pub turn_id:String, pub summary_index:i64 });
wire!(pub struct ModelReroutedNotification { pub thread_id:String, pub to_model:Option<String> });
wire!(pub struct ItemStartedNotification { pub thread_id:String, pub turn_id:String, pub item:ThreadItem });
pub type ItemCompletedNotification = ItemStartedNotification;
wire!(pub struct ModelSafetyBufferingUpdatedNotification { pub thread_id:String, pub turn_id:String, pub show_buffering_ui:Option<bool> });
wire!(pub struct ThreadTokenUsageUpdatedNotification { pub thread_id:String, pub token_usage:Option<ThreadTokenUsage> });
wire!(pub struct AccountRateLimitsUpdatedNotification { pub rate_limits:Option<RateLimitSnapshot> });
wire!(pub struct ErrorNotification { pub thread_id:String, pub error:TurnError, pub will_retry:bool });
wire!(pub struct HookOutputEntry { pub kind:String, pub text:Option<String> });
wire!(pub struct HookRunSummary { pub event_name:String, pub status:String, pub source_path:Option<String>, pub entries:Vec<HookOutputEntry> });
wire!(pub struct HookCompletedNotification { pub thread_id:String, pub run:HookRunSummary });

#[derive(Clone,Debug,PartialEq,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(tag = "method", content = "params")]
pub enum ServerNotification {
    #[serde(rename = "serverRequest/resolved")] ServerRequestResolved(ServerRequestResolvedNotification),
    #[serde(rename = "turn/started")] TurnStarted(TurnStartedNotification),
    #[serde(rename = "turn/completed")] TurnCompleted(TurnCompletedNotification),
    #[serde(rename = "thread/status/changed")] ThreadStatusChanged(ThreadStatusChangedNotification),
    #[serde(rename = "thread/settings/updated")] ThreadSettingsUpdated(ThreadSettingsUpdatedNotification),
    #[serde(rename = "item/agentMessage/delta")] AgentMessageDelta(AgentMessageDeltaNotification),
    #[serde(rename = "item/reasoning/summaryTextDelta")] ReasoningSummaryTextDelta(ReasoningSummaryTextDeltaNotification),
    #[serde(rename = "item/reasoning/textDelta")] ReasoningTextDelta(ReasoningTextDeltaNotification),
    #[serde(rename = "item/reasoning/summaryPartAdded")] ReasoningSummaryPartAdded(ReasoningSummaryPartAddedNotification),
    #[serde(rename = "model/rerouted")] ModelRerouted(ModelReroutedNotification),
    #[serde(rename = "item/started")] ItemStarted(ItemStartedNotification),
    #[serde(rename = "item/completed")] ItemCompleted(ItemCompletedNotification),
    #[serde(rename = "model/safetyBuffering/updated")] ModelSafetyBufferingUpdated(ModelSafetyBufferingUpdatedNotification),
    #[serde(rename = "thread/tokenUsage/updated")] ThreadTokenUsageUpdated(ThreadTokenUsageUpdatedNotification),
    #[serde(rename = "account/rateLimits/updated")] AccountRateLimitsUpdated(AccountRateLimitsUpdatedNotification),
    #[serde(rename = "account/login/completed")] AccountLoginCompleted(AccountLoginCompletedNotification),
    #[serde(rename = "error")] Error(ErrorNotification),
    #[serde(rename = "hook/completed")] HookCompleted(HookCompletedNotification),
    /// Método que o Hangar não usa: ignorado sem erro.
    #[serde(skip)] Unknown,
}

// ---------- pedidos do servidor ----------

wire!(pub struct CommandExecutionRequestApprovalParams { pub thread_id:String, pub command:Option<String>, pub cwd:Option<String>, pub reason:Option<String> });
wire!(pub struct FileChangeRequestApprovalParams { pub thread_id:String, pub grant_root:Option<String>, pub reason:Option<String> });
wire!(pub struct ToolRequestUserInputQuestion { pub id:String, pub header:String, pub question:String, pub is_other:Option<bool>,
    pub is_secret:Option<bool>, pub options:Option<Vec<Value>> });
wire!(pub struct ToolRequestUserInputParams { pub thread_id:String, pub questions:Vec<ToolRequestUserInputQuestion> });
wire!(pub struct ThreadOnlyParams { pub thread_id:String });

#[derive(Clone,Debug,PartialEq,Deserialize)]
#[cfg_attr(test,derive(schemars::JsonSchema))]
#[serde(tag = "method", content = "params")]
pub enum ServerRequest {
    #[serde(rename = "item/commandExecution/requestApproval")] CommandExecutionApproval(CommandExecutionRequestApprovalParams),
    #[serde(rename = "item/fileChange/requestApproval")] FileChangeApproval(FileChangeRequestApprovalParams),
    #[serde(rename = "item/tool/requestUserInput")] ToolRequestUserInput(ToolRequestUserInputParams),
    /// Atendidos na 5B; na 5A continuam recusados com `-32601`, mas já reconhecidos.
    #[serde(rename = "item/permissions/requestApproval")] PermissionsApproval(ThreadOnlyParams),
    #[serde(rename = "mcpServer/elicitation/request")] McpServerElicitation(ThreadOnlyParams),
    #[serde(skip)] Unknown,
}

/// Lê os `params` direto do `&Value`, sem copiar: `item/completed` pode trazer MBs de saída.
macro_rules! decoder {
    ($ty:ident, { $($method:literal => $variant:ident($params:ty)),* $(,)? }) => {
        impl $ty {
            pub const METHODS:&[&str] = &[$($method),*];
            pub fn decode(method:&str,params:&Value) -> Result<Self,DecodeError> {
                let empty;
                let params = if params.is_null() { empty = Value::Object(Default::default()); &empty } else { params };
                let decoded = match method {
                    $($method => <$params as Deserialize>::deserialize(params).map(Self::$variant),)*
                    _ => return Ok(Self::Unknown),
                };
                decoded.map_err(|error|DecodeError { method:method.into(),error })
            }
        }
    };
}

decoder!(ServerNotification,{
    "serverRequest/resolved" => ServerRequestResolved(ServerRequestResolvedNotification),
    "turn/started" => TurnStarted(TurnStartedNotification),
    "turn/completed" => TurnCompleted(TurnCompletedNotification),
    "thread/status/changed" => ThreadStatusChanged(ThreadStatusChangedNotification),
    "thread/settings/updated" => ThreadSettingsUpdated(ThreadSettingsUpdatedNotification),
    "item/agentMessage/delta" => AgentMessageDelta(AgentMessageDeltaNotification),
    "item/reasoning/summaryTextDelta" => ReasoningSummaryTextDelta(ReasoningSummaryTextDeltaNotification),
    "item/reasoning/textDelta" => ReasoningTextDelta(ReasoningTextDeltaNotification),
    "item/reasoning/summaryPartAdded" => ReasoningSummaryPartAdded(ReasoningSummaryPartAddedNotification),
    "model/rerouted" => ModelRerouted(ModelReroutedNotification),
    "item/started" => ItemStarted(ItemStartedNotification),
    "item/completed" => ItemCompleted(ItemCompletedNotification),
    "model/safetyBuffering/updated" => ModelSafetyBufferingUpdated(ModelSafetyBufferingUpdatedNotification),
    "thread/tokenUsage/updated" => ThreadTokenUsageUpdated(ThreadTokenUsageUpdatedNotification),
    "account/rateLimits/updated" => AccountRateLimitsUpdated(AccountRateLimitsUpdatedNotification),
    "account/login/completed" => AccountLoginCompleted(AccountLoginCompletedNotification),
    "error" => Error(ErrorNotification),
    "hook/completed" => HookCompleted(HookCompletedNotification),
});
decoder!(ServerRequest,{
    "item/commandExecution/requestApproval" => CommandExecutionApproval(CommandExecutionRequestApprovalParams),
    "item/fileChange/requestApproval" => FileChangeApproval(FileChangeRequestApprovalParams),
    "item/tool/requestUserInput" => ToolRequestUserInput(ToolRequestUserInputParams),
    "item/permissions/requestApproval" => PermissionsApproval(ThreadOnlyParams),
    "mcpServer/elicitation/request" => McpServerElicitation(ThreadOnlyParams),
});

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_fields_and_missing_optionals_are_tolerated() {
        let n = ServerNotification::decode("turn/started",&json!({"threadId":"t","turn":{"id":"u","status":"inProgress","novo":1}})).unwrap();
        let ServerNotification::TurnStarted(n) = n else { panic!() };
        assert_eq!((n.thread_id.as_str(),n.turn.id.as_str()),("t","u"));
        let n = ServerNotification::decode("item/agentMessage/delta",&json!({"delta":"oi"})).unwrap();
        assert!(matches!(n,ServerNotification::AgentMessageDelta(d) if d.delta == "oi" && d.turn_id.is_empty()));
    }

    #[test]
    fn unknown_method_is_unknown_not_error() {
        assert!(matches!(ServerNotification::decode("thread/novidade",&json!({"x":1})),Ok(ServerNotification::Unknown)));
        assert!(matches!(ServerRequest::decode("foo/bar",&json!({})),Ok(ServerRequest::Unknown)));
    }

    #[test]
    fn wrong_type_on_known_method_is_error_with_method() {
        let err = ServerNotification::decode("item/agentMessage/delta",&json!({"delta":5})).unwrap_err();
        assert_eq!(err.method,"item/agentMessage/delta");
    }

    #[test]
    fn null_params_decode_as_empty() {
        assert!(matches!(ServerNotification::decode("turn/started",&Value::Null),Ok(ServerNotification::TurnStarted(_))));
    }

    #[test]
    fn presence_is_kept_for_settings() {
        let ServerNotification::ThreadSettingsUpdated(n) = ServerNotification::decode("thread/settings/updated",
            &json!({"threadId":"t","threadSettings":{"model":"m","effort":null}})).unwrap() else { panic!() };
        assert_eq!(n.thread_settings.effort,Some(None));
        assert_eq!(n.thread_settings.service_tier,None);
        assert_eq!(n.thread_settings.model,Some("m".into()));
    }

    #[test]
    fn thread_item_tags_and_fallback() {
        let item:ThreadItem = serde_json::from_value(json!({"type":"agentMessage","id":"a","text":"x","delivery":"async","questions":[{"title":"Q","options":["s"]}]})).unwrap();
        assert!(matches!(&item,ThreadItem::AgentMessage { delivery:Some(d),questions:Some(questions),.. } if d == "async" && questions[0].title == "Q"));
        let item:ThreadItem = serde_json::from_value(json!({"type":"imageGeneration","id":"z"})).unwrap();
        assert!(matches!(item,ThreadItem::Unknown));
    }

    #[test]
    fn null_optionals_on_agent_message_are_tolerated() {
        // O Codex manda `questions` e `delivery` como `null` em toda mensagem sem pergunta.
        let message = json!({"type":"agentMessage","id":"a","text":"x","questions":null,"delivery":null});
        let item:ThreadItem = serde_json::from_value(message.clone()).unwrap();
        assert!(matches!(item,ThreadItem::AgentMessage { questions:None,delivery:None,.. }));
        let n = ServerNotification::decode("item/completed",&json!({"threadId":"t","turnId":"u","item":message})).unwrap();
        assert!(matches!(n,ServerNotification::ItemCompleted(n) if matches!(n.item,ThreadItem::AgentMessage { .. })));
    }

    #[test]
    fn malformed_item_does_not_sink_the_turn() {
        // O hidratar do motor lê item a item assim: o torto vira `Unknown`, o vizinho segue.
        let items = json!([{"type":"agentMessage","text":5},{"type":"agentMessage","id":"a","text":"ok"}]);
        let typed:Vec<_> = items.as_array().unwrap().iter().map(|item|ThreadItem::deserialize(item).unwrap_or(ThreadItem::Unknown)).collect();
        assert!(matches!(typed[0],ThreadItem::Unknown));
        assert!(matches!(&typed[1],ThreadItem::AgentMessage { text,.. } if text == "ok"));
        let response:ThreadReadResponse = serde_json::from_value(json!({"thread":{"id":"t","turns":[{"id":"u","status":"completed","items":items}]}})).unwrap();
        assert_eq!(response.thread.turns[0].id,"u");
    }

    #[test]
    fn client_requests_keep_the_wire_shape() {
        let (method,params) = ClientRequest::ThreadStart(ThreadStartParams { cwd:Some("/p".into()),model:None,
            approval_policy:Some("never".into()),sandbox:Some("danger-full-access".into()),service_tier:None }).into_parts();
        assert_eq!(method,"thread/start");
        assert_eq!(params,json!({"cwd":"/p","model":null,"approvalPolicy":"never","sandbox":"danger-full-access"}));
        let (_,params) = ClientRequest::ThreadSettingsUpdate(ThreadSettingsUpdateParams { thread_id:"t".into(),
            effort:Some(None),..Default::default() }).into_parts();
        assert_eq!(params,json!({"threadId":"t","effort":null}));
        let (_,params) = ClientRequest::TurnStart(TurnStartParams { thread_id:"t".into(),input:vec![json!({"type":"text","text":"oi"})],
            approval_policy:None,summary:None }).into_parts();
        assert_eq!(params,json!({"threadId":"t","input":[{"type":"text","text":"oi"}]}));
        let (method,params) = ClientRequest::AccountRateLimitsRead.into_parts();
        assert_eq!((method,params),("account/rateLimits/read",json!({})));
    }

    #[test]
    fn client_request_methods_match_the_variants() {
        use ClientRequest as C;
        let all = vec![C::Initialize(Default::default()),C::ThreadStart(Default::default()),C::ThreadResume(Default::default()),
            C::ThreadRead(Default::default()),C::ThreadSettingsUpdate(Default::default()),C::ThreadCompactStart(Default::default()),
            C::ThreadUnsubscribe(Default::default()),C::ThreadBackgroundTerminalsTerminate(Default::default()),C::TurnStart(Default::default()),
            C::TurnSteer(Default::default()),C::TurnInterrupt(Default::default()),C::ModelList(Default::default()),
            C::SkillsList(Default::default()),C::AccountRateLimitsRead,C::AccountRead(Default::default()),
            C::AccountLoginStart(LoginAccountParams::ChatgptDeviceCode),C::AccountLoginCancel(Default::default())];
        // Sem curinga: variante nova não compila até entrar na lista acima.
        for request in &all {
            match request { C::Initialize(_)|C::ThreadStart(_)|C::ThreadResume(_)|C::ThreadRead(_)|C::ThreadSettingsUpdate(_)
                |C::ThreadCompactStart(_)|C::ThreadUnsubscribe(_)|C::ThreadBackgroundTerminalsTerminate(_)|C::TurnStart(_)
                |C::TurnSteer(_)|C::TurnInterrupt(_)|C::ModelList(_)|C::SkillsList(_)|C::AccountRateLimitsRead|C::AccountRead(_)
                |C::AccountLoginStart(_)|C::AccountLoginCancel(_) => {} }
        }
        let methods:Vec<_> = all.into_iter().map(|request|request.into_parts().0).collect();
        assert_eq!(methods,ClientRequest::METHODS);
    }

    #[test]
    fn methods_lists_match_the_variants() {
        // A tabela do `decoder!` tem de casar com o `rename` de cada variante (é ele que o recorte confere).
        for method in ServerNotification::METHODS {
            let decoded = ServerNotification::decode(method,&json!({})).unwrap();
            assert!(!matches!(decoded,ServerNotification::Unknown),"{method}");
            assert_eq!(decoded,serde_json::from_value(json!({"method":method,"params":{}})).unwrap(),"{method}");
        }
        for method in ServerRequest::METHODS {
            let decoded = ServerRequest::decode(method,&json!({})).unwrap();
            assert!(!matches!(decoded,ServerRequest::Unknown),"{method}");
            assert_eq!(decoded,serde_json::from_value(json!({"method":method,"params":{}})).unwrap(),"{method}");
        }
    }
}
