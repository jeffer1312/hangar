# Parte 6, grupos: gestão de grupos no Rust, N sessões em M máquinas — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

Status: ready-for-human (decisões D1 e D2 tomadas; aguarda aprovação do plano e a escolha do modo de execução)

**Goal:** com o Rust de pé, o `hangar-server` passa a ser o único que grava grupos (1a); um grupo
pode ter N sessões em M máquinas com um `gid` e uma máquina dona (1b); o desktop nativo mostra cada
grupo como um bloco só, com todos os membros e a máquina de cada um (2).

**Architecture:** módulo novo `groups` no `hangar-server` com o formato de arquivo de hoje
(sidecar por membro local) mais, na 1b, um registro por grupo entre máquinas na dona; um lock por
processo, sem rede nem entrega sob ele; operações locais portadas do `pair.py` como planejadores
puros; operações entre máquinas aplicadas pela dona e entregues como registro inteiro com versão,
por uma fila em disco com um trabalhador por máquina de destino. O Python chama o Rust pela ponte
privada `/__hangar_server/groups` e só grava no modo `python`. O nativo monta um modelo global de
grupos a partir das listas de todas as máquinas.

**Tech Stack:** Rust (axum, tokio, serde_json, reqwest, chrono, sha1_smol) em
`crates/hangar-server` e `crates/hangar-api`; Python 3.14 + FastAPI em `backend/app`; GPUI Kit em
`desktop-native`; golden gerado pelo Python em `backend/tests/fixtures/contract/`.

**Spec:** [`desenho.md`](desenho.md) (mesma pasta). Linhas conferidas em `999d3571c` (no
`backend/app/api.py`, tudo depois da linha 2687 está 10 linhas acima do que o desenho cita, que é
de `9072e9aa6`); cada Task reconfere as dela antes de codar. **O código deste plano não foi
compilado no planejamento**: tipos, nomes e testes são o contrato entre as Tasks; o executor
ajusta a sintaxe ao compilar, sem mudar comportamento nem nome público.

## Lotes — o que corre em paralelo

- **Entrega 1a** (migração com paridade; branch própria a partir da `main`; pode juntar sozinha):
  Task 1 → Task 2 → Task 3 → Task 4 → Task 5 → Task 6 → (Task 7 ∥ Task 8) → Task 9 (a Task 5
  mexe nas rotas que a Task 4 cria; a 3 pode correr em paralelo com o fim da 2 se o executor
  separar `deliver.rs`, que não depende do serviço).
- **Entrega 1b** (federação; branch a partir da 1a juntada): Task 10 → Task 11 → (Task 12 ∥
  Task 13) → Task 14 → Task 15.
- **Entrega 2** (nativo; depois da 1b juntada): Task 16 → Task 17 → (Task 18 ∥ Task 19) → Task 20 →
  Task 21. As Tasks 16–17 podem começar antes (só consomem campos da lista), mas só juntam depois.

## Global Constraints

- **Testes (regra do projeto):** cada Task escreve os testes dela; rodar só quando o dono
  autorizar na abertura da execução, e então só os focados: `cd backend && uv run pytest
  tests/<x>.py` e `cd crates && nice -n 10 env CARGO_BUILD_JOBS=4 cargo test -p hangar-server
  --test <x>` (ou `--lib groups::`). No máximo 2 `cargo` na máquina (`pgrep -c -x cargo`); `target/`
  da worktree apagado no fim; `rust-analyzer` desligado na worktree.
- **Contrato interno:** cada entrega que mexe nele sobe **um** número nos dois lados, o próximo
  livre na hora de juntar (hoje 38): `backend/app/rust_server.py:35`,
  `crates/hangar-server/src/lib.rs:33` e o teste fixo `crates/hangar-server/tests/proxy.rs:33`. Na
  1a entram: variáveis do filho `HANGAR_SERVER_ID`, `HANGAR_PEERS_FILE`, `HANGAR_PAIR_ARCHIVE`; ponte
  `/groups`; campo `groups` da saúde; rotas `/internal/pair/text`, `/internal/orq/{group-phase,
  promote,associate,is-orchestrator}`, `/internal/external-pairs/end`. Na 1b, o que ela acrescentar.
- **Dono único:** no modo `rust`/`pending`, nada no Python grava em `.hangar-pair/` (sidecars,
  `groups/`, fila), nem funde/arquiva contrato; as primitivas de escrita do `pair.py` recusam (Task 6).
  Erro numa operação de grupo no Rust é erro com código, nunca repasse ao Python depois de começar.
- **Sem rede nem entrega sob o lock de grupo:** sob o lock só disco, fila e `/internal/orq/*`.
- **Paridade (1a):** toda resposta das rotas de hoje (`/pair`, `DELETE /pair`, `/group-message`,
  `/pair/contract`, `/pair-remote`, `/unpair-remote`) que o Rust passa a dar sai igual à do Python
  (corpo e código), provada por golden de `backend/tests/fixtures/contract/gen_golden.py`. Campos
  novos só acrescentam.
- **Corpo estrito:** rotas entre máquinas novas (1b) recusam campo desconhecido e levam `protocol: 1`.
- **Confiança:** máquina do `peers.json` = mesmo dono (token de dono). Par externo fica no Python e
  nunca entra em grupo; convidado (8766) e Connect (8768) não ganham rota nova; `share_gate._BLOCKED`
  ganha `group`.
- **Escopo:** sem migrar autenticação, convidados, cadastro de peers, convites, `task-suggestion`.
  O Rust só **lê** `peers.json`.
- **Dependências:** sem crate nova. `thiserror`, `async-trait` e `tower` não são dependências
  diretas: erro com `impl Display` à mão; futuro em caixa como `ServiceFuture`
  (`terminal_input.rs:7`); data com `chrono` (`Local::now().format(...)`, como
  `runtime/local_policy.rs:228`).
- **Códigos novos** entram com frase em `messages/pt.json` e `messages/en.json` e no mapa
  `packages/core/src/errosApi.ts` no mesmo commit: `erro_grupo_limpeza_falhou`, `erro_contrato_mudou` (1a);
  `erro_grupo_peer_versao_antiga`, `erro_grupo_dono_indisponivel`, `erro_grupo_dono_desconhecido`,
  `erro_grupo_fusao_entre_donos`, `erro_grupo_membro_nao_confirmado`, `erro_grupo_identificador_igual`,
  `erro_grupo_identificador_divergente`, `erro_grupo_peer_nao_registrado`,
  `erro_grupo_orq_entre_maquinas`, `erro_grupo_entre_maquinas_sem_rust`, `erro_grupo_mudou`,
  `erro_grupo_inexistente` (1b). `erro_sessao_sem_grupo` já existe
  (`errosApi.ts:303`).
- **Desempenho** (`docs/migracao-rust/README.md`, "Desempenho: erros que já custaram"): nenhum laço
  novo sem entrada que mude (varredura de 2 s só com sidecar presente; trabalhador da fila só com
  item); I/O de arquivo em `spawn_blocking`; nada de fsync sob lock; canais com limite.
- Log e diário só com código, nome de sessão, id de máquina e nome de campo; nunca texto de recado,
  de tarefa ou de contrato.
- Identificador novo em inglês; comentário curto em português, sobre o porquê. `git add` por
  caminho; commits descritivos em inglês; push só com autorização do dono.
- Ler antes: "Regras vigentes" de `docs/decisoes/windows.md` (Tasks 1, 5, 10, 14) e de
  `docs/decisoes/harnesses.md` (Tasks 3, 4, 6, 14); "Grupo: o protocolo é do HOOK…" e "Grupo de 1 só
  existe no grupo `orq`…" de `docs/decisoes/plataforma.md` (Tasks 2, 7); Coding Guides do `gpui-kit`
  e Design Guides do `gpui-kit-design-guides` (Tasks 18–20).
- Testes Python: não existe fixture `pair_dir`; o `test_pair.py:12-15` tem um `_tmp_pair_dir` autouse
  que isola `pair.settings.projects_dir`, e `_arquivo_dir` é trocado por teste (`test_pair.py:174`).
  Arquivo de teste novo traz as próprias fixtures (o `client` é por arquivo, ex. `test_api.py:36`).
- Nativo: todo texto por `tr()`/`tr_web()` com chave em `messages/pt.json` e `en.json`
  (`i18n.rs:46` falha sem as duas).

## Review Focus

1. **Máquina que não responde no meio de uma entrada de N membros** (casa confirma `b`, lab não
   responde para `c`): a resposta diz que `b` entrou e `c` não; depois que lab volta, não sobra
   sidecar de `c` em lab e `c` é avisado se chegou a receber o protocolo. Teste na Task 11
   (`join_partial_when_one_server_is_down`).
2. **Envio atrasado, repetido ou fora de ordem** (dissolveu e um envio velho chega; versão nova
   chega antes da que tinha `joining`): nenhum sidecar é criado fora do `joining` e ninguém que saiu
   volta. Testes na Task 11 (`push_never_creates_sidecar_outside_joining`,
   `late_push_does_not_resurrect_pending_leave`, `stale_push_after_dissolve_is_ignored`).
3. **Mesmo nome em duas máquinas no mesmo grupo** (`a` em casa e `a` em lab): sidecars, aviso de
   grupo, saída e o nativo tratam como dois membros. Testes nas Tasks 11
   (`same_name_on_two_servers_are_two_members`) e 17 (`same_name_two_servers`).
4. **Chave do `peers.json` diferente do `CP_SERVER_ID` da outra máquina** (`lab` cadastrada como
   `notebook`): pareamento recusado com a frase que diz os dois nomes, nada gravado. Teste na Task 10
   (`key_mismatch_is_refused_before_any_write`).
5. **Nome reusado com o Rust fora** (membro `fed` morto, reserva Python, sessão nova com o mesmo
   nome): a sessão nova não nasce no grupo, a saída do antigo fica na fila e sai quando o Rust volta.
   Teste na Task 13 (`test_python_mode_reused_name_leaves_and_queues`).

Também cobertos nas Tasks donas: lista que falhou na varredura (Task 7), aviso esperando porta de
entrada fechada não trava renomear (Task 4), convidado sem `GET /group` (Task 11), par externo
desfeito pelo `DELETE /pair` do Rust (Task 5).

---

## Entrega 1a — migração com paridade

### Task 1: Armazenamento de grupos no Rust

Status: ready-for-agent

**Files:**
- Create: `crates/hangar-server/src/groups/mod.rs`, `crates/hangar-server/src/groups/model.rs`,
  `crates/hangar-server/src/groups/store.rs`
- Modify: `crates/hangar-server/src/lib.rs:2-29` (`pub mod groups;`)
- Modify: `crates/hangar-server/src/list/links.rs:60-115` (`link_file`, `pair_of`, `legacy_gid`
  passam a usar `groups::store::file_stem` e `groups::model::Sidecar::parse`)
- Modify: `backend/tests/fixtures/contract/gen_golden.py` (caso `pair_sidecars`)
- Test: `crates/hangar-server/tests/groups_store.rs`, fixtures em
  `backend/tests/fixtures/contract/golden/pair_sidecars/`

**Interfaces:**
- Produces: `groups::model::{Fed, Sidecar, Member, GroupRecord, PROTOCOL, legacy_gid}`;
  `groups::store::{PairDir, file_stem, StoreError}`.

