use super::*;
use model::*;
#[derive(Clone)]
pub struct Synchronizer {
    pub home: PathBuf,
    pub claude_dir: PathBuf,
    pub env: BTreeMap<String, String>,
    pub runner: Arc<dyn Runner>,
}
impl Synchronizer {
    pub fn new(
        home: PathBuf,
        claude_dir: PathBuf,
        mut env: BTreeMap<String, String>,
        runner: Arc<dyn Runner>,
    ) -> Self {
        env.retain(|k, _| !k.starts_with("GIT_"));
        for (k, v) in [
            ("HOME", home.to_string_lossy().to_string()),
            ("USERPROFILE", home.to_string_lossy().to_string()),
            (
                "CLAUDE_CONFIG_DIR",
                claude_dir.to_string_lossy().to_string(),
            ),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            (
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "nul" } else { "/dev/null" }.into(),
            ),
            ("GIT_TERMINAL_PROMPT", "0".into()),
            ("GIT_OPTIONAL_LOCKS", "0".into()),
        ] {
            env.insert(k.into(), v);
        }
        Self {
            home,
            claude_dir,
            env,
            runner,
        }
    }
    pub fn ledger(&self) -> PathBuf {
        self.home.join(".hangar/omp-plugin-sync.json")
    }
    pub fn directories(&self) -> Result<crate::accounts::secondary_auth::OmpDirectories, String> {
        crate::accounts::secondary_auth::omp_directories(&self.home, &self.env).map_err(|e| {
            if e == "omp_profile_invalid" {
                err("Nome de perfil OMP inválido")
            } else {
                err("Diretório OMP inválido")
            }
        })
    }
    async fn cli(&self, action: &str, arg: Option<&str>) -> Result<Value, String> {
        let mut args = vec!["omp".into(), "plugin".into(), action.into()];
        if let Some(arg) = arg {
            args.push(arg.into())
        }
        args.push("--json".into());
        let out = self
            .runner
            .run(
                args,
                &self.home,
                &self.env,
                Duration::from_secs(if action == "install" { 120 } else { 30 }),
            )
            .await?;
        if out.code != 0 {
            return Err(format!("CLI OMP falhou em {action} (código {})", out.code));
        }
        io::parse(&out.stdout).map_err(|e| {
            if e == "Registro deve ser um objeto" {
                err("Resposta do CLI deve ser um objeto")
            } else {
                err("Resposta JSON do CLI inválida")
            }
        })
    }
    fn catalog_identity(&self, kind: &str, uri: &str) -> Result<Value, String> {
        if uri.is_empty() {
            return Err(err("Origem do catálogo ausente"));
        }
        if kind == "local" {
            let p = if let Some(rest) = uri.strip_prefix("~/") {
                self.home.join(rest)
            } else {
                PathBuf::from(uri)
            };
            return Ok(json!([
                "local",
                p.canonicalize().map_err(io::os_error)?.to_string_lossy()
            ]));
        }
        if !["github", "git", "url"].contains(&kind) {
            return Err(err("Tipo de catálogo não suportado"));
        }
        identity(&if kind == "github" {
            format!(
                "https://github.com/{}.git",
                uri.strip_suffix(".git").unwrap_or(uri)
            )
        } else {
            uri.into()
        })
    }
    fn marketplaces(&self, root: &Path) -> Result<Map<String, Value>, String> {
        let v = io::read(
            &root.parent().ok_or("OSError")?.join("marketplaces.json"),
            Some(json!({"version":1,"marketplaces":[]})),
        )?;
        if v["version"] != 1 {
            return Err(err("Registro de marketplaces inválido"));
        }
        let entries = v["marketplaces"]
            .as_array()
            .ok_or("Registro de marketplaces inválido")?;
        let mut out = Map::new();
        for e in entries {
            let name = e["name"]
                .as_str()
                .ok_or("Entrada de marketplace inválida")?;
            if out.contains_key(name) {
                return Err(err("Marketplace duplicado no registro nativo"));
            }
            self.catalog_identity(
                e["sourceType"].as_str().unwrap_or(""),
                e["sourceUri"].as_str().unwrap_or(""),
            )?;
            out.insert(name.into(), e.clone());
        }
        Ok(out)
    }
    pub async fn import_marketplaces(&self, dry: bool, stop: &Stop<'_>) -> Value {
        let mut report = report(dry);
        if let Err(e) = self.import_inner(dry, stop, &mut report).await {
            push_error(&mut report, e)
        }
        report
    }
    async fn import_inner(
        &self,
        dry: bool,
        stop: &Stop<'_>,
        report: &mut Value,
    ) -> Result<(), String> {
        let mut directories = self.directories()?;
        let _guard = if dry {
            None
        } else {
            if io::symlink(&self.ledger()) {
                return Err(err("Estado gerenciado simbólico recusado"));
            }
            let lock = io::lock(&self.ledger().with_extension("lock")).await?;
            directories = self.directories()?;
            Some(lock)
        };
        let root = directories.data_root.join("plugins");
        let known = io::read(
            &self.claude_dir.join("plugins/known_marketplaces.json"),
            Some(json!({})),
        )?;
        let mut native = self.marketplaces(&root)?;
        report["complete_inventory"] = json!(true);
        for (alias, e) in known.as_object().unwrap() {
            if stop() {
                break;
            }
            let mut item = json!({"identity":alias,"action":"diagnostic"});
            let result: Result<(), String> = async {
                let s = e["source"]
                    .as_object()
                    .ok_or("Fonte do marketplace Claude inválida")?;
                if truth(s.get("ref")) || truth(s.get("branch")) {
                    return Err(err(
                        "Referência de catálogo não representável pelo importador nativo",
                    ));
                }
                let kind = s.get("source").and_then(Value::as_str).unwrap_or("");
                let (kind, uri) = match kind {
                    "directory" => ("local", s.get("path")),
                    "github" => ("github", s.get("repo")),
                    "git" | "url" => (kind, s.get("url")),
                    _ => return Err(err("Fonte de catálogo não suportada")),
                };
                let uri = uri
                    .and_then(Value::as_str)
                    .ok_or("Origem do catálogo ausente")?;
                let id = self.catalog_identity(kind, uri)?;
                let location = e["installLocation"]
                    .as_str()
                    .ok_or("Catálogo local não encontrado")?;
                let local = Path::new(location);
                let p = local.join(".claude-plugin/marketplace.json");
                let cat = io::read(
                    &if p.exists() {
                        p
                    } else {
                        local.join("marketplace.json")
                    },
                    None,
                )?;
                let name = cat["name"]
                    .as_str()
                    .filter(|s| matches_re(r"\A[A-Za-z0-9][A-Za-z0-9._-]*\z", s))
                    .ok_or("Catálogo local inválido")?;
                if !cat["plugins"].is_array() {
                    return Err(err("Catálogo local inválido"));
                }
                item["name"] = json!(name);
                if let Some(e) = native.get(name) {
                    let same = self.catalog_identity(
                        e["sourceType"].as_str().unwrap_or(""),
                        e["sourceUri"].as_str().unwrap_or(""),
                    )? == id;
                    item["action"] = json!(if same { "unchanged" } else { "conflict" });
                    item["reason"] = json!(if same {
                        "Catálogo existente preservado"
                    } else {
                        "Nome existente com outra origem; preservado"
                    });
                    return Ok(());
                }
                item["action"] = json!("import");
                if dry {
                    item["planned"] = json!(true);
                    return Ok(());
                }
                if stop() {
                    item["action"] = json!("interrupted");
                    item["reason"] = json!("Parada solicitada antes da importação");
                    return Ok(());
                }
                let out = self
                    .runner
                    .run(
                        vec![
                            "omp".into(),
                            "plugin".into(),
                            "marketplace".into(),
                            "add".into(),
                            uri.into(),
                        ],
                        &self.home,
                        &self.env,
                        Duration::from_secs(120),
                    )
                    .await?;
                if out.code != 0 {
                    return Err(format!("Importação nativa falhou (código {})", out.code));
                }
                native = self.marketplaces(&root)?;
                let e = native
                    .get(name)
                    .ok_or("CLI não confirmou o catálogo e sua origem")?;
                if self.catalog_identity(
                    e["sourceType"].as_str().unwrap_or(""),
                    e["sourceUri"].as_str().unwrap_or(""),
                )? != id
                {
                    return Err(err("CLI não confirmou o catálogo e sua origem"));
                }
                item["reason"] = json!("Catálogo registrado pelo mecanismo nativo");
                Ok(())
            }
            .await;
            if let Err(e) = result {
                item["action"] = json!("error");
                item["reason"] = json!(e);
                report["errors"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"identity":alias,"reason":e}));
            }
            report["items"].as_array_mut().unwrap().push(item);
            if stop() {
                break;
            }
        }
        Ok(())
    }
    async fn native(
        &self,
        root: &Path,
        dry: bool,
        stop: &Stop<'_>,
    ) -> Result<(Map<String, Value>, BTreeSet<String>), String> {
        let package = io::read(&root.join("package.json"), Some(json!({"dependencies":{}})))?;
        let lock = io::read(
            &root.join("omp-plugins.lock.json"),
            Some(json!({"plugins":{},"settings":{}})),
        )?;
        let market = io::read(
            &root.join("installed_plugins.json"),
            Some(json!({"version":2,"plugins":{}})),
        )?;
        let empty = Map::new();
        let deps = package.get("dependencies").unwrap_or(&Value::Null);
        let deps = if deps.is_null() && !package.as_object().unwrap().contains_key("dependencies") {
            &empty
        } else {
            deps.as_object().ok_or("Inventário nativo inválido")?
        };
        let prefs = lock
            .get("plugins")
            .map(Value::as_object)
            .unwrap_or(Some(&empty))
            .ok_or("Inventário nativo inválido")?;
        let settings = lock
            .get("settings")
            .map(Value::as_object)
            .unwrap_or(Some(&empty))
            .ok_or("Inventário nativo inválido")?;
        let marketplaces = market["plugins"]
            .as_object()
            .ok_or("Inventário nativo inválido")?;
        let mut states = Map::new();
        for (n, s) in deps {
            let s = s.as_str().ok_or("Dependência nativa inválida")?;
            if !name(n) {
                return Err(err("Dependência nativa inválida"));
            }
            let p = prefs
                .get(n)
                .ok_or("Preferências nativas ausentes ou inválidas")?;
            let enabled = p["enabled"]
                .as_bool()
                .ok_or("Preferências nativas ausentes ou inválidas")?;
            let f = p.get("enabledFeatures").cloned().unwrap_or(Value::Null);
            if !features(&f) {
                return Err(err("Seleção de features inválida"));
            }
            let path = package_path(root, n);
            let installed = io::read(&path.join("package.json"), None)?;
            if installed["name"] != *n {
                return Err(err("Nome instalado diverge do registro"));
            }
            let proof = (|| -> Result<Value, String> {
                let (origin, rev) = spec(s)?;
                let resolved = match git_identity(&path)? {
                    Some(v) => Some(v),
                    None => bun_revision(root, n)?,
                };
                if resolved == Some((origin.clone(), rev.clone())) {
                    manifest(&path)?;
                    Ok(json!({"origin":origin,"revision":rev}))
                } else {
                    Ok(Value::Null)
                }
            })()
            .unwrap_or(Value::Null);
            states.insert(n.clone(),json!({"spec":s,"enabled":enabled,"features":f,"settings":settings.get(n).cloned().unwrap_or(json!({})),"version":installed.get("version").cloned().unwrap_or(Value::Null),"path":path.to_string_lossy(),"proof":proof,"digest":digest(&path)?}));
        }
        if !dry {
            if stop() {
                return Err(err("Passagem interrompida antes de consultar o CLI"));
            }
            let listing = self.cli("list", None).await?;
            let npm = listing["npm"]
                .as_array()
                .ok_or("Formato do inventário CLI inválido")?;
            let market_cli = listing["marketplace"]
                .as_array()
                .ok_or("Formato do inventário CLI inválido")?;
            let mut seen = BTreeSet::new();
            for item in npm {
                let n = item["name"]
                    .as_str()
                    .ok_or("CLI e registros nativos divergem")?;
                let state = states.get(n).ok_or("CLI e registros nativos divergem")?;
                if !seen.insert(n.to_string()) {
                    return Err(err("CLI e registros nativos divergem"));
                }
                let path = item["path"]
                    .as_str()
                    .ok_or("Caminho nativo ausente no inventário CLI")?;
                let path = lexical_absolute(&self.home, path);
                if item["enabled"] != state["enabled"]
                    || item["enabledFeatures"] != state["features"]
                    || item["version"] != state["version"]
                    || json!(path.to_string_lossy()) != state["path"]
                    || !item["manifest"].is_object()
                {
                    return Err(err("CLI e registros nativos divergem"));
                }
            }
            if seen != states.keys().cloned().collect() {
                return Err(err("CLI omitiu uma instalação nativa"));
            }
            let ids = market_cli
                .iter()
                .filter_map(|e| e["id"].as_str())
                .map(String::from)
                .collect::<Vec<_>>();
            if ids.len() != market_cli.len()
                || ids.into_iter().collect::<BTreeSet<_>>()
                    != marketplaces.keys().cloned().collect()
            {
                return Err(err("Inventário marketplace divergente"));
            }
        }
        Ok((states, marketplaces.keys().cloned().collect()))
    }
    async fn sources(
        &self,
        dry: bool,
        stop: &Stop<'_>,
        report: &mut Value,
    ) -> Result<(Map<String, Value>, BTreeSet<String>, BTreeSet<String>), String> {
        let registry = io::read(
            &self.claude_dir.join("plugins/installed_plugins.json"),
            None,
        )?;
        if registry["version"] != 2 || !registry["plugins"].is_object() {
            return Err(err("Inventário Claude inválido"));
        }
        let known = io::read(
            &self.claude_dir.join("plugins/known_marketplaces.json"),
            Some(json!({})),
        )?;
        let settings = io::read(&self.claude_dir.join("settings.json"), Some(json!({})))?;
        let empty = Map::new();
        let enabled = settings
            .get("enabledPlugins")
            .map(Value::as_object)
            .unwrap_or(Some(&empty))
            .ok_or("Configuração de habilitação inválida")?;
        let registry = registry["plugins"].as_object().unwrap();
        let mut out = Map::new();
        let mut ambiguous = BTreeSet::new();
        for (id, entries) in registry {
            if !id.contains('@') || !entries.as_array().is_some_and(|a| !a.is_empty()) {
                return Err(err("Entrada Claude inválida"));
            }
            let entries = entries.as_array().unwrap();
            if entries
                .iter()
                .any(|e| !e["installPath"].is_string() || !e["scope"].is_string())
            {
                return Err(err("Entrada Claude incompleta"));
            }
            let candidate:Result<(String,Value),String>=async{
    let users=entries.iter().filter(|e|e["scope"]=="user").collect::<Vec<_>>();if users.len()!=1{return Err(err("Escopo não pode ser preservado como instalação global"))}let entry=users[0];let rev=entry["gitCommitSha"].as_str().filter(|s|sha(s)).ok_or("Revisão Claude sem SHA completo")?;let active=enabled.get(id).unwrap_or(&Value::Bool(false)).as_bool().ok_or("Habilitação Claude inválida")?;let (plugin,catalog)=id.rsplit_once('@').unwrap();let catalog=&known[catalog];let local=catalog["installLocation"].as_str().ok_or("Origem do catálogo ausente")?;let cat=io::read(&Path::new(local).join(".claude-plugin/marketplace.json"),None)?;let candidates=cat["plugins"].as_array().ok_or("Catálogo inválido")?.iter().filter(|e|e["name"]==plugin).collect::<Vec<_>>();if candidates.len()!=1{return Err(err("Origem do plugin ambígua"))}let mut source=&candidates[0]["source"];if let Some(s)=source.as_str(){if s!="."&&s!="./"{return Err(err("Plugin em subpasta não pode ser instalado pela raiz"))}source=&catalog["source"];}
    let kind=source["source"].as_str().unwrap_or("");if !["url","git","github"].contains(&kind){return Err(err("Origem sem raiz instalável verificável"))}let url=if kind=="github"{format!("https://github.com/{}.git",source["repo"].as_str().unwrap_or(""))}else{source["url"].as_str().unwrap_or("").to_string()};let origin=identity(&url)?;if !source["sha"].is_null()&&source["sha"]!=rev{return Err(err("SHA do catálogo diverge da instalação Claude"))}let root=Path::new(entry["installPath"].as_str().unwrap());let package=manifest(root)?;if git_identity(root)?!=Some((origin.clone(),rev.into())){return Err(err("Checkout Claude não comprova origem e revisão"))}if stop(){return Err(err("__stopped"))}
    if !dry{let out=self.runner.run(vec!["git".into(),"-c".into(),format!("core.hooksPath={}",if cfg!(windows){"nul"}else{"/dev/null"}),"-c".into(),"core.fsmonitor=false".into(),"show".into(),format!("{rev}:package.json")],root,&self.env,Duration::from_secs(15)).await?;if out.code!=0||serde_json::from_str::<Value>(&out.stdout).map_err(|_|err("JSONDecodeError"))?!=package{return Err(err("Manifesto local diverge da revisão Git"))}}
    let n=package["name"].as_str().unwrap().to_string();Ok((n.clone(),json!({"id":id,"name":n,"origin":origin,"url":url,"revision":rev,"enabled":active})))
   }.await;
            match candidate {
                Ok((n, v)) => {
                    if out.contains_key(&n) || ambiguous.contains(&n) {
                        if let Some(previous) = out.remove(&n) {
                            report["items"].as_array_mut().unwrap().push(json!({"identity":previous["id"],"name":n,"action":"diagnostic","reason":"Mais de uma origem para o pacote nativo"}));
                        }
                        ambiguous.insert(n);
                        report["items"].as_array_mut().unwrap().push(json!({"identity":id,"action":"diagnostic","reason":"Mais de um plugin Claude usa o mesmo pacote nativo"}));
                    } else {
                        out.insert(n, v);
                    }
                }
                Err(e) if e == "__stopped" => break,
                Err(e) => report["items"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"identity":id,"action":"diagnostic","reason":e})),
            }
        }
        Ok((out, registry.keys().cloned().collect(), ambiguous))
    }
    fn validate_ledger(&self, root: &Path, l: &Value) -> Result<(), String> {
        if l["version"] != 1 || !l["items"].is_object() {
            return Err(err("Registro de gestão inválido"));
        }
        for (n, r) in l["items"].as_object().unwrap() {
            if !name(n)
                || !r.is_object()
                || !matches!(
                    r["status"].as_str(),
                    Some("managed" | "pending" | "suspended")
                )
            {
                return Err(err("Registro de propriedade inválido"));
            }
            let s = &r["source"];
            let id = s["id"].as_str().unwrap_or("");
            if s["name"] != *n
                || !id
                    .rsplit_once('@')
                    .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
                || !s["revision"].as_str().is_some_and(sha)
                || !s["enabled"].is_boolean()
                || !s["url"].is_string()
            {
                return Err(err("Vínculo de origem inválido no registro de gestão"));
            }
            let origin = identity(s["url"].as_str().unwrap())?;
            if s["origin"] != origin {
                return Err(err("Origem inconsistente no registro de gestão"));
            }
            let native = r
                .get("native")
                .ok_or("Estado nativo ausente no registro de gestão")?;
            if native.is_null() {
                if r["status"] == "managed" {
                    return Err(err("Gestão confirmada exige prova nativa"));
                }
                continue;
            }
            if !native.is_object()
                || !native["enabled"].is_boolean()
                || !native["settings"].is_object()
                || !native["digest"]
                    .as_str()
                    .is_some_and(|s| matches_re(r"\A[0-9a-f]{64}\z", s))
                || native["path"] != json!(package_path(root, n).to_string_lossy())
                || !native["spec"].is_string()
                || !native.as_object().unwrap().contains_key("version")
                || (!native["version"].is_null() && !native["version"].is_string())
            {
                return Err(err("Estado nativo inválido no registro de gestão"));
            }
            if !native.as_object().unwrap().contains_key("features") {
                return Err(err("Seleção nativa ausente no registro de gestão"));
            }
            if !features(&native["features"]) {
                return Err(err("Seleção nativa inválida no registro de gestão"));
            }
            let p = &native["proof"];
            if p["origin"] != origin
                || !p["revision"].as_str().is_some_and(sha)
                || spec(native["spec"].as_str().unwrap())?
                    != (origin, p["revision"].as_str().unwrap().into())
            {
                return Err(err("Prova nativa inconsistente no registro de gestão"));
            }
        }
        Ok(())
    }
    fn save(&self, root: &Path, l: &Value) -> Result<(), String> {
        self.validate_ledger(root, l)?;
        io::write(&self.ledger(), l)
    }
    fn change_pin(&self, root: &Path, n: &str, expected: &Value, new: &str) -> Result<(), String> {
        let p = root.join("package.json");
        let mut package = io::read(&p, None)?;
        if !package["dependencies"].is_object() || package["dependencies"].get(n) != Some(expected)
        {
            return Err(err("Pin nativo foi alterado por outro escritor"));
        }
        package["dependencies"][n] = json!(new);
        io::write(&p, &package)
    }
    async fn restore_pin(
        &self,
        root: &Path,
        n: &str,
        pin: &str,
        previous: &Value,
    ) -> Result<(), String> {
        let observed = self.native(root, true, &|| false).await?.0;
        let Some(current) = observed.get(n) else {
            return Ok(());
        };
        if current["spec"] != pin {
            return Ok(());
        }
        let unchanged = previous
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k != "spec" && *k != "proof")
            .all(|(k, v)| current.get(k) == Some(v));
        let proof = match git_identity(Path::new(current["path"].as_str().ok_or("OSError")?))? {
            Some(v) => Some(v),
            None => bun_revision(root, n)?,
        };
        let original = &previous["proof"];
        if unchanged
            && proof
                == Some((
                    original["origin"].clone(),
                    original["revision"].as_str().ok_or("OSError")?.into(),
                ))
        {
            self.change_pin(
                root,
                n,
                &json!(pin),
                previous["spec"].as_str().ok_or("OSError")?,
            )?;
        }
        Ok(())
    }
    pub async fn reconcile(&self, dry: bool, stop: &Stop<'_>) -> Value {
        let mut report = report(dry);
        if let Err(e) = self.reconcile_inner(dry, stop, &mut report).await {
            push_error(&mut report, e)
        }
        report
    }
    async fn reconcile_inner(
        &self,
        dry: bool,
        stop: &Stop<'_>,
        report: &mut Value,
    ) -> Result<(), String> {
        let mut directories = self.directories()?;
        if !dry && stop() {
            return Ok(());
        }
        let _guard = if dry {
            None
        } else {
            if io::symlink(&self.ledger()) {
                return Err(err("Estado gerenciado simbólico recusado"));
            }
            let g = io::lock(&self.ledger().with_extension("lock")).await?;
            directories = self.directories()?;
            Some(g)
        };
        if !dry && stop() {
            return Ok(());
        }
        let root = directories.data_root.join("plugins");
        let mut ledger = io::read(&self.ledger(), Some(json!({"version":1,"items":{}})))?;
        self.validate_ledger(&root, &ledger)?;
        let (sources, ids, ambiguous) = self.sources(dry, stop, report).await?;
        if stop() {
            return Ok(());
        }
        let (mut native, marketplaces) = self.native(&root, dry, stop).await?;
        report["complete_inventory"] = json!(true);
        let names = sources
            .keys()
            .chain(ledger["items"].as_object().unwrap().keys())
            .chain(ambiguous.iter())
            .cloned()
            .collect::<BTreeSet<_>>();
        for n in names {
            if stop() {
                break;
            }
            if ambiguous.contains(&n) {
                report["items"].as_array_mut().unwrap().push(json!({"identity":n,"name":n,"action":"diagnostic","reason":"Origem ambígua; instalação e propriedade preservadas"}));
                continue;
            }
            let source = sources.get(&n).cloned();
            let state = native.get(&n).cloned();
            let record = ledger["items"].get(&n).cloned();
            let s = source.as_ref().unwrap_or(&Value::Null);
            let v = state.as_ref().unwrap_or(&Value::Null);
            let r = record.as_ref().unwrap_or(&Value::Null);
            let mut enabled = s["enabled"].clone();
            if source.is_some()
                && state.is_some()
                && record.is_some()
                && s["enabled"] == r["source"]["enabled"]
            {
                enabled = v["enabled"].clone()
            }
            let (action, reason) = if record.is_some() && r["status"] != "managed" {
                (
                    "suspended",
                    "Gestão suspensa; intervenção manual preservada",
                )
            } else if record.is_some() && r["native"] != *v {
                (
                    "suspended",
                    "Instalação OMP foi alterada fora da sincronização",
                )
            } else if source.is_none() {
                if record.is_some() && !ids.contains(r["source"]["id"].as_str().unwrap_or("")) {
                    ("uninstall", "Sem alteração")
                } else {
                    ("diagnostic", "Origem atual não pode ser confirmada")
                }
            } else if marketplaces.contains(s["id"].as_str().unwrap_or("")) {
                (
                    "diagnostic",
                    "Plugin de marketplace preservado; atualizações pertencem ao mecanismo nativo",
                )
            } else if state.is_some()
                && (v["proof"].is_null() || v["proof"]["origin"] != s["origin"])
            {
                (
                    "diagnostic",
                    "Instalação existente não comprova a mesma origem",
                )
            } else if record.is_none() && state.is_some() && s["enabled"] == false {
                (
                    "diagnostic",
                    "Plugin Claude inativo não autoriza adotar instalação manual",
                )
            } else if record.is_none() && state.is_some() {
                if v["proof"]["revision"] == s["revision"] {
                    ("adopt", "Sem alteração")
                } else {
                    ("diagnostic", "Revisões diferentes não autorizam adoção")
                }
            } else if s["enabled"] == false {
                (
                    if state.is_some() && v["enabled"] == true && enabled == false {
                        "disable"
                    } else if state.is_some() {
                        "pending"
                    } else {
                        "unchanged"
                    },
                    "Sem alteração",
                )
            } else if state.is_none() || v["proof"]["revision"] != s["revision"] {
                ("install", "Sem alteração")
            } else if v["enabled"] == false && enabled == true {
                ("enable", "Sem alteração")
            } else {
                ("unchanged", "Sem alteração")
            };
            let item = json!({"identity":if source.is_some(){s["id"].clone()}else{r["source"].get("id").cloned().unwrap_or(json!(n))},"name":n,"action":action,"reason":reason,"revision":s["revision"]});
            report["items"].as_array_mut().unwrap().push(item);
            let index = report["items"].as_array().unwrap().len() - 1;
            if dry {
                report["items"][index]["planned"] = json!(true);
                continue;
            }
            if action == "suspended" {
                if record.is_some() {
                    ledger["items"][&n]["status"] = json!("suspended");
                    self.save(&root, &ledger)?
                }
                continue;
            }
            if ["diagnostic", "pending", "unchanged"].contains(&action) {
                if record.is_some()
                    && source.is_some()
                    && action != "diagnostic"
                    && r["source"] != *s
                {
                    ledger["items"][&n]["source"] = s.clone();
                    self.save(&root, &ledger)?
                }
                continue;
            }
            if action == "adopt" {
                ledger["items"][&n] = json!({"status":"managed","source":s,"native":v});
                self.save(&root, &ledger)?;
                continue;
            }
            let before = self.native(&root, false, stop).await?.0;
            if before.get(&n) != state.as_ref() {
                return Err(err("Instalação mudou antes da ação"));
            }
            if stop() {
                report["items"][index]["action"] = json!("interrupted");
                report["items"][index]["reason"] = json!("Parada solicitada antes da operação");
                break;
            }
            ledger["items"][&n] = json!({"status":"pending","source":if source.is_some(){s.clone()}else{r["source"].clone()},"native":v});
            self.save(&root, &ledger)?;
            let mut prepared = None;
            let effect: Result<Option<Map<String, Value>>, String> = async {
                let mut argument = n.clone();
                if action == "install" {
                    let suffix = match v["features"].as_array() {
                        Some(f) => format!(
                            "[{}]",
                            f.iter()
                                .map(|v| v.as_str().unwrap())
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                        None => String::new(),
                    };
                    let pin = format!(
                        "git+{}#{}",
                        s["url"]
                            .as_str()
                            .unwrap()
                            .strip_prefix("git+")
                            .unwrap_or(s["url"].as_str().unwrap()),
                        s["revision"].as_str().unwrap()
                    );
                    argument = format!("{pin}{suffix}");
                    if state.is_some() && v["spec"] != pin {
                        self.change_pin(&root, &n, &v["spec"], &pin)?;
                        prepared = Some(pin)
                    }
                }
                self.cli(action, Some(&argument)).await?;
                if action == "install" && enabled != true {
                    self.cli("disable", Some(&n)).await?;
                }
                if stop() {
                    ledger["items"][&n]["status"] = json!("suspended");
                    self.save(&root, &ledger)?;
                    return Ok(None);
                }
                let after = self.native(&root, false, stop).await?.0;
                let confirmed = after.get(&n);
                if action == "uninstall" {
                    if confirmed.is_some() {
                        return Err(err("CLI não confirmou a remoção"));
                    }
                    ledger["items"].as_object_mut().unwrap().remove(&n);
                } else {
                    let c = confirmed.ok_or("Efeito nativo não corresponde à ação solicitada")?;
                    if c["proof"].is_null()
                        || c["proof"]["origin"] != s["origin"]
                        || c["enabled"] != enabled
                        || (action == "install" && c["proof"]["revision"] != s["revision"])
                        || (state.is_some()
                            && (c["features"] != v["features"] || c["settings"] != v["settings"]))
                    {
                        return Err(err("Efeito nativo não corresponde à ação solicitada"));
                    }
                    ledger["items"][&n] = json!({"status":"managed","source":s,"native":c});
                }
                self.save(&root, &ledger)?;
                Ok(Some(after))
            }
            .await;
            match effect {
                Ok(Some(after)) => native = after,
                Ok(None) => break,
                Err(failure) => {
                    let cleanup: Result<(), String> = async {
                        if let Some(pin) = prepared {
                            self.restore_pin(&root, &n, &pin, v).await?
                        }
                        let observed = self.native(&root, true, &|| false).await?.0;
                        if observed.get(&n) == state.as_ref() {
                            if let Some(r) = record {
                                ledger["items"][&n] = r;
                            } else {
                                ledger["items"].as_object_mut().unwrap().remove(&n);
                            }
                        } else {
                            ledger["items"][&n]["status"] = json!("suspended")
                        }
                        self.save(&root, &ledger)
                    }
                    .await;
                    if cleanup.is_err() {
                        tracing::warn!(
                            code = "omp_plugin_rollback_failed",
                            plugin = n,
                            "desfazer falhou após a ação; preservando a causa original"
                        );
                    }
                    return Err(failure);
                }
            }
        }
        Ok(())
    }
}
fn lexical_absolute(home: &Path, p: &str) -> PathBuf {
    let p = home.join(p);
    let mut out = PathBuf::new();
    for c in p.components() {
        if c == std::path::Component::ParentDir {
            out.pop();
        } else if c != std::path::Component::CurDir {
            out.push(c)
        }
    }
    out
}
fn package_path(root: &Path, name: &str) -> PathBuf {
    name.split('/')
        .fold(root.join("node_modules"), |path, part| path.join(part))
}
