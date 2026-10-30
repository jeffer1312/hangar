//! O que `GET /api/credenciais` tem de responder a cada estado de disco; `quotas` semeia o cache de cota.

const BODY_1: &str = r#"[
{"id":"claude:⟦HOME⟧/.claude","tipo":"claude","auth_method":"oauth","codex_account":null,"codex_sync":null,"nome":"default","nome_natural":"default","apelido":null,"ativa":true,"path":"⟦HOME⟧/.claude","login":{"estado":"ok","loggedIn":true,"email":null,"plano":null,"motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":[],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"default","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true}
]"#;
const QUOTAS_1: &str = r#"[
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"claude:⟦HOME⟧/.claude","label":"default","provedor":"claude","ativa":true,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"codex:⟦HOME⟧/.codex","label":"Codex","provedor":"codex","ativa":false,"idade_s":null,"refresh_expires_at":null}
]"#;
const QUOTAS_2: &str = r#"[
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"claude:⟦HOME⟧/.claude","label":"default","provedor":"claude","ativa":true,"idade_s":null,"refresh_expires_at":null}
]"#;

pub struct Expected {
    pub status: u16,
    pub content_type: Option<&'static str>,
    pub body: &'static str,
}

pub struct Case {
    pub name: &'static str,
    pub query: &'static str,
    pub codex_cli: bool,
    pub files: &'static [(&'static str, &'static str)],
    pub files_hex: &'static [(&'static str, &'static str)],
    pub quotas: Option<&'static str>,
    /// `None`: a parte com problema sai sozinha e só a conta Claude fica na lista.
    pub expected: Option<Expected>,
}