- [x] **Step 1: Gerar as fixtures pelo Python**

Em `gen_golden.py`, um caso que grava com `pair.PairLink(...).set(...)` num `projects_dir`
temporário os sidecars abaixo, copia os `.json` para `golden/pair_sidecars/` e grava
`expected.json` com o `PairLink(name).get()` de cada um (inclusive `legado.json` com `{"peer": "x"}`
sem `gid` e `quebrado.json` com `[1,2]`, escritos crus):

```python
CASOS_PAIR = {
    "a": dict(peers=["b"], task="T-1 filtro", gid="ab12cd34", harness={"a": "claude", "b": "codex", "z": "pi"}),
    "orq-solo": dict(peers=[], task="", gid="cd34ef56", harness={}, orq=True),
    "nome com espaço": dict(peers=["a"], task="", gid="ab12cd34", harness={}),
}
```

E um sidecar com `fed`, escrito cru (o Python de hoje não grava `fed`):

```json
{"peers": ["b", "lab::c"], "task": "t", "gid": "ab12cd34", "harness": {}, "fed": {"owner": "casa", "local": true, "version": 7}}
```

- [x] **Step 2: Escrever o teste**

```rust
// crates/hangar-server/tests/groups_store.rs
mod common;
use hangar_server::groups::model::{Fed, Sidecar};
use hangar_server::groups::store::{PairDir, file_stem};
use serde_json::Value;

fn golden_dir() -> std::path::PathBuf { common::contract().join("golden/pair_sidecars") }

#[test]
fn reads_what_python_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    for f in std::fs::read_dir(golden_dir()).unwrap() {
        let f = f.unwrap().path();
        if f.extension().is_some_and(|e| e == "json") && f.file_name().unwrap() != "expected.json" {
            std::fs::copy(&f, tmp.path().join(f.file_name().unwrap())).unwrap();
        }
    }
    let dir = PairDir::new(tmp.path().to_path_buf(), tmp.path().join("arquivo"));
    let expected: Value = serde_json::from_str(&std::fs::read_to_string(golden_dir().join("expected.json")).unwrap()).unwrap();
    for (name, want) in expected.as_object().unwrap() {
        let got = dir.sidecar(name).unwrap();
        match want {
            Value::Null => assert!(got.is_none(), "{name}"),
            w => {
                let got = got.unwrap();
                let peers: Vec<String> = w["peers"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_owned()).collect();
                assert_eq!(got.peers, peers, "{name}");
                assert_eq!(got.gid, w["gid"].as_str().unwrap(), "{name}: gid legado igual ao do Python");
                assert_eq!(got.orq, w["orq"].as_bool().unwrap(), "{name}");
            }
        }
    }
}

#[test]
fn fed_round_trip_and_python_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = PairDir::new(tmp.path().to_path_buf(), tmp.path().join("arquivo"));
    let s = Sidecar {
        peers: vec!["b".into(), "lab::c".into()], task: Some("t".into()), gid: "ab12cd34".into(),
        harness: [("a".into(), "claude".into()), ("fora".into(), "pi".into())].into(),
        orq: false, fed: Some(Fed { owner: "casa".into(), local: true, version: 7 }),
    };
    dir.write_sidecar("a", &s).unwrap();
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(tmp.path().join("a.json")).unwrap()).unwrap();
    assert!(raw.get("orq").is_none(), "orq só aparece quando é true, como o PairLink.set");
    assert!(raw["harness"].get("fora").is_none(), "harness só com membros do grupo, como o PairLink.set");
    assert_eq!(dir.sidecar("a").unwrap().unwrap().fed, s.fed);
}

#[test]
fn file_stem_matches_python_sanitize() {
    assert_eq!(file_stem("nome com espaço"), "nome-com-espa-o");
    assert_eq!(file_stem("ok_1.2-x"), "ok_1.2-x");
}

#[test]
fn sidecars_skips_registry_files() {
    // external_pairs.json e groups/ não aparecem como membro.
}
```

- [x] **Step 3: Rodar e ver falhar** — `cargo test -p hangar-server --test groups_store`.
  Esperado: não compila (`groups` não existe).

- [x] **Step 4: Escrever `model.rs`**

```rust
//! Arquivos de grupo em `.hangar-pair`. O sidecar é o formato de hoje (Python, hook e lista leem);
//! `fed` só existe em grupo entre máquinas e é ignorado por quem não o conhece.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const PROTOCOL: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fed { pub owner: String, pub local: bool, pub version: u64 }

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Sidecar {
    pub peers: Vec<String>,
    /// `None` = `null` no arquivo; a lista mostra `pair_task: null` nesse caso, como o Python.
    pub task: Option<String>,
    pub gid: String,
    pub harness: BTreeMap<String, String>,
    pub orq: bool,
    pub fed: Option<Fed>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member { pub server: String, pub name: String, #[serde(default)] pub provider: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupRecord {
    pub protocol: u32,
    pub gid: String,
    pub owner: String,
    pub version: u64,
    #[serde(default)]
    pub task: String,
    pub members: Vec<Member>,
}

use crate::list::discover_other::truthy; // a que `links.rs::pair_of` usa hoje; tornar `pub(crate)`

impl Sidecar {
    /// `PairLink.get` (pair.py:75-94), regra movida de `links.rs::pair_of` sem mudança.
    pub fn parse(name: &str, data: &Map<String, Value>) -> Option<Sidecar> {
        let raw = match data.get("peers") {
            Some(p) => p.clone(),
            None => data.get("peer").filter(|p| truthy(p)).map(|p| Value::Array(vec![p.clone()])).unwrap_or(Value::Null),
        };
        let peers: Vec<String> = raw.as_array().map(|a| a.iter().filter_map(Value::as_str).filter(|p| !p.is_empty()).map(str::to_owned).collect()).unwrap_or_default();
        let orq = data.get("orq") == Some(&Value::Bool(true));
        if peers.is_empty() && !orq {
            return None;
        }
        let task = match data.get("task") { None => Some(String::new()), Some(t) => t.as_str().map(str::to_owned) };
        let gid = data.get("gid").and_then(Value::as_str).filter(|g| !g.is_empty()).map(str::to_owned).unwrap_or_else(|| legacy_gid(name, &peers));
        let harness = data.get("harness").and_then(Value::as_object)
            .map(|h| h.iter().filter_map(|(k, v)| v.as_str().map(|p| (k.clone(), p.to_owned()))).collect())
            .unwrap_or_default();
        // `fed` torto = ausente; quem lê pelo `PairDir` registra no diário.
        let fed = data.get("fed").and_then(|f| serde_json::from_value::<Fed>(f.clone()).ok());
        Some(Sidecar { peers, task, gid, harness, orq, fed })
    }

    /// Mesmo corpo que o `PairLink.set` grava (pair.py:96-108).
    pub fn to_json(&self, name: &str) -> Value {
        let inside: Vec<&str> = std::iter::once(name).chain(self.peers.iter().map(String::as_str)).collect();
        let harness: Map<String, Value> = self.harness.iter()
            .filter(|(k, _)| inside.contains(&k.as_str()))
            .map(|(k, p)| (k.clone(), Value::String(p.clone()))).collect();
        let mut body = json!({"peers": self.peers, "task": self.task, "gid": self.gid, "harness": harness});
        if self.orq { body["orq"] = json!(true); }
        if let Some(fed) = &self.fed { body["fed"] = serde_json::to_value(fed).unwrap(); }
        body
    }
}

/// `_gid_legado` do pair.py:55, movido de `links.rs:111-115`.
pub fn legacy_gid(name: &str, peers: &[String]) -> String {
    let mut all: Vec<&str> = std::iter::once(name).chain(peers.iter().map(String::as_str)).collect();
    all.sort_unstable();
    sha1_smol::Sha1::from(all.join("\n")).digest().to_string()[..8].to_owned()
}
```

Conferir qual `truthy` o `links.rs` importa hoje (`list/discover_other.rs:601` ou
`list/context.rs:273`) e usar a mesma.

- [x] **Step 5: Escrever `store.rs`**

```rust
//! Leitura e escrita da pasta `.hangar-pair` e do arquivo de contratos. Sem lock aqui: quem
//! escreve é o `GroupService`, um por processo, sob o lock dele.
use super::model::{GroupRecord, Sidecar};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StoreError { Io(std::io::Error), Json(serde_json::Error) }
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { StoreError::Io(e) => write!(f, "io: {e}"), StoreError::Json(e) => write!(f, "json: {e}") }
    }
}
impl From<std::io::Error> for StoreError { fn from(e: std::io::Error) -> Self { StoreError::Io(e) } }
impl From<serde_json::Error> for StoreError { fn from(e: serde_json::Error) -> Self { StoreError::Json(e) } }

/// `pqueue._sanitize`: fora de `[A-Za-z0-9_.-]` vira `-`.
pub fn file_stem(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '-' }).collect()
}

pub struct PairDir { root: PathBuf, archive: PathBuf }

impl PairDir {
    pub fn new(root: PathBuf, archive: PathBuf) -> Self { Self { root, archive } }
    pub fn root(&self) -> &Path { &self.root }
    /// Ausente ou ilegível = sem grupo (como o Python); ilegível vai ao diário pelo chamador.
    pub fn sidecar(&self, name: &str) -> Result<Option<Sidecar>, StoreError>;
    pub fn write_sidecar(&self, name: &str, s: &Sidecar) -> Result<(), StoreError>;
    pub fn clear_sidecar(&self, name: &str) -> Result<(), StoreError>; // .json e .json.tmp
    /// (stem, sidecar) dos `*.json` da raiz, exceto `external_pairs.json`.
    pub fn sidecars(&self) -> Result<Vec<(String, Sidecar)>, StoreError>;
    /// `_merge_contract` (pair.py:231-258): anexa `grupo-`/`regras-` do perdedor ao sobrevivente,
    /// com o cabeçalho `## Contrato herdado do grupo <gid> (merge)`. Falha só ao diário.
    pub fn merge_contract(&self, loser: &str, survivor: &str);
    /// `_arquivar_contratos` (pair.py:271-284): `<archive>/<prefixo>-<gid>-<AAAAmmdd-HHMMSS>.md`
    /// em hora local (`chrono::Local`). Falha só ao diário.
    pub fn archive_contracts(&self, gid: &str);
    pub fn contract_path(&self, gid: &str) -> PathBuf { self.root.join(format!("grupo-{gid}.md")) }
}