pub const CASES: &[Case] = &[
    Case {
        name: "completo",
        query: "",
        codex_cli: true,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true, "email": "a@b", "subscriptionType": "max"}"#),
            (".claude-work/.hangar-conta", ""),
            (".claude-work/.fake-auth.json", r#"{"loggedIn": false}"#),
            (".claude-semcli/.hangar-conta", ""),
            (".claude-backup/.credentials.json", "{}"),
            (".claude-backup/projects/x", ""),
            (".claude-pend/.hangar-conta", ""),
            (".claude-pend/.hangar-account-pending", ""),
            (".claude/.hangar-apelidos.json", r#"{"claude:⟦HOME⟧/.claude-work": "  Firma  ", "claude:⟦HOME⟧/.claude-semcli": "   ", "chave:kimi": "Kimi Pessoal", "codex:⟦HOME⟧/.codex-work": "Trabalho", "kimi:gen": "Gerado"}"#),
            (".claude/engines.json", r#"{"kimi": {"label": "Kimi", "base_url": "https://api.kimi.com/coding", "api_key": "sk-kimi-abcdefg4f2a", "model": "k3"}, "curta": {"base_url": "https://x.example", "api_key": "1234567890", "model": "m"}, "oc": {"label": "", "base_url": "https://opencode.ai/zen/go", "api_key": "", "model": "m"}, "oc2": {"label": 0, "base_url": "https://opencode.ai/zen", "api_key": "çççççççççççç", "model": "m"}, "cc": {"label": "CommandCode", "base_url": "https://api.commandcode.ai", "api_key": "k", "model": "m"}, "semchave": {"model": "m"}, "Nome Ruim": {"base_url": "https://y", "api_key": "abc", "model": "m"}, "vazio": {"base_url": "", "api_key": 5, "model": "m"}}"#),
            (".claude/.hangar-opencode.json", r#"{"chave:oc": {"workspace_id": "ws", "auth_cookie": "c"}, "chave:oc2": {"workspace_id": "ws", "auth_cookie": ""}}"#),
            (".codex/.fake-account.json", r#"{"account": {"type": "chatgpt", "email": "e@x", "planType": "pro"}}"#),
            (".codex-work/.hangar-codex-conta", r#"{"version": 1, "id": "work"}"#),
            (".codex-work/.fake-account.json", r#"{"account": {"type": "apiKey"}}"#),
            (".codex-off/.hangar-codex-conta", r#"{"version": 1, "id": "off"}"#),
            (".codex-off/.fake-account.json", r#"{"account": null}"#),
            (".codex-ruim/.hangar-codex-conta", r#"{"version": 1, "id": "outro"}"#),
            (".kimi-code/config.toml", r#"default_model = "apikey/k3"
[providers.apikey]
type = "kimi"
api_key = "sk-a"
base_url = "https://api.kimi.com/coding/v1"
# >>> hangar: provedor gen (app/agentes_sync) — gerado, não editar à mão
[providers.gen]
type = "kimi"
api_key = "sk-g"
base_url = "https://api.kimi.com/coding/v1"
# <<< hangar: fim do provedor gen
[providers.oauth]
type = "kimi"
base_url = "https://api.kimi.com"
"#),
        ],
        files_hex: &[],
        quotas: Some(r#"[
{"estado":"lida","janelas":[{"rotulo":"5h","pct":13,"reset_ts":1800000000},{"rotulo":"Fable","pct":2.5,"reset_ts":null,"por_modelo":true}],"ts":null,"motivo":null,"reset_credits":null,"id":"claude:⟦HOME⟧/.claude","label":"default","provedor":"claude","ativa":true,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"claude:⟦HOME⟧/.claude-work","label":"Firma","provedor":"claude","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"lida","janelas":[{"rotulo":"5h","pct":40.0,"reset_ts":null,"por_modelo":false}],"ts":null,"motivo":null,"reset_credits":{"available_count":2,"credits":[{"id":"r1","expires_at":1800000000,"title":"Grátis"}]},"id":"codex:⟦HOME⟧/.codex","label":"Codex","provedor":"codex","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"codex:⟦HOME⟧/.codex-work","label":"Trabalho","provedor":"codex","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"formato-desconhecido","reset_credits":null,"id":"kimi:apikey","label":"apikey","provedor":"kimi","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"lida","janelas":[{"rotulo":"5h","pct":13,"reset_ts":1800000000},{"rotulo":"Fable","pct":2.5,"reset_ts":null,"por_modelo":true}],"ts":null,"motivo":null,"reset_credits":null,"id":"kimi:gen","label":"Gerado","provedor":"kimi","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"lida","janelas":[{"rotulo":"5h","pct":13,"reset_ts":1800000000},{"rotulo":"Fable","pct":2.5,"reset_ts":null,"por_modelo":true}],"ts":null,"motivo":null,"reset_credits":null,"id":"chave:kimi","label":"Kimi Pessoal","provedor":"kimi","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"chave:curta","label":"curta","provedor":"kimi","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"chave:cc","label":"CommandCode","provedor":"commandcode","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"claude:⟦HOME⟧/.claude-semcli","label":"semcli","provedor":"claude","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"codex:⟦HOME⟧/.codex-off","label":"off","provedor":"codex","ativa":false,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"chave:oc2","label":"oc2","provedor":"kimi","ativa":false,"idade_s":null,"refresh_expires_at":null}
]"#),
        expected: Some(Expected { status: 200, content_type: Some("application/json"), body: r#"[
{"id":"claude:⟦HOME⟧/.claude","tipo":"claude","auth_method":"oauth","codex_account":null,"codex_sync":null,"nome":"default","nome_natural":"default","apelido":null,"ativa":true,"path":"⟦HOME⟧/.claude","login":{"estado":"ok","loggedIn":true,"email":"a@b","plano":"max","motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":[],"cota":{"estado":"lida","janelas":[{"rotulo":"5h","pct":13.0,"reset_ts":1800000000.0,"por_modelo":false},{"rotulo":"Fable","pct":2.5,"reset_ts":null,"por_modelo":true}],"ts":null,"idade_s":null,"motivo":null,"label":"default","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"claude:⟦HOME⟧/.claude-semcli","tipo":"claude","auth_method":"oauth","codex_account":null,"codex_sync":null,"nome":"semcli","nome_natural":"semcli","apelido":null,"ativa":false,"path":"⟦HOME⟧/.claude-semcli","login":{"estado":"indisponivel","loggedIn":null,"email":null,"plano":null,"motivo":"cli-indisponivel","refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":[],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"semcli","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"claude:⟦HOME⟧/.claude-work","tipo":"claude","auth_method":"oauth","codex_account":null,"codex_sync":null,"nome":"Firma","nome_natural":"Firma","apelido":"Firma","ativa":false,"path":"⟦HOME⟧/.claude-work","login":{"estado":"ok","loggedIn":false,"email":null,"plano":null,"motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":[],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"Firma","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"chave:kimi","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"Kimi Pessoal","nome_natural":"Kimi","apelido":"Kimi Pessoal","ativa":false,"path":null,"login":null,"base_url":"https://api.kimi.com/coding","chave_mascarada":"sk-kimi••••4f2a","usos":["claude_code"],"cota":{"estado":"lida","janelas":[{"rotulo":"5h","pct":13.0,"reset_ts":1800000000.0,"por_modelo":false},{"rotulo":"Fable","pct":2.5,"reset_ts":null,"por_modelo":true}],"ts":null,"idade_s":null,"motivo":null,"label":"Kimi Pessoal","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"chave:curta","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"curta","nome_natural":"curta","apelido":null,"ativa":false,"path":null,"login":null,"base_url":"https://x.example","chave_mascarada":"••••••••••","usos":["claude_code"],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"curta","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"chave:oc","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"oc","nome_natural":"oc","apelido":null,"ativa":false,"path":null,"login":null,"base_url":"https://opencode.ai/zen/go","chave_mascarada":null,"usos":["claude_code"],"cota":null,"aceita_cookie":true,"cookie_definido":true,"gerenciada":true},
{"id":"chave:oc2","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"oc2","nome_natural":"oc2","apelido":null,"ativa":false,"path":null,"login":null,"base_url":"https://opencode.ai/zen","chave_mascarada":"ççççççç••••çççç","usos":["claude_code"],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"oc2","reset_credits":null},"aceita_cookie":true,"cookie_definido":false,"gerenciada":true},
{"id":"chave:cc","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"CommandCode","nome_natural":"CommandCode","apelido":null,"ativa":false,"path":null,"login":null,"base_url":"https://api.commandcode.ai","chave_mascarada":"•","usos":["claude_code"],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"CommandCode","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"chave:semchave","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"semchave","nome_natural":"semchave","apelido":null,"ativa":false,"path":null,"login":null,"base_url":null,"chave_mascarada":null,"usos":["claude_code"],"cota":null,"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"chave:vazio","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"vazio","nome_natural":"vazio","apelido":null,"ativa":false,"path":null,"login":null,"base_url":"","chave_mascarada":null,"usos":["claude_code"],"cota":null,"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"codex:⟦HOME⟧/.codex","tipo":"codex","auth_method":"oauth","codex_account":"default","codex_sync":null,"nome":"default","nome_natural":"default","apelido":null,"ativa":true,"path":"⟦HOME⟧/.codex","login":{"estado":"ok","loggedIn":true,"email":"e@x","plano":"pro","motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":["codex_cli"],"cota":{"estado":"lida","janelas":[{"rotulo":"5h","pct":40.0,"reset_ts":null,"por_modelo":false}],"ts":null,"idade_s":null,"motivo":null,"label":"Codex","reset_credits":{"available_count":2,"credits":[{"id":"r1","expires_at":1800000000,"title":"Grátis","description":null,"status":"unknown"}]}},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"codex:⟦HOME⟧/.codex-off","tipo":"codex","auth_method":"none","codex_account":"off","codex_sync":"idle","nome":"off","nome_natural":"off","apelido":null,"ativa":false,"path":"⟦HOME⟧/.codex-off","login":{"estado":"ok","loggedIn":false,"email":null,"plano":null,"motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":["codex_cli"],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"off","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"codex:⟦HOME⟧/.codex-work","tipo":"codex","auth_method":"api_key","codex_account":"work","codex_sync":"idle","nome":"Trabalho","nome_natural":"work","apelido":"Trabalho","ativa":false,"path":"⟦HOME⟧/.codex-work","login":{"estado":"ok","loggedIn":true,"email":null,"plano":null,"motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":["codex_cli"],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"Trabalho","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"kimi:apikey","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"apikey","nome_natural":"apikey","apelido":null,"ativa":false,"path":null,"login":null,"base_url":null,"chave_mascarada":null,"usos":["kimi_cli"],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"formato-desconhecido","label":"apikey","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":false},
{"id":"kimi:gen","tipo":"chave","auth_method":"api_key","codex_account":null,"codex_sync":null,"nome":"Gerado","nome_natural":"Gerado","apelido":"Gerado","ativa":false,"path":null,"login":null,"base_url":null,"chave_mascarada":null,"usos":["kimi_cli"],"cota":{"estado":"lida","janelas":[{"rotulo":"5h","pct":13.0,"reset_ts":1800000000.0,"por_modelo":false},{"rotulo":"Fable","pct":2.5,"reset_ts":null,"por_modelo":true}],"ts":null,"idade_s":null,"motivo":null,"label":"Gerado","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true}
]"# }),
    },
    Case {
        name: "forcar_rele_cota",
        query: "?forcar=true",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": false}"#),
        ],
        files_hex: &[],
        quotas: Some(r"[]"),
        expected: Some(Expected { status: 200, content_type: Some("application/json"), body: r#"[
{"id":"claude:⟦HOME⟧/.claude","tipo":"claude","auth_method":"oauth","codex_account":null,"codex_sync":null,"nome":"default","nome_natural":"default","apelido":null,"ativa":true,"path":"⟦HOME⟧/.claude","login":{"estado":"ok","loggedIn":false,"email":null,"plano":null,"motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":[],"cota":{"estado":"sem_credencial","janelas":[],"ts":null,"idade_s":null,"motivo":"credencial-ilegivel","label":"default","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true}
]"# }),
    },
    Case {
        name: "codex_estoura_prazo",
        query: "",
        codex_cli: true,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".codex/.fake-account.json", r#"{"account": {"type": "apiKey"}}"#),
            (".codex/.fake-delay", ""),
        ],
        files_hex: &[],
        quotas: Some(QUOTAS_1),
        expected: Some(Expected { status: 200, content_type: Some("application/json"), body: r#"[
{"id":"claude:⟦HOME⟧/.claude","tipo":"claude","auth_method":"oauth","codex_account":null,"codex_sync":null,"nome":"default","nome_natural":"default","apelido":null,"ativa":true,"path":"⟦HOME⟧/.claude","login":{"estado":"ok","loggedIn":true,"email":null,"plano":null,"motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":[],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"default","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"codex:⟦HOME⟧/.codex","tipo":"codex","auth_method":"unknown","codex_account":"default","codex_sync":null,"nome":"default","nome_natural":"default","apelido":null,"ativa":true,"path":"⟦HOME⟧/.codex","login":{"estado":"indisponivel","loggedIn":null,"email":null,"plano":null,"motivo":"prazo","refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":["codex_cli"],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"Codex","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true}
]"# }),
    },
    Case {
        name: "codex_sem_cli",
        query: "",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".codex/config.toml", ""),
        ],
        files_hex: &[],
        quotas: Some(QUOTAS_1),
        expected: Some(Expected { status: 200, content_type: Some("application/json"), body: r#"[
{"id":"claude:⟦HOME⟧/.claude","tipo":"claude","auth_method":"oauth","codex_account":null,"codex_sync":null,"nome":"default","nome_natural":"default","apelido":null,"ativa":true,"path":"⟦HOME⟧/.claude","login":{"estado":"ok","loggedIn":true,"email":null,"plano":null,"motivo":null,"refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":[],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"default","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true},
{"id":"codex:⟦HOME⟧/.codex","tipo":"codex","auth_method":"unknown","codex_account":"default","codex_sync":null,"nome":"default","nome_natural":"default","apelido":null,"ativa":true,"path":"⟦HOME⟧/.codex","login":{"estado":"indisponivel","loggedIn":null,"email":null,"plano":null,"motivo":"cli-ausente","refreshExpiresAt":null},"base_url":null,"chave_mascarada":null,"usos":["codex_cli"],"cota":{"estado":"indisponivel","janelas":[],"ts":null,"idade_s":null,"motivo":"http-404","label":"Codex","reset_credits":null},"aceita_cookie":false,"cookie_definido":false,"gerenciada":true}
]"# }),
    },
    Case {
        name: "engines_fora_do_utf8",
        query: "",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".claude/engines.json", ""),
        ],
        files_hex: &[(".claude/engines.json", "7b2261223aff7d")],
        quotas: None,
        expected: None,
    },
    Case {
        name: "motor_nao_objeto",
        query: "",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".claude/engines.json", r#"{"a": "texto"}"#),
        ],
        files_hex: &[],
        quotas: None,
        expected: None,
    },
    Case {
        name: "motor_label_numero",
        query: "",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".claude/engines.json", r#"{"a": {"label": 5, "base_url": "x", "api_key": "k"}}"#),
        ],
        files_hex: &[],
        quotas: Some(r#"[
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"claude:⟦HOME⟧/.claude","label":"default","provedor":"claude","ativa":true,"idade_s":null,"refresh_expires_at":null},
{"estado":"indisponivel","janelas":[],"ts":null,"motivo":"http-404","reset_credits":null,"id":"chave:a","label":5,"provedor":"kimi","ativa":false,"idade_s":null,"refresh_expires_at":null}
]"#),
        expected: None,
    },
    Case {
        name: "motor_base_url_lista",
        query: "",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".claude/engines.json", r#"{"a": {"base_url": ["x"], "api_key": "k"}}"#),
        ],
        files_hex: &[],
        quotas: Some(QUOTAS_2),
        expected: None,
    },
    Case {
        name: "engines_quebrado",
        query: "",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".claude/engines.json", "{nao json"),
        ],
        files_hex: &[],
        quotas: Some(QUOTAS_2),
        expected: Some(Expected { status: 200, content_type: Some("application/json"), body: BODY_1 }),
    },
    Case {
        name: "engines_com_bom",
        query: "",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".claude/engines.json", "\u{feff}{\"a\": {\"base_url\": \"x\", \"api_key\": \"k\"}}"),
        ],
        files_hex: &[],
        quotas: Some(QUOTAS_2),
        expected: Some(Expected { status: 200, content_type: Some("application/json"), body: BODY_1 }),
    },
    Case {
        name: "vazio",
        query: "",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".claude/engines.json", "{}"),
        ],
        files_hex: &[],
        quotas: Some(QUOTAS_2),
        expected: Some(Expected { status: 200, content_type: Some("application/json"), body: BODY_1 }),
    },
    Case {
        name: "query_invalida",
        query: "?forcar=talvez",
        codex_cli: false,
        files: &[
            (".claude/.fake-auth.json", r#"{"loggedIn": true}"#),
            (".claude/engines.json", "{}"),
        ],
        files_hex: &[],
        quotas: Some(QUOTAS_2),
        expected: Some(Expected { status: 422, content_type: Some("application/json"), body: r#"{"detail":[{"type":"bool_parsing","loc":["query","forcar"],"msg":"Input should be a valid boolean, unable to interpret input","input":"talvez"}]}"# }),
    },
];