/// Troca o arquivo inteiro: temporário na mesma pasta + rename, com as novas tentativas do
/// Windows (acesso negado no rename) copiadas de `hangar-workspace/src/files.rs:160-175`. Sem
/// fsync e sem a conferência de conteúdo do `files::write`, que não servem aqui.
fn replace_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()>;
```

Os métodos de registro (`record`, `write_record`, `delete_record`, `records`) e a fila entram na
Task 10, quando passam a existir.

- [x] **Step 6: Trocar `links.rs`** — `pair_of` vira `read_object` + `Sidecar::parse`; o `Pair`
  privado sai. A lista não muda (os testes `contract_list` e `list_*` existentes cobrem).

- [x] **Step 7: Rodar** `--test groups_store` e `--test list_routes`. Esperado: PASS.

- [x] **Step 8: Commit**

```bash
git add crates/hangar-server/src/groups crates/hangar-server/src/lib.rs crates/hangar-server/src/list/links.rs crates/hangar-server/tests/groups_store.rs backend/tests/fixtures/contract/gen_golden.py backend/tests/fixtures/contract/golden/pair_sidecars
git commit -m "feat(groups): read and write pair sidecars in Rust"
```

### Task 2: Operações de grupo de uma máquina (planejador puro + serviço)

Status: ready-for-agent

**Files:**
- Create: `crates/hangar-server/src/groups/local.rs`, `crates/hangar-server/src/groups/service.rs`
- Test: `crates/hangar-server/tests/groups_local.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces:

```rust
pub enum OrqPhase { Live, Ended, NotStarted, Unknown }
pub enum JoinRefusal { Mix, TaskConflict { existing: String }, AlreadyGrouped }
pub struct JoinInput<'a> { pub name: &'a str, pub others: &'a [String], pub task: &'a str,
    pub replace_task: bool, pub harness: &'a BTreeMap<String, String>, pub orq: bool }
pub struct JoinPlan { pub members: Vec<String>, pub gid: String, pub new_gid: bool, pub task: String,
    pub orq: bool, pub merged: Vec<String>, pub writes: Vec<(String, Sidecar)>,
    pub before: BTreeMap<String, Option<Sidecar>> }
pub struct LeavePlan { pub ex_peers: Vec<String>, pub clears: Vec<String>,
    pub writes: Vec<(String, Sidecar)>, pub archive: Option<String>, pub before: BTreeMap<String, Option<Sidecar>> }
pub fn plan_join(read: &dyn Fn(&str) -> Option<Sidecar>, input: &JoinInput, fresh_gid: &dyn Fn() -> String) -> Result<JoinPlan, JoinRefusal>;
pub fn plan_leave(read: &dyn Fn(&str) -> Option<Sidecar>, name: &str, orq_alive: bool) -> Option<LeavePlan>;
pub fn plan_rename(read: &dyn Fn(&str) -> Option<Sidecar>, old: &str, new: &str) -> (Vec<String>, Vec<(String, Sidecar)>);
pub fn plan_external_link(read: &dyn Fn(&str) -> Option<Sidecar>, local: &str, address: &str,
    harness: &BTreeMap<String, String>, fresh_gid: &dyn Fn() -> String) -> Result<Vec<(String, Sidecar)>, JoinRefusal>;

/// Fatos da orquestração, que moram no Python (Task 3 liga às rotas `/internal/orq/*`).
pub trait OrqFacts: Send + Sync {
    fn phase<'a>(&'a self, gid: &'a str) -> BoxFuture<'a, OrqPhase>;                       // falha = Unknown
    fn promote<'a>(&'a self, name: &'a str, gid: &'a str) -> BoxFuture<'a, Result<(), String>>; // Err = texto do conflito
}
pub struct GroupService { /* dir, lock: tokio::sync::Mutex<()>, orq: Arc<dyn OrqFacts>, server_id */ }
impl GroupService {
    pub async fn join(&self, input: JoinOwned) -> Result<JoinOutcome, GroupError>;
    pub async fn leave(&self, name: &str) -> Result<Vec<String>, GroupError>;   // ex-companheiros
    pub async fn rename(&self, old: &str, new: &str) -> Result<(), GroupError>;
    pub async fn restore(&self, before: BTreeMap<String, Option<Sidecar>>) -> Result<(), GroupError>;
    pub async fn external_link(&self, local: &str, address: &str, harness: BTreeMap<String, String>) -> Result<String, GroupError>; // gid
    pub async fn external_unlink(&self, local: &str, address: &str) -> Result<(), GroupError>;
}
pub struct JoinOwned { pub name: String, pub others: Vec<String>, pub task: String, pub replace_task: bool,
    pub harness: BTreeMap<String, String>, pub orq: bool }
pub struct JoinOutcome { pub members: Vec<String>, pub gid: String, pub task: String, pub orq: bool,
    pub newcomers: Vec<String>, pub before: BTreeMap<String, Option<Sidecar>> }
pub enum GroupError { Refused(JoinRefusal), Orq(String), Store(StoreError) }
```

`BoxFuture<'a, T>` = `Pin<Box<dyn Future<Output = T> + Send + 'a>>`, como o `ServiceFuture` de
`terminal_input.rs:7`.

- [x] **Step 1: Escrever os testes portados de `backend/tests/test_pair.py`** (conferir os nomes
  equivalentes ao portar)

```rust
// crates/hangar-server/tests/groups_local.rs
use hangar_server::groups::local::*;
use hangar_server::groups::model::Sidecar;
use std::cell::RefCell;
use std::collections::BTreeMap;

struct Disk(RefCell<BTreeMap<String, Sidecar>>);
impl Disk {
    fn new() -> Self { Disk(RefCell::new(BTreeMap::new())) }
    fn read(&self) -> impl Fn(&str) -> Option<Sidecar> + '_ { move |n| self.0.borrow().get(n).cloned() }
    fn apply(&self, writes: &[(String, Sidecar)], clears: &[String]) {
        let mut d = self.0.borrow_mut();
        for c in clears { d.remove(c); }
        for (n, s) in writes { d.insert(n.clone(), s.clone()); }
    }
}
fn none() -> BTreeMap<String, String> { BTreeMap::new() }
fn gid(s: &'static str) -> impl Fn() -> String { move || s.to_owned() }
fn input<'a>(name: &'a str, others: &'a [String], task: &'a str, h: &'a BTreeMap<String, String>) -> JoinInput<'a> {
    JoinInput { name, others, task, replace_task: false, harness: h, orq: false }
}

#[test]
fn two_solo_sessions_become_a_group() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "t", &h), &gid("g1")).ok().unwrap();
    assert_eq!(p.members, ["a", "b"]);
    assert!(p.new_gid);
    d.apply(&p.writes, &[]);
    assert_eq!(d.read()("b").unwrap().peers, ["a"]);
}

#[test]
fn third_member_keeps_gid() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let p = plan_join(&d.read(), &input("c", &["a".into()], "", &h), &gid("g2")).ok().unwrap();
    assert_eq!(p.gid, "g1");
    assert_eq!(p.members.len(), 3);
}

#[test]
fn different_task_without_replace_is_refused_before_any_write() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "x", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    match plan_join(&d.read(), &input("c", &["a".into()], "y", &h), &gid("g2")) {
        Err(JoinRefusal::TaskConflict { existing }) => assert_eq!(existing, "x"),
        _ => panic!("tarefa diferente sem replace_task precisa ser recusada"),
    }
}

#[test]
fn fusion_keeps_first_gid_and_lists_loser() {
    let (d, h) = (Disk::new(), none());
    for (a, b, g) in [("a", "b", "g1"), ("c", "d", "g2")] {
        let p = plan_join(&d.read(), &input(a, &[b.into()], "", &h), &gid(g)).ok().unwrap();
        d.apply(&p.writes, &[]);
    }
    let p = plan_join(&d.read(), &input("a", &["c".into()], "", &h), &gid("g3")).ok().unwrap();
    assert_eq!((p.gid.as_str(), p.merged.as_slice()), ("g1", &["g2".to_owned()][..]));
}

#[test]
fn external_or_remote_member_never_enters_a_local_group() {
    let (d, h) = (Disk::new(), none());
    d.apply(&[("a".into(), Sidecar { peers: vec!["fulano::x".into()], task: Some(String::new()), gid: "e1".into(), ..Default::default() })], &[]);
    assert!(matches!(plan_join(&d.read(), &input("b", &["a".into()], "", &h), &gid("g")), Err(JoinRefusal::Mix)));
}

#[test]
fn leave_of_pair_dissolves_and_archives() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let l = plan_leave(&d.read(), "a", false).unwrap();
    assert_eq!(l.ex_peers, ["b"]);
    assert_eq!(l.clears, ["a", "b"]);
    assert_eq!(l.archive.as_deref(), Some("g1"));
}

#[test]
fn leave_keeps_remaining_group() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into(), "c".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let l = plan_leave(&d.read(), "a", false).unwrap();
    d.apply(&l.writes, &l.clears);
    assert_eq!(d.read()("b").unwrap().peers, ["c"]);
    assert!(l.archive.is_none());
}

#[test]
fn live_orq_group_keeps_last_member() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &JoinInput { name: "arb", others: &["exec".into()], task: "", replace_task: false, harness: &h, orq: true }, &gid("o1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let l = plan_leave(&d.read(), "exec", true).unwrap();
    d.apply(&l.writes, &l.clears);
    let arb = d.read()("arb").unwrap();
    assert!(arb.peers.is_empty() && arb.orq, "grupo orq com execução viva fica com o árbitro sozinho");
    assert!(l.archive.is_none());
}

#[test]
fn rename_rewrites_own_and_peers() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let (clears, writes) = plan_rename(&d.read(), "a", "a2");
    d.apply(&writes, &clears);
    assert!(d.read()("a").is_none());
    assert_eq!(d.read()("b").unwrap().peers, ["a2"]);
}
```

No mesmo arquivo, testes do `GroupService` com `PairDir` num `tempdir` e um `OrqFacts` falso:
`join_restores_on_partial_write_failure` (pasta só leitura depois do primeiro arquivo: no fim os
sidecars são os de antes), `orq_join_promotes_once_and_restores_on_conflict` (falso devolve `Err`:
sidecars voltam, erro `GroupError::Orq`), `leave_archives_contract_file` (`grupo-g1.md` vai para o
arquivo), `external_link_refuses_grouped_session`.

- [x] **Step 2: Rodar e ver falhar** — `cargo test -p hangar-server --test groups_local`.

- [x] **Step 3: Implementar `local.rs`** portando `join_group`, `leave`, `rename_pair` e o ramo de
  par externo do `join_group` (`pair.py:162-223`, `287-323`, `353-370`) sem mudar regra, exceto: se
  a união de membros tem nome com `::`, recusa com `JoinRefusal::Mix` (o par 1:1 entre máquinas é
  da Task 5). `plan_external_link` recusa com `AlreadyGrouped` se `local` já tem sidecar e grava só
  o sidecar de `local` com `peers: [address]`, tarefa vazia, `harness` recebido e `gid` novo.

- [x] **Step 4: Implementar `service.rs`**: um `tokio::sync::Mutex<()>` por processo; dentro dele,
  ler, planejar e gravar com `spawn_blocking`; escrita que falha restaura `before` (como
  `_restore_locked`); fusão chama `merge_contract` para cada perdedor; `orq` com gid novo chama
  `OrqFacts::promote` ainda sob o lock e restaura se falhar; `leave` consulta `OrqFacts::phase` só
  quando o sidecar é `orq` e conta `Live`/`Unknown` como viva; arquivamento depois de soltar o lock
  (faxina não desfaz saída). Nada de HTTP ou entrega aqui dentro.

- [x] **Step 5: Rodar** os testes da Task. Esperado: PASS.

- [x] **Step 6: Commit** — `git commit -m "feat(groups): port single-machine group operations to Rust"`

### Task 3: Entrega de aviso, texto do protocolo e fatos da orquestração

Status: ready-for-agent

**Files:**
- Create: `crates/hangar-server/src/groups/deliver.rs`, `crates/hangar-server/src/groups/orq.rs`
- Modify: `crates/hangar-server/src/session_write/input.rs:187` (extrair o miolo do handler `input`)
- Modify: `backend/app/internal_api.py` (rotas `/internal/pair/text`,
  `/internal/orq/{group-phase,promote,associate,is-orchestrator}`)
- Modify: `crates/hangar-server/tests/fake/mod.rs` (Python falso com essas rotas e com
  `/api/sessions/{n}/input` contando chamadas)
- Test: `crates/hangar-server/tests/groups_deliver.rs`, `backend/tests/test_internal_groups.py`

**Interfaces:**
- Produces:
  - `session_write::input::deliver_text(st: &AppState, name: &str, text: &str) -> Result<(), Value>`:
    o mesmo caminho do `/input` com `steer: false` (o Python usa `_enviar`, que é isso), sem
    `Request` HTTP; provedor que o Rust não atende é repassado ao Python `/api/sessions/{name}/input`
    com o token do dono, pela mesma função que o handler usa para repassar. `Err` = o `detail` da
    resposta (envelope `{code, params, msg}` como o `_deliver`).
  - `groups::deliver::protocol_text(st, kind, args) -> Result<String, Value>` via
    `POST /internal/pair/text {kind: "group"|"orq"|"external", me, others, task, contract,
    contract_remote, harness, peer, owner}` → `{text}`.
  - `groups::orq::PythonOrq: OrqFacts` via `/internal/orq/group-phase {gid}` →
    `{"phase": "live"|"ended"|null|"unknown"}` e `/internal/orq/promote {name, gid}` → `{}` ou 409
    `erro_orq_arquivo_mudou` (o Python trata `IdentityUnavailable` como hoje: só log, 200).
  - `groups::orq::is_orchestrator(st, names) -> Result<Vec<String>, Value>` via
    `/internal/orq/is-orchestrator {names}` → `{names}` (os que `orq_runs.find` acha: a recusa
    `_recusa_orq`, `api.py:4849-4853`).
  - `/internal/orq/associate {name, gid, mtime}` → contexto do `orq_context.associate` sem
    `pair._LOCK` (usada pela ponte na Task 6).

- [x] **Step 1: Testes** — Rust: `deliver_text` numa sessão Claude com terminal falsa chega pelo
  caminho do Rust; numa sessão `codex` repassa ao Python falso com `{"text", "steer": false}`;
  erro do Python volta como envelope. Python: cada rota interna com sessão falsa, inclusive
  `promote` com `IdentityUnavailable` (200) e com `PromotionConflict` (409).
- [x] **Step 2: Rodar e ver falhar.**
- [x] **Step 3: Implementar.** Extrair o miolo do `session_write::input` numa função sem mudar o
  handler (que passa a chamá-la). As rotas Python ficam no mesmo arquivo e com a mesma autenticação
  das demais `/internal/*`.
- [x] **Step 4: Rodar.** Esperado: PASS.
- [x] **Step 5: Commit** — `git commit -m "feat(groups): deliver group notices and read orchestration facts from Rust"`

### Task 4: Rotas de grupo de uma máquina no Rust (paridade)

Status: ready-for-agent

**Files:**
- Create: `crates/hangar-server/src/groups/routes.rs`
- Modify: `crates/hangar-server/src/routes.rs:346` (`pass_any` passa a `pub(crate)`)
- Modify: `crates/hangar-server/src/config.rs:55` (ler `HANGAR_SERVER_ID` e `HANGAR_PAIR_ARCHIVE`;
  o Python passa a mandá-las na Task 6)
- Modify: `backend/tests/fixtures/contract/gen_golden.py` (respostas das rotas de grupo)
- Test: `crates/hangar-server/tests/groups_routes.rs`

**Interfaces:**
- Consumes: Tasks 2 e 3, `ListBridge` (retrato e descoberta forçada).
- Produces: handlers de `POST|DELETE /api/sessions/{name}/pair`,
  `POST /api/sessions/{name}/group-message`, `GET|PUT /api/sessions/{name}/pair/contract`, montados
  por `groups::routes::router() -> Router<Arc<AppState>>`. **Não registrados no roteador principal
  nesta Task** (a Task 6 registra junto com a troca do Python, para nunca haver dois escritores).

- [x] **Step 1: Golden pelo Python** — `gen_golden.py` roda as rotas Python com sessões falsas e
  grava status + corpo de: par de duas soltas; tarefa diferente (409); `erro_autopareamento` (400);
  `erro_peer_nao_informado` (400); sessão inexistente (404); orquestrador (409 `erro_sessao_orq`);
  saída (`{ok, warning: null}`); saída de quem não está em grupo; `group-message` (ok; barra; `[grupo:`
  reencaminhado; sem grupo 404 `erro_sessao_sem_grupo`; 6º em 60 s = 429); contrato sem grupo (404
  `erro_sessao_nao_pareada`) e com grupo; corpo com campo a mais (422 do FastAPI).

- [x] **Step 2: Testes das rotas** (padrão de `tests/list_routes.rs`: `fake` + `list_support::server`,
  com o `groups::routes::router()` montado no servidor de teste), um por golden do Step 1, comparando
  por `common::canon`, mais:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn pair_delivers_protocol_only_to_newcomers() {
    // a e b soltas: o Python falso recebe 2 /input com o texto que ele devolveu em /internal/pair/text;
    // c entra depois: 1 /input, só para c.
}
#[tokio::test(flavor = "multi_thread")]
async fn pair_is_undone_when_no_notice_arrives() {
    // Python falso responde 400 em todo /input: 502 erro_pareamento_desfeito e nenhum sidecar.
}
#[tokio::test(flavor = "multi_thread")]
async fn notice_waiting_on_closed_ingress_does_not_block_rename() {
    // porta de entrada de b fechada; POST /pair de a com b fica esperando a entrega;
    // em paralelo, GroupService::rename("c","c2") termina sem esperar o /pair.
}
#[tokio::test(flavor = "multi_thread")]
async fn contract_put_checks_mtime_and_creates_exclusively() {
    // PUT com mtime 0 cria; segundo PUT com mtime 0 → 409 erro_contrato_mudou; PUT com o mtime devolvido grava.
}
```

- [x] **Step 3: Rodar e ver falhar.**

- [x] **Step 4: Implementar `routes.rs`.**
  - Dono: `auth.is_owner`; não dono → `pass_any` (Python).
  - Sessões e provedores: retrato da `ListBridge` (até 2 s); nome ausente → uma descoberta forçada
    antes do 404 (como `_cached_info_sync`, `api.py:1096-1107`).
  - Orquestrador: `groups::orq::is_orchestrator` → 409 `erro_sessao_orq`, nas mesmas rotas em que o
    Python chama `_recusa_orq`.
  - Transferência, rota a rota como no Python: `/pair` e `DELETE /pair` com a recusa do
    `_transfer_guard`; `/group-message` com a do `_transfer_check` (`api.py:537`); `/pair/contract`
    sem conferência. No Rust a pergunta é à porta de entrada da sessão (`IngressGates`, fechada com
    `held`), devolvendo o mesmo código.
  - Protocolo: `protocol_text` + `deliver_text` **depois** de soltar o lock do serviço; todos os
    avisos falharam → `GroupService::restore(before)` e 502 `erro_pareamento_desfeito`; parcial →
    `warning` `erro_pareamento_aviso_parcial` (corpo como `api.py:5157-5169`).
  - Anti-tempestade: 5 por gid em 60 s, em memória (`api.py:5274-5283`).
  - Contrato: `GET` = `{peers, path, content}` (mais `owner` e `mtime`, só acréscimo); `PUT {content,
    mtime}` grava com `OpenOptions::create_new` quando o arquivo não existe e confere o `mtime`
    (segundos `f64`, tolerância `1e-6`, como `orq_md`) senão; conflito → 409 `erro_contrato_mudou`.
    Este código entra já na 1a com frase nas três superfícies.

- [x] **Step 5: Rodar** os testes da Task. Esperado: PASS.

- [x] **Step 6: Commit** — `git commit -m "feat(groups): single-machine pairing routes in Rust"`

### Task 5: Par 1:1 entre máquinas no Rust (paridade)

Status: ready-for-agent

**Files:**
- Create: `crates/hangar-server/src/groups/peers.rs`, `crates/hangar-server/src/groups/legacy.rs`
- Modify: `crates/hangar-server/src/groups/routes.rs` (`/pair` com um peer `srv::x`;
  `POST /api/sessions/{name}/pair-remote` e `/unpair-remote`; `DELETE /pair` avisando remotos e
  desfazendo par externo)
- Modify: `backend/tests/fixtures/contract/gen_golden.py` (respostas do 1:1)
- Test: `crates/hangar-server/tests/groups_peers.rs`, `crates/hangar-server/tests/groups_legacy.rs`

**Interfaces:**
- Produces:

```rust
pub struct PeerCfg { pub base_url: String, pub token: String }
pub struct PeerBook { /* caminho de HANGAR_PEERS_FILE, cache por (mtime, len) */ }
impl PeerBook { pub fn get(&self, server: &str) -> Option<PeerCfg>; } // `_eh_peer` + base_url + token
pub enum PeerError { Unknown, Refused { status: u16, detail: serde_json::Value }, Transport(String) }
pub struct PeerClient { /* reqwest::Client */ }
impl PeerClient {
    /// `peers.call` (peers.py:401-411): prazo de 8 s por leitura e 16 s no total, corpo até 1 MiB,
    /// segue redirect como o Python.
    pub async fn call(&self, server: &str, method: reqwest::Method, path: &str, body: Option<&serde_json::Value>) -> Result<Option<serde_json::Value>, PeerError>;
}
```

- [x] **Step 1: Golden pelo Python**: `_pair_cross_server` feliz, peer recusou (502
  `erro_pareamento_rejeitado`), rede caiu (502 `erro_pareamento_nao_confirmado`, com a chamada de
  limpeza ao `/unpair-remote`), `CP_SERVER_ID` ausente (400), mais de um remoto (400
  `erro_pareamento_cross_1_1`); `/pair-remote` feliz e com aviso falho (502); `/unpair-remote`
  de quem não é par (`noop`); saída avisando remoto que não responde (`warning`
  `erro_pareamento_saida_falhou` com `erro_peer_nao_avisado`).

- [x] **Step 2: Testes**: os goldens acima com duas instâncias de teste (`casa` e `lab`, cada uma com
  `tempdir`, `HANGAR_SERVER_ID` e `peers.json` apontando para a outra); `PeerBook` lê só entradas
  com `base_url` e `token` (entrada só com `app` não é peer; `enabled: false` continua endereçável);
  `PeerClient`: corpo de 2 MiB → `Transport`; 409 com código → `Refused`; o redirect segue sem levar o
  `Authorization` para outro host (conferir o comportamento do `reqwest` 0.13.4; se ele levar,
  recusar redirect para outro host); `delete_pair_ends_external_pair` (sessão com par externo: o
  Python falso recebe `/internal/external-pairs/end`).

- [x] **Step 3: Rodar e ver falhar.**

- [x] **Step 4: Implementar** o iniciador (`api.py:5162-5211`), o receptor (`5219-5243`), o
  `/unpair-remote` com a defesa de "é mesmo par" (`5250-5268`), e a saída (`DELETE /pair` e
  `GroupService::leave` chamada pela ponte) avisando cada remoto de máquina própria pelo
  `/unpair-remote` e chamando `/internal/external-pairs/end` quando um ex-companheiro é par externo
  (o Python desfaz o lado de fora como `registry._encerrar_pares_externos`). Rede sempre fora do lock.

- [x] **Step 5: Rodar.** Esperado: PASS.

- [x] **Step 6: Commit** — `git commit -m "feat(groups): 1:1 cross-machine pairing in Rust with Python parity"`

### Task 6: O Python passa a pedir ao Rust; o Rust registra as rotas

Status: ready-for-agent

**Files:**
- Create: `backend/app/groups_bridge.py`, `crates/hangar-server/src/groups/bridge.rs`
- Modify: `crates/hangar-server/src/routes.rs:75-101` (o `GroupService` nasce no `AppState`),
  `routes.rs:211,227-238` (ponte `POST /__hangar_server/groups` com `private_ok`),
  `routes.rs:240-283` (registrar `groups::routes::router()`), `routes.rs:244-249` (404 da ponte na
  porta pública, como as outras), `routes.rs:309-321` (saúde: `"groups": true`),
  `crates/hangar-server/src/migration_status.rs:63-85` (área `pairing` passa a Rust),
  `crates/hangar-server/src/lib.rs:33`, `crates/hangar-server/tests/proxy.rs:33`
- Modify: `backend/app/rust_server.py:35,322-340,391-414` (protocolo, variáveis do filho,
  endereço da ponte, saúde com `groups`)
- Modify: `backend/app/pair.py` (primitivas de escrita recusam no modo `rust`/`pending`)
- Modify: `backend/app/api.py` (rotas de grupo: no modo `rust`/`pending` repassam à ponte;
  `_pair_sweep_loop` só no modo `python`; kill sem `_avisar_saida` no modo `rust`),
  `backend/app/registry.py:2433,2495,2547,2972,3009,3078,3096,3118,3121-3130`,
  `backend/app/external_pair_api.py:108,115,123,159,203,246,252,268`,
  `backend/app/orq_context.py:174-190`
- Test: `backend/tests/test_groups_bridge.py`, `crates/hangar-server/tests/groups_bridge.rs`

**Interfaces:**
- Consumes: Tasks 2–5.
- Produces (ponte, envelope `{"op","args"}` → `{"ok":true,"result"}` |
  `{"ok":false,"error":{code,detail}}`, como `list/bridge.rs:788-810`):

| op | args | result |
|---|---|---|
| `group.route` | `{method, name, route: "pair"\|"contract"\|"group-message"\|"pair-remote"\|"unpair-remote", body}` | `{status, body}` da rota do Rust |
| `group.leave` | `{name}` | `{ex_peers}` (o Rust já avisou remotos e pediu o fim do par externo) |
| `group.rename` | `{old, new}` | `{}` |
| `group.external_link` | `{local, address, harness}` | `{gid}` |
| `group.external_unlink` | `{local, address}` | `{}` (é o `restore` do par externo: o retrato antes do link é sempre "solto") |
| `group.orq_associate` | `{name, gid, mtime}` | contexto, ou o erro da `/internal/orq/associate` |

  `groups_bridge.rust_owns_groups() -> bool` (modo `rust`/`pending` e saúde com `groups`);
  `groups_bridge.call(op, **args) -> dict` (no `pending` espera até `PENDING_WAIT_S`, depois
  `runtime_starting`, como as demais operações); `pair.GroupsOwnedByRust(RuntimeError)`.

- [x] **Step 1: Testes Python**

```python
# backend/tests/test_groups_bridge.py
def test_write_primitives_refuse_in_rust_mode(monkeypatch, tmp_path):
    monkeypatch.setattr(pair.settings, "projects_dir", str(tmp_path / "projects"))
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)
    with pytest.raises(pair.GroupsOwnedByRust):
        pair.PairLink("a").set(["b"], "", "g1", {})
    with pytest.raises(pair.GroupsOwnedByRust):
        pair.PairLink("a").clear()

def test_kill_in_rust_mode_asks_rust(monkeypatch):
    chamadas = []
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)
    monkeypatch.setattr(groups_bridge, "call", lambda op, **a: chamadas.append((op, a)) or {"ex_peers": ["b"]})
    registry.SessionRegistry()._clear_pair("a")
    assert chamadas == [("group.leave", {"name": "a"})]

def test_create_refuses_when_group_cleanup_fails(monkeypatch): ...
    # ponte responde erro e "a.json" existe: create("a") → ValueError com erro_grupo_limpeza_falhou

def test_pair_route_forwards_status_and_body(monkeypatch, client):
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)
    monkeypatch.setattr(groups_bridge, "call", lambda op, **a: {"status": 409, "body": {"detail": {"code": "erro_pareamento_tarefa_existente"}}})
    r = client.post("/api/sessions/a/pair", json={"peer": "b", "task": "y"})
    assert r.status_code == 409 and r.json()["detail"]["code"] == "erro_pareamento_tarefa_existente"

def test_sweep_loop_does_not_run_in_rust_mode(monkeypatch): ...
def test_associate_in_rust_mode_goes_through_bridge(monkeypatch): ...
```

No Rust (`groups_bridge.rs`): ponte sem segredo → 404 (como as outras pontes privadas,
`workspace_routes.rs:51`); `group.leave` de membro de par devolve `ex_peers`; `group.route` devolve
o mesmo `{status, body}` da rota pública; `group.orq_associate` chama `/internal/orq/associate`
segurando o lock (um `group.leave` concorrente espera).

- [x] **Step 2: Rodar e ver falhar.**

- [x] **Step 3: Implementar.** No modo `rust`/`pending`: `registry._clear_pair` → `group.leave`;
  na criação, falha da ponte com sidecar presente recusa a criação (`erro_grupo_limpeza_falhou`); no
  kill, falha vai ao log e a varredura do Rust resolve; `registry.rename` → `group.rename` antes de
  mexer no tmux/sidecar sem terminal; `api.kill_session` → sem `_avisar_saida` (o Rust avisa);
  `external_pair_api` → `group.external_link`/`external_unlink`/`leave`; rotas Python de grupo,
  `/pair-remote` e `/unpair-remote` → `group.route` (atende Connect, 8766 do dono e o MCP, que chama
  as funções em processo); `_pair_sweep_loop` não sobe; `orq_context.associate` → `group.orq_associate`.
  O corpo atual do `associate`, sem `pair._LOCK`, vira a função que a `/internal/orq/associate`
  chama. Subir o protocolo para o próximo número livre nos três lugares.

- [x] **Step 4: Rodar** os testes da Task e `--test proxy`. Esperado: PASS.

- [x] **Step 5: Commit** — `git commit -m "feat(groups): make Rust the only writer of group files"`

### Task 7: Varredura de membro morto e de grupo `orq` sozinho no Rust

Status: ready-for-agent

**Files:**
- Create: `crates/hangar-server/src/groups/sweep.rs`
- Modify: `crates/hangar-server/src/routes.rs:75-101` (subir a tarefa junto com o `ListBridge`)
- Test: `crates/hangar-server/tests/groups_sweep.rs`

**Interfaces:**
- Consumes: `GroupService::leave`, `ListBridge` (retrato), `OrqFacts::phase`.
- Produces: `groups::sweep::spawn(service, bridge) -> JoinHandle<()>`.

- [x] **Step 1: Testes**

```rust
#[tokio::test(start_paused = true)] async fn dead_member_leaves_after_five_seconds_of_absence() {} // 4 s fica; 5 s sai
#[tokio::test(start_paused = true)] async fn sweep_skips_failed_or_empty_list() {}                 // erro ou vazia: nada sai
#[tokio::test(start_paused = true)] async fn sanitized_name_counts_as_alive() {}                   // "nome com espaço" vivo fica
#[tokio::test(start_paused = true)] async fn lone_orq_group_dissolves_only_when_run_ended_or_grace_passed() {}
#[tokio::test(start_paused = true)] async fn no_sidecar_no_list_request() {}                       // sem sidecar, nada é pedido
```

- [x] **Step 2: Rodar e ver falhar.**
- [x] **Step 3: Implementar** `registry._varrer_pares_mortos` (`registry.py:3138-3174`) e
  `pair.dissolve_lone_orq` (`pair.py:330-350`) a cada 2 s: primeiro um `read_dir` barato; só com
  sidecar, o retrato da lista (até 2 s); lista com erro ou vazia não varre (diário uma vez por
  sequência: `grupos.varredura_falhou`/`grupos.varredura_voltou`); `orq` sozinho sai com fase
  `Ended`, ou `NotStarted` com o sidecar sem escrita há mais de 3600 s. A saída é o mesmo
  `GroupService::leave` (que já avisa remotos e pede o fim do par externo).
- [x] **Step 4: Rodar.** Esperado: PASS.
- [x] **Step 5: Commit** — `git commit -m "feat(groups): sweep dead group members in Rust"`

### Task 8: A lista reemite quando só o grupo muda

Status: ready-for-agent

**Files:**
- Modify: `crates/hangar-server/src/list/sig.rs:68-88`, `backend/app/sse.py:345-399`,
  `backend/app/list_facts.py:21-27` (`SIG_FIELDS`): mesma tupla nos três, com
  `pair_peers, pair_gid, pair_task, pair_external` no fim
- Modify: `backend/tests/fixtures/contract/gen_list.py:843` (regenerar a `sig` dos goldens da lista)
- Modify: `crates/hangar-server/src/groups/service.rs` (depois de cada escrita, o mesmo caminho do
  `list.invalidate`, `list/bridge.rs:285`)
- Test: `crates/hangar-server/tests/list_routes.rs` (caso novo), `backend/tests/test_sse.py`,
  `backend/tests/test_list_facts.py:179`, `crates/hangar-server/tests/contract_list/markers.rs:121-132`

- [x] **Step 1: Teste** — Rust `pair_change_alone_reemits_the_list`: lista aberta, só o sidecar de
  `b` muda → chega um `sessions` novo em até 2 tiques. Python: duas listas que diferem só em
  `pair_peers` têm `_list_sig` diferente.
- [x] **Step 2: Rodar e ver falhar.**
- [x] **Step 3: Implementar** (tupla idêntica; `test_list_facts.py:179` e `markers.rs` provam a
  paridade).
- [x] **Step 4: Rodar.** Esperado: PASS.
- [x] **Step 5: Commit** — `git commit -m "fix(list): re-emit the session list when only the group changes"`

### Task 9: Verificação manual com o dono (1a)

Status: ready-for-human

- [ ] **Step 1: verificação manual — grupo local**: pelo canal de testes, `hangar-send --pair`,
  `--group`, `--unpair`, renomear e matar membro; badges nas telas mudando na hora.
- [ ] **Step 2: verificação manual — par 1:1 entre máquinas** entre esta máquina e outra já na 1a, e
  com uma máquina ainda na `main` (o receptor antigo e o `/unpair-remote` antigo respondem).
- [ ] **Step 3: verificação manual — orquestração**: uma execução `orquestrar-auto` curta; o grupo
  `orq` nasce, o árbitro fica sozinho entre Tasks, dissolve no fim.
- [ ] **Step 4: verificação manual — reserva Python**: `CP_RUST_SERVER=0`; grupo local funciona como
  hoje.
- [ ] **Step 5: verificação manual — Rust caindo com grupo vivo**: matar o `hangar-server` no meio de
  uma troca de conta de uma sessão em grupo; quando ele volta, a sessão continua no grupo e a
  varredura não a tira.

---

## Entrega 1b — federação

### Task 10: Identidade de máquina, registro e fila de saída

Status: ready-for-agent

**Files:**
- Modify: `crates/hangar-server/src/groups/store.rs` (registro e fila),
  `crates/hangar-server/src/groups/peers.rs` (`OldVersion`; conferência de identidade)
- Create: `crates/hangar-server/src/groups/outbox.rs`
- Modify: `crates/hangar-api/src/session.rs:108-114` (`pair_owner: Option<String>`),
  `crates/hangar-server/src/list/links.rs:175-187` (`pair_owner` = `fed.owner`),
  `backend/app/models.py` (`pair_owner: str | None = None`), `backend/app/pair.py:75-94`
  (`get()` devolve `fed`), `backend/app/registry.py:250-252,1486-1497,1561-1564,1586-1589`
  (`pair_owner`), `backend/app/share_guest_api.py:35` (`_OTHER_SESSIONS` tira `pair_owner`),
  `crates/hangar-server/src/list/sig.rs`, `backend/app/sse.py`, `backend/app/list_facts.py`
  (`pair_owner` na tupla), goldens da lista
- Test: `crates/hangar-server/tests/groups_outbox.rs`, `crates/hangar-server/tests/groups_peers.rs`

**Interfaces:**
- Produces:

```rust
impl PairDir {
    pub fn record(&self, gid: &str) -> Result<Option<GroupRecord>, StoreError>;   // groups/<gid>.json
    pub fn write_record(&self, r: &GroupRecord) -> Result<(), StoreError>;
    pub fn delete_record(&self, gid: &str) -> Result<(), StoreError>;
    pub fn records(&self) -> Result<Vec<GroupRecord>, StoreError>;
}
pub enum PeerError { Unknown, Refused { status: u16, detail: Value }, Transport(String), OldVersion,
    IdentityMismatch { key: String, reported: String } }

#[derive(Serialize, Deserialize)] pub struct OutboxItem { pub id: String, pub server: String, pub gid: String,
    pub kind: OutboxKind, pub attempts: u32, pub next_at: u64 }
#[derive(Serialize, Deserialize)] #[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutboxKind {
    Push { record: GroupRecord, joining: Vec<String> },
    Dissolved { merged_into: Option<String> },
    Leave { member: Member },
    Rename { member: Member, new_name: String },
}
pub struct Outbox { /* .hangar-pair/groups/outbox.json */ }
impl Outbox {
    pub fn push_record(&mut self, server: &str, record: &GroupRecord, joining: &[String]); // coalesce Push sem joining
    pub fn dissolved(&mut self, server: &str, gid: &str, merged_into: Option<&str>);      // tira Push pendente do gid
    pub fn op(&mut self, server: &str, gid: &str, kind: OutboxKind);                       // ordem preservada
    pub fn next_for(&self, server: &str) -> Option<&OutboxItem>;                           // um por destino, em ordem
    pub fn done(&mut self, id: &str);
    pub fn failed(&mut self, id: &str, now: u64);       // só rede: 2,5,30,60,120,240,300,600,1800 s
    pub fn pending_renames(&self, gid: &str) -> Vec<(Member, String)>;
    pub fn next_wake(&self) -> Option<u64>;
}
/// Um trabalhador por máquina de destino; o chamador que precisa do resultado (entrada) espera o
/// item dele com prazo.
pub struct OutboxRunner;
impl OutboxRunner { pub fn spawn(..) -> Self; pub fn kick(&self, server: &str);
    pub async fn wait(&self, id: &str, deadline: Duration) -> Result<Option<Value>, PeerError>; }
```

`OldVersion` = 404 cujo `detail` é a string `"Not Found"`, 405, ou 422 numa rota `/api/groups/*`.
Recusa definitiva (`Refused`, `OldVersion`, `IdentityMismatch`) marca o item como feito e vai ao
diário (`grupos.recusado`); só `Transport` volta à fila. Toda resposta das rotas de grupo traz
`server_id`; o `PeerClient` compara com a chave usada e devolve `IdentityMismatch`.

- [ ] **Step 1: Testes**:

```rust
#[test] fn push_records_coalesce_but_joining_push_is_kept() {}
#[test] fn dissolved_drops_pending_push_of_same_gid() {}
#[test] fn definitive_refusal_leaves_queue_and_unblocks_next_item() {}
#[test] fn transport_failure_walks_the_backoff_and_caps_at_1800() {}
#[test] fn queue_survives_reopen() {}
#[tokio::test(flavor = "multi_thread")] async fn key_mismatch_is_refused_before_any_write() {}
#[tokio::test(flavor = "multi_thread")] async fn one_worker_per_destination_keeps_order() {}
#[tokio::test(flavor = "multi_thread")] async fn old_route_missing_is_old_version() {}
```

- [ ] **Step 2: Rodar e ver falhar.**
- [ ] **Step 3: Implementar.** O trabalhador de cada destino só existe com item; acorda no
  `next_at` ou por `kick(server)` depois de qualquer resposta boa daquela máquina. Diário por
  transição (`grupos.pendente`/`grupos.entregue`), uma vez por máquina e código. `pair_owner` na
  lista e na assinatura (Python e Rust), goldens regenerados; convidado não recebe `pair_owner`.
- [ ] **Step 4: Rodar.** Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(groups): group records, server identity check and durable outbox"`

### Task 11: Grupo entre máquinas: dona, participante e roteamento

Status: ready-for-agent

**Files:**
- Create: `crates/hangar-server/src/groups/fed.rs` (puro), `crates/hangar-server/src/groups/fed_routes.rs`
- Modify: `crates/hangar-server/src/groups/service.rs` (candidato com `fed` ou `srv::` novo vai à
  federação), `crates/hangar-server/src/groups/routes.rs` (`/pair` com N remotos;
  `GET /api/sessions/{name}/group`), `crates/hangar-server/src/routes.rs` (rotas `/api/groups/*`),
  `backend/app/share_gate.py:37` (`group` em `_BLOCKED`), `backend/app/api.py` (rota Python
  `GET /api/sessions/{name}/group` que repassa à ponte, para o Connect)
- Test: `crates/hangar-server/tests/groups_fed_unit.rs`, `crates/hangar-server/tests/groups_fed.rs`,
  `backend/tests/test_share_gate.py` (caso `group`)

**Interfaces:**
- Consumes: Tasks 2, 4, 6, 10.
- Produces: variantes de `GroupError`: `Peer(PeerError)`, `OwnerUnavailable { owner }`,
  `UnknownOwner { owner }`, `FusionAcrossOwners`, `SameServerId`, `PeerNotRegistered { server }`,
  `OrqAcrossMachines`, `GroupChanged`, cada uma com o código de mesmo sentido das Global Constraints.

```rust
pub enum FedOutcome { Kept(GroupRecord), Dissolved { gid: String, last: Vec<Member> } }
pub fn add_members(rec: &GroupRecord, joiners: &[Member]) -> GroupRecord;           // version + 1
pub fn remove_member(rec: &GroupRecord, m: &Member) -> FedOutcome;                  // ≤ 1 no total: Dissolved
pub fn rename_member(rec: &GroupRecord, m: &Member, new_name: &str) -> GroupRecord;
pub fn should_apply(local_version: Option<u64>, incoming: &GroupRecord) -> Apply;   // Newer | Same | Stale
/// O que a participante faz com um registro recebido: só cria sidecar para quem está em
/// `joining_ok`; só altera/apaga sidecar existente da mesma (dona, gid); renomeação pendente por cima.
pub struct LocalApply { pub writes: Vec<(String, Sidecar)>, pub clears: Vec<String>,
    pub missing: Vec<Member> /* no registro, sem sidecar, fora do joining → pedir saída */,
    pub dropped: Vec<String> /* tinha sidecar, saiu do registro sem pedir → avisar */ }
pub fn local_apply(rec: &GroupRecord, me: &str, existing: &BTreeMap<String, Sidecar>,
    joining_ok: &[String], pending_renames: &[(Member, String)]) -> LocalApply;
```

Rotas entre máquinas (corpo estrito, `protocol: 1`, só dono, resposta sempre com `server_id`):
- `POST /api/groups/{gid}/ops` na dona: `{protocol, op: "join", members: [Member], expected:
  {"srv::nome": gid|null}, task, replace_task}` → `{record, refused: [{member, code}]}` (409
  `erro_grupo_mudou` se algum `expected` não bate); `{op: "leave", member}`;
  `{op: "rename", member, new_name}`.
- `PUT /api/groups/{gid}` na participante: `{protocol, record, joining: [name], dissolved: bool,
  merged_into: string|null}` → `{accepted: [name], refused: [{name, code}]}`.
- `GET /api/groups/{gid}` na dona → registro ou 404 `erro_grupo_inexistente`.
- `GET /api/sessions/{name}/group` (pública) → `{server_id, gid, owner, version, task, orq,
  members, sync}` ou 404 `erro_sessao_sem_grupo`; `sync` = `"pending"` com item na fila.

- [ ] **Step 1: Testes puros** (`groups_fed_unit.rs`):

```rust
#[test] fn remove_down_to_one_dissolves_counting_all_servers() {}
#[test] fn push_never_creates_sidecar_outside_joining() {
    // registro v8 com casa::a e lab::x; lab sem sidecar de x e joining vazio:
    // local_apply não grava x e devolve x em `missing` (vira pedido de saída).
}
#[test] fn late_push_does_not_resurrect_pending_leave() {
    // lab apagou o sidecar de c e enfileirou Leave; registro v5 ainda com lab::c e joining vazio:
    // nada é gravado para c.
}
#[test] fn member_dropped_by_owner_is_told_it_left() {}  // sidecar existente, fora do registro → `dropped`
#[test] fn same_name_on_two_servers_are_two_members() {
    // casa::a e lab::a: em casa, peers do a = ["lab::a"]; em lab, peers do a = ["casa::a"].
}
#[test] fn pending_rename_maps_name_in_pushed_record() {}
#[test] fn same_version_is_same_not_newer() {}
```

- [ ] **Step 2: Testes de duas e três máquinas** (`groups_fed.rs`): instâncias de teste com `tempdir`,
  `HANGAR_SERVER_ID` (`casa`, `lab`, `vps`) e `peers.json` cruzados; Python falso em cada uma.

```rust
#[tokio::test(flavor = "multi_thread")] async fn two_plus_two_group_has_one_gid_everywhere() {}
#[tokio::test(flavor = "multi_thread")] async fn three_machines_n_members_one_group() {}
#[tokio::test(flavor = "multi_thread")] async fn join_partial_when_one_server_is_down() {}
#[tokio::test(flavor = "multi_thread")] async fn leave_while_owner_down_is_local_and_queued() {}
#[tokio::test(flavor = "multi_thread")] async fn owner_restart_between_commit_and_push_still_delivers() {}
#[tokio::test(flavor = "multi_thread")] async fn stale_push_after_dissolve_is_ignored() {}
#[tokio::test(flavor = "multi_thread")] async fn repeated_push_does_not_redeliver_protocol() {}
#[tokio::test(flavor = "multi_thread")] async fn join_into_group_owned_elsewhere_is_forwarded_to_owner() {}
#[tokio::test(flavor = "multi_thread")] async fn group_changed_since_discovery_is_409() {}
#[tokio::test(flavor = "multi_thread")] async fn fusion_same_owner_appends_contract() {}
#[tokio::test(flavor = "multi_thread")] async fn fusion_push_order_does_not_matter() {}
#[tokio::test(flavor = "multi_thread")] async fn fusion_of_different_owners_is_refused() {}   // D2 = A
#[tokio::test(flavor = "multi_thread")] async fn participant_refuses_unknown_or_grouped_session() {}
#[tokio::test(flavor = "multi_thread")] async fn same_server_id_and_unregistered_peer_are_refused() {}
#[tokio::test(flavor = "multi_thread")] async fn orq_group_never_takes_remote_member() {}
#[tokio::test(flavor = "multi_thread")] async fn old_peer_gets_old_version_message() {}
#[tokio::test(flavor = "multi_thread")] async fn startup_pull_converges_participant() {}
#[tokio::test(flavor = "multi_thread")] async fn dissolved_on_owner_clears_participant_on_pull() {}
#[tokio::test(flavor = "multi_thread")] async fn guest_cannot_read_group_view() {}
```

Cada teste confere o que ficou em cada `tempdir` e o que cada Python falso recebeu em `/input`.

- [ ] **Step 3: Rodar e ver falhar.**

- [ ] **Step 4: Implementar `fed.rs`** (funções puras acima).

- [ ] **Step 5: Roteamento no `/pair`** (fora do lock): grupo de cada candidato (local: sidecar;
  remoto: `GET {srv}/api/sessions/{nome}/group`, 404 com código = solto). Nenhum grupo → esta
  máquina cria como dona. Um grupo → `op: join` na dona com `expected`. Grupos da mesma dona → fusão
  na dona. Donas diferentes → 409 `erro_grupo_fusao_entre_donos` (D2, decidido). Recusas antes de qualquer escrita:
  `HANGAR_SERVER_ID` vazio, id remoto igual ao meu, máquina fora do `peers.json`, chave ≠ id, grupo
  `orq`, par externo. O par antigo 1:1 (sidecar com `srv::` e sem `fed`) não entra em grupo novo:
  `erro_pareamento_mistura_cross` como hoje.

- [ ] **Step 6: Dona.** Sob o lock: registro + versão nova + sidecars locais + itens da fila para
  cada participante (fila antes da rede). Fora do lock: espera o resultado do item de cada
  participante (`OutboxRunner::wait`, prazo de 16 s); recusado ou sem resposta → `remove_member`
  (versão nova, de novo com a fila). Resposta: quem entrou e, em `warning`,
  `erro_grupo_membro_nao_confirmado` com quem ficou de fora; nenhum confirmado → 502 e o grupo volta
  ao que era. Na subida, um `Push` sem `joining` de cada registro para cada participante.

- [ ] **Step 7: Participante.** `PUT /api/groups/{gid}`: `should_apply` (`Same` responde o mesmo
  resultado sem reentregar); confere cada `joining` na própria lista (descoberta forçada se faltar),
  sem outro grupo, não `orq`, não par externo; `local_apply` sob o lock; fora dele, entrega o
  protocolo aos aceitos, avisa os `dropped` com o aviso de saída de hoje e enfileira saída dos
  `missing`. `dissolved` apaga só sidecars daquele `gid`; com `merged_into`, sem aviso. Saída,
  renomeação e morte de membro local de grupo de dona alheia: aplica no sidecar na hora e enfileira
  `Leave`/`Rename`. Na subida, `GET` na dona de cada grupo em que tem membro.

- [ ] **Step 8: Rodar.** Esperado: PASS.

- [ ] **Step 9: Commit** — `git commit -m "feat(groups): N-session groups across machines with a single owner"`

### Task 12: Aviso de grupo entre máquinas e contrato pela dona

Status: ready-for-agent

**Files:**
- Modify: `crates/hangar-server/src/groups/fed_routes.rs` (`POST /api/groups/{gid}/deliver`,
  `GET|PUT /api/groups/{gid}/contract`, `op: "message"`), `crates/hangar-server/src/groups/routes.rs`
  (`/group-message` e `/pair/contract` repassam à dona)
- Test: `crates/hangar-server/tests/groups_fed.rs` (casos novos)

**Interfaces:**
- Produces: `op: "message" {sender: Member, text}` → `{peers: [qualificado], pulados: [qualificado], warning}`;
  `POST /api/groups/{gid}/deliver {protocol, sender: "srv::nome", text, members: [nome]}` →
  `{server_id, results: {nome: {ok, error}}}`; `GET /api/groups/{gid}/contract` →
  `{server_id, content, mtime}`; `PUT /api/groups/{gid}/contract {protocol, content, mtime}` →
  `{server_id, mtime}` ou 409.

- [ ] **Step 1: Testes**:

```rust
#[tokio::test(flavor = "multi_thread")] async fn group_message_reaches_all_machines_with_qualified_sender() {}
// [grupo: casa::a] em lab; [grupo: a] em casa.
#[tokio::test(flavor = "multi_thread")] async fn storm_limit_is_counted_at_the_owner() {} // 3 de casa + 3 de lab: o 6º é 429
#[tokio::test(flavor = "multi_thread")] async fn unreachable_participant_is_listed_in_pulados() {}
#[tokio::test(flavor = "multi_thread")] async fn owner_down_group_message_is_503_and_nothing_sent() {}
#[tokio::test(flavor = "multi_thread")] async fn contract_from_participant_reads_and_writes_owner_file() {}
#[tokio::test(flavor = "multi_thread")] async fn concurrent_create_keeps_first_text() {} // dois PUT mtime 0: um 200, outro 409
```

- [ ] **Step 2: Rodar e ver falhar.**
- [ ] **Step 3: Implementar.** As recusas de hoje (barra, `[grupo:`/`[de:` reencaminhado, sem grupo)
  ficam na máquina do remetente, antes da rede. A dona entrega aos locais com `[grupo: <nome>]` e
  manda a cada participante o texto com `sender` qualificado (`[grupo: <srv>::<nome>]` lá). Aviso
  e contrato são chamadas diretas com prazo (não passam pela fila: não há o que repetir depois).
  `GET …/pair/contract` numa participante devolve `path: ""`, `owner` e o conteúdo da dona; dona
  fora → 503 `erro_grupo_dono_indisponivel`.
- [ ] **Step 4: Rodar.** Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(groups): cross-machine group notices and owner-hosted contract"`

### Task 13: Reserva Python com grupo entre máquinas

Status: ready-for-agent

**Files:**
- Modify: `backend/app/pair.py` (`FederatedGroupError`; saída de membro `fed` apaga o sidecar e
  acrescenta `Leave` em `groups/outbox.json`), `backend/app/api.py` (rename recusa membro `fed`
  antes do tmux, `api.py:~3451`; rotas traduzem `FederatedGroupError` para 409
  `erro_grupo_entre_maquinas_sem_rust`), `backend/app/registry.py:2954-2972` (mesma recusa no ramo
  sem terminal, antes de `headless_sessions.rename`)
- Test: `backend/tests/test_pair.py`, `crates/hangar-server/tests/groups_outbox.rs`
  (a fila aceita o item escrito pelo Python)

- [ ] **Step 1: Testes**

```python
def test_python_mode_reused_name_leaves_and_queues(tmp_path, monkeypatch):
    monkeypatch.setattr(pair, "_arquivo_dir", lambda: tmp_path / "arquivo")
    d = pair._pair_dir()
    (d / "a.json").write_text(json.dumps({"peers": ["lab::c"], "task": "", "gid": "ab12cd34", "harness": {},
                                          "fed": {"owner": "lab", "local": False, "version": 3}}))
    pair.leave("a")
    assert not (d / "a.json").exists()
    fila = json.loads((d / "groups" / "outbox.json").read_text())
    assert fila["items"][-1]["kind"] == "leave" and fila["items"][-1]["server"] == "lab"

def test_python_mode_refuses_join_and_rename_of_federated_member(tmp_path): ...
def test_rename_of_federated_member_refused_before_tmux(monkeypatch, client): ...
```

- [ ] **Step 2: Rodar e ver falhar.**
- [ ] **Step 3: Implementar.** O item da fila segue o formato serializado de `OutboxItem` da Task 10
  (o teste Rust lê um arquivo escrito pelo Python). Só no modo `python`.
- [ ] **Step 4: Rodar.** Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(groups): keep cross-machine groups safe in the Python fallback"`

### Task 14: Texto aos agentes, `hangar-send`, MCP e documentação

Status: ready-for-agent

**Files:**
- Modify: `backend/app/pair_texto.py:40-60`, `backend/hooks/pair_hook.py`, `scripts/hangar-send`
  (verbo `--contrato`, ajuda), `backend/app/mcp_server.py:88-103` (`sessions`: o item da própria
  sessão ganha `pares` com os nomes qualificados), `scripts/install-hangar-send.sh:103-125` (texto do
  protocolo: `--group` alcança outras máquinas; pareamento de N entre máquinas),
  `scripts/omniroute-statusline.js:305-309` (conferir que lê `fed` sem quebrar),
  `messages/pt.json`, `messages/en.json`, `packages/core/src/errosApi.ts`
- Modify: `docs/decisoes/plataforma.md` (entrada nova "Grupos: o Rust grava, a dona decide"; ajustar
  "Grupo: o protocolo é do HOOK…" sobre a saída remota), `docs/migracao-rust/README.md` (partes e
  "Ainda no Python"), `CLAUDE.md` (regra curta com link)
- Test: `backend/tests/test_pair_texto.py`, `backend/tests/test_pair_hook.py`,
  `scripts/test-hangar-send-identidade.sh` (caso `--contrato`)

**Interfaces:**
- Consumes: `fed.local` (Task 1), `GET|PUT …/pair/contract` (Tasks 4, 12).
- Produces: `pair_texto.texto_grupo(me, others, task, contrato, harness, contrato_remoto=False)`;
  `hangar-send --contrato` (imprime `# mtime: <m>` e o conteúdo) e `hangar-send --contrato --gravar
  <mtime>` (lê o stdin; imprime o `mtime` novo; 409 vira frase e saída 1).

- [ ] **Step 1: Testes**

```python
def test_remote_contract_line_points_to_command():
    t = pair_texto.texto_grupo("c", ["casa::a"], "T-1", None, {"c": "claude"}, contrato_remoto=True)
    assert "hangar-send --contrato" in t and "casa::a" in t
    assert "todas as máquinas" in t

def test_local_contract_line_keeps_path():
    t = pair_texto.texto_grupo("a", ["b"], "", "/x/grupo-g.md", {"a": "claude"})
    assert "/x/grupo-g.md" in t and "--contrato" not in t

def test_hook_reads_fed_local_flag(tmp_path): ...   # fed.local false → linha do comando
```

- [ ] **Step 2: Rodar e ver falhar.**
- [ ] **Step 3: Implementar.** Linha do contrato sempre que há `gid`; `contrato_remoto` troca o
  caminho pelo comando; com membro `srv::` em `others`, uma linha diz que membro de outra máquina é
  endereçado como `servidor::sessao` e que o aviso de grupo alcança todas as máquinas. O hook decide
  por `fed.local` (ausente = local). `hangar-send` no Windows: conferir pelas regras de `windows.md`.
- [ ] **Step 4: Rodar** os testes da Task. Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(groups): tell agents where the contract lives and that group notices cross machines"`

### Task 15: Verificação manual com o dono (1b)

Status: ready-for-human

- [ ] **Step 1: verificação manual — grupo 2+1 entre duas máquinas**: `hangar-send --pair lab::c` de
  `a` com `b` já pareada; `--list` nas duas; `--group` chegando nas três; contrato lido e gravado de
  `c` por `--contrato`; saída de `c` avisando só `c`.
- [ ] **Step 2: verificação manual — três máquinas** (esta, outra e a VPS, se o dono quiser): grupo
  com membros nas três; desligar a dona e conferir que saídas ficam na fila e convergem ao religar.
- [ ] **Step 3: verificação manual — máquina fora do ar**: desligar o backend de `lab`, sair com `b`
  em casa e com `c` em lab; religar; nenhum sidecar sobrando (`ls ~/.claude/.hangar-pair` nas duas)
  e o diário com `grupos.pendente`/`grupos.entregue`.
- [ ] **Step 4: verificação manual — reserva Python**: `CP_RUST_SERVER=0` numa das máquinas com o
  grupo vivo; `--unpair` sai e a fila envia quando o Rust volta; `--pair` responde a frase do 409.
- [ ] **Step 5: verificação manual — VM Windows (DELPHI-02, clone separado)**: grupo entre a VM e
  esta máquina; renomear uma sessão na VM; conferir sidecars e o hook depois de `/clear`.

---

## Entrega 2 — desktop nativo

### Task 16: Identidade de máquina no nativo e `pair_owner`

Status: ready-for-agent

**Files:**
- Modify: `desktop-native/src/api/dto.rs:18-90` (`pair_owner: Option<String>`),
  `desktop-native/src/app/servers.rs:31,83-156` (`RemoteList.server_id: Option<String>`, lido do
  `Api::config()` existente, `desktop-native/src/api/mod.rs:443`, em `somente_leitura.server_id`;
  máquina de convite não pergunta), `desktop-native/src/app.rs:1124-1132` (o mesmo para a ativa)
- Test: `desktop-native/src/app/servers.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `App::server_ids() -> Vec<(String /*chave do app*/, Option<String> /*server_id*/)>`.

- [ ] **Step 1: Teste**: `server_id` ausente ou vazio vira `None`; conexão que cai mantém o último
  lido; convite não chama `config()`.
- [ ] **Step 2: Rodar e ver falhar** (`cd desktop-native && nice -n 10 cargo test servers::`).
- [ ] **Step 3: Implementar.**
- [ ] **Step 4: Rodar.** Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(native): learn each server's id for cross-machine groups"`

### Task 17: Modelo global de grupos (puro)

Status: ready-for-agent

**Files:**
- Create: `desktop-native/src/app/groups_model.rs`
- Test: no próprio arquivo (`#[cfg(test)]`)

**Interfaces:**
- Consumes: Task 16.
- Produces:

```rust
pub(crate) enum ListState { Loading, Online, Offline }
pub(crate) struct ServerList<'a> { pub key: &'a str, pub server_id: Option<&'a str>, pub label: &'a str,
    pub color_id: &'a str, pub state: ListState, pub loaded: bool, pub rows: &'a [SessionInfo] }
pub(crate) enum Presence { Here, MachineOffline, NotConnected, Loading }
pub(crate) struct MemberView { pub server_key: Option<String>, pub server_label: String, pub name: String,
    pub presence: Presence }
pub(crate) struct GroupView { pub key: String, pub gid: String, pub owner: Option<String>, pub task: String,
    pub members: Vec<MemberView>, pub machines: usize, pub waiting: usize }
pub(crate) fn build(lists: &[ServerList]) -> Vec<GroupView>;
```

Regras: chave = `owner::gid` com `pair_owner`, senão `<chave da máquina>::gid`; membro presente só
se a própria máquina dele, carregada, tem a linha com o mesmo `pair_gid` (e `pair_owner`); par
externo (`pair_external`) nunca vira membro e segue a linha de hoje; par antigo 1:1 (sem
`pair_owner`, um peer `srv::x`) junta as duas metades quando a linha `x` de `srv` aponta de volta
para esta; `srv::nome` de máquina conhecida fora do ar → `MachineOffline` (com a última linha boa,
se houver); de `server_id` sem máquina no app → `NotConnected`; máquina carregando → `Loading`;
máquina no ar sem a linha → membro fora; `server_id` repetido em duas entradas do app → as
referências a ele viram `NotConnected`.

- [ ] **Step 1: Testes**

```rust
#[test] fn local_groups_with_same_gid_on_two_servers_stay_apart() {}
#[test] fn federated_group_is_one_with_members_of_both_servers() {}
#[test] fn legacy_one_to_one_pair_is_joined_when_both_sides_point_back() {}
#[test] fn external_pair_is_never_a_member() {}
#[test] fn offline_server_member_is_kept_and_marked() {}
#[test] fn unknown_server_member_is_not_connected() {}
#[test] fn member_that_left_on_its_online_server_is_dropped() {}
#[test] fn same_name_two_servers() {}
#[test] fn duplicated_server_id_is_not_guessed() {}
#[test] fn loading_server_member_is_loading() {}
#[test] fn waiting_counts_awaiting_input_members() {}
```

- [ ] **Step 2: Rodar e ver falhar.**
- [ ] **Step 3: Implementar.**
- [ ] **Step 4: Rodar.** Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(native): build one group view from every server's list"`

### Task 18: Seção "Grupos" na barra lateral

Status: ready-for-agent (D1 decidida: todos os grupos na seção do topo)

**Files:**
- Modify: `desktop-native/src/app.rs:4877-4930` (membros de grupo saem dos blocos por máquina e das
  seções; a seção nova vem antes), `desktop-native/src/app/sidebar.rs:369-393,430-499` (`layout`,
  `remote_layout`, `visible_order` com a seção primeiro), `desktop-native/src/app/grouping.rs:58-73,89,289-340`
  (`cluster`, `pair_key`, `render_pair_header` a partir de `GroupView`), `messages/pt.json`,
  `messages/en.json`
- Test: `desktop-native/src/app/sidebar.rs` (`#[cfg(test)]`, ordem de navegação)

**Interfaces:**
- Consumes: Task 17 (`GroupView`).

Desenho (Design Guides do gpui-kit lidos antes; rodar a lista de revisão deles no fim):
- Cabeçalho da seção: `Grupos` (`native_groups_section`) com contagem, como os cabeçalhos de máquina.
- Cabeçalho de cada grupo: rótulo da tarefa como hoje (`split_code`); à direita, contagem de
  membros, `N máquinas` quando `machines > 1`, `N aguardando` quando `waiting > 0`. Recolher como hoje
  (`pair_key` passa a ser a chave do `GroupView`).
- Membro presente: a linha de sessão de hoje com o selo da máquina (ponto `theme::server_color(id)` +
  rótulo curto) quando há mais de uma máquina no app.
- `MachineOffline`: a última linha boa, esmaecida, estado ao vivo trocado por `máquina
  indisponível` (`native_group_member_offline`); abre o chat como hoje.
- `NotConnected`: nome + máquina + `não conectada neste app` (`native_group_member_not_connected`);
  não é clicável como sessão.
- `Loading`: nome + máquina + `carregando` (`native_group_member_loading`).
- Sem grupos: a seção não aparece.

- [ ] **Step 1: Teste** de `visible_order`: grupo com membros de duas máquinas vem antes dos blocos
  de máquina, uma vez só; os membros não aparecem de novo nos blocos.
- [ ] **Step 2: Rodar e ver falhar.**
- [ ] **Step 3: Implementar.**
- [ ] **Step 4: Rodar** o teste e o de i18n (`i18n.rs:46`). Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(native): show each group once with members from every machine"`

### Task 19: Parear sessões de máquinas diferentes pelo nativo

Status: ready-for-agent

**Files:**
- Modify: `desktop-native/src/app/grouping.rs:14-41,182-211,340-540` (`can_pair` sem `OtherServer`
  e `CrossServer`; `group_candidates` de todas as máquinas; `AskMode::Join.target` vira `Target`;
  pedido vai à máquina da origem com o alvo qualificado `server_id::nome`), `messages/*.json`
- Test: `desktop-native/src/app/grouping.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Testes**: `can_pair` aceita outra máquina quando as duas têm `server_id`; recusa com
  `Refusal::NoServerId` (`grupo_recusa_sem_identificador`) quando falta; mantém `Orq`, `Dead`, `Same`,
  `SameGroup`, `Invite` e par externo; o corpo do pedido leva `peers: ["lab::c"]`.
- [ ] **Step 2: Rodar e ver falhar.**
- [ ] **Step 3: Implementar.** Erros do servidor aparecem como hoje (`failed()`), inclusive os
  códigos novos.
- [ ] **Step 4: Rodar.** Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(native): pair sessions across machines"`

### Task 20: Painel do grupo com membros de todas as máquinas

Status: ready-for-agent

**Files:**
- Modify: `desktop-native/src/app/group_sheet.rs:1-3,101-126,217-291` (membros do `GroupView`;
  conversa de cada membro pela máquina dele; contrato pela dona — `pair_owner` → máquina, grupo local
  → máquina do membro; sair pela máquina do membro; candidato de qualquer máquina; "enviar ao grupo"
  usa `group-message` da máquina do membro aberto em vez de `broadcast`), `messages/*.json`
- Test: `desktop-native/src/app/group_sheet.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Testes**: dona fora → área do contrato diz `contrato na máquina <x>, indisponível`
  (`native_group_contract_offline`) e o resto do painel funciona; membro `NotConnected` sem conversa;
  erro de um membro no rodapé sem esconder os outros.
- [ ] **Step 2: Rodar e ver falhar.**
- [ ] **Step 3: Implementar.**
- [ ] **Step 4: Rodar.** Esperado: PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(native): group panel spans every machine"`

### Task 21: Verificação manual com o dono (nativo)

Status: ready-for-human

- [ ] **Step 1: verificação manual — grupo entre duas máquinas no nativo**: build otimizado; print da
  barra lateral com o grupo em cima, membros com o selo de cada máquina e nenhum repetido no bloco da
  máquina.
- [ ] **Step 2: verificação manual — máquina fora**: parar o backend de uma máquina; print com o
  membro esmaecido e "máquina indisponível"; religar e ver voltar.
- [ ] **Step 3: verificação manual — arrastar e painel**: arrastar sessão de uma máquina sobre outra,
  confirmar no diálogo, abrir o painel, ler o contrato e a conversa de cada membro.
