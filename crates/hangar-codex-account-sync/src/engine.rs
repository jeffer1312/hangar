//! Uma preparação por conta, com assinatura confirmada somente depois dos efeitos em disco.
use crate::{
    Error, Native, Paths, blocking, config, hash, issue, json_bytes, object, public_state, read,
    resources, status, storage,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

pub fn preparation_status(paths: Paths<'_>, is_default: bool) -> Value {
    if is_default {
        return status("ready", vec![], false, None, None);
    }
    if paths.destination.is_symlink() || !paths.destination.is_dir() {
        return failure("codex_account_destination_invalid");
    }
    storage::read_state(paths)
        .map(|state| public_state(&state))
        .unwrap_or_else(|_| failure("codex_account_state_invalid"))
}
fn failure(code: &str) -> Value {
    status("error", vec![issue(code, json!({}))], false, None, None)
}

/// Prepara a conta adicional e conserva o estado anterior se a fonte mudar durante a escrita.
pub async fn prepare(
    paths: Paths<'_>,
    is_default: bool,
    force: bool,
    native: &mut impl Native,
) -> Value {
    if is_default {
        return status("ready", vec![], false, None, None);
    }
    if paths.destination.is_symlink() || !paths.destination.is_dir() {
        return failure("codex_account_destination_invalid");
    }
    let mut previous = match storage::read_state(paths) {
        Ok(value) => value,
        Err(_) => return failure("codex_account_state_invalid"),
    };
    let _guard = match storage::lock(&paths.destination.join(".hangar-integracao.lock")).await {
        Ok(guard) => guard,
        Err(error) => {
            return status(
                "error",
                vec![issue(
                    "codex_account_prepare_failed",
                    json!({"error":error.kind()}),
                )],
                false,
                None,
                None,
            );
        }
    };
    let result = async {
        previous = storage::read_state(paths)?;
        run(paths, force, &mut previous, native).await
    }
    .await;
    match result {
        Ok(value) => value,
        Err(error) => {
            let code = if matches!(error, Error::Changed) {
                "codex_account_changed_during_prepare"
            } else {
                "codex_account_prepare_failed"
            };
            let params = if matches!(error, Error::Changed) {
                json!({})
            } else {
                json!({"error":error.kind()})
            };
            let value = status("error", vec![issue(code, params)], false, None, None);
            previous["public"] = value.clone();
            invalidate_cache(&mut previous);
            let _ = storage::write_state(paths, &previous);
            value
        }
    }
}
fn destination_paths(state: &Value) -> BTreeSet<String> {
    let mut paths = BTreeSet::from(["config.toml".into()]);
    paths.extend(object(&state["resources"]).into_iter().map(|(key, _)| key));
    paths.extend(object(&state["profiles"]).into_iter().map(|(key, _)| key));
    paths
}
fn profile_names(source: &Path) -> Result<BTreeSet<String>, Error> {
    let mut names = BTreeSet::new();
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.ends_with(".config.toml") && (path.is_file() || path.is_symlink()) {
            names.insert(name.into_owned());
        }
    }
    Ok(names)
}
fn metadata(path: &Path) -> Result<Value, Error> {
    let stat = match fs::symlink_metadata(path) {
        Ok(stat) => stat,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(e) => return Err(e.into()),
    };
    let mut value = json!({"directory":stat.is_dir(),"readonly":stat.permissions().readonly()});
    if !stat.is_dir() || stat.is_symlink() {
        value["len"] = json!(stat.len());
        value["modified"] = json!(
            stat.modified()
                .ok()
                .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos().to_string())
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        value["mode"] = json!(stat.mode());
        value["ino"] = json!(stat.ino());
        value["dev"] = json!(stat.dev());
        if !stat.is_dir() || stat.is_symlink() {
            value["ctime"] = json!([stat.ctime(), stat.ctime_nsec()]);
        }
    }
    if stat.is_symlink() {
        value["target"] = json!(fs::read_link(path)?);
        value["resolved"] = json!(fs::canonicalize(path).ok());
        if let Ok(target) = fs::metadata(path) {
            value["target_len"] = json!(target.len());
            value["target_modified"] = json!(
                target
                    .modified()
                    .ok()
                    .and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos().to_string())
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                value["target_meta"] = json!([
                    target.mode() as i128,
                    target.ino() as i128,
                    target.dev() as i128,
                    target.ctime() as i128,
                    target.ctime_nsec() as i128
                ]);
            }
        }
    }
    Ok(value)
}
fn inventory(root: &Path, relative: &BTreeSet<String>) -> Result<String, Error> {
    let mut paths = BTreeSet::from([root.to_owned()]);
    for item in relative {
        if !resources::safe_relative(item) {
            return Err(Error::Invalid);
        }
        let path = root.join(item);
        paths.insert(path.clone());
        for parent in path.ancestors().skip(1) {
            if parent == root || parent.starts_with(root) {
                paths.insert(parent.to_owned());
            }
        }
    }
    let mut entries = Vec::new();
    for path in paths {
        entries.push(json!([path, metadata(&path)?]));
    }
    Ok(hash(&json_bytes(&json!(entries))))
}
fn source_inventory(paths: Paths<'_>) -> Result<(String, Vec<Value>), Error> {
    let (files, issues) = resources::collect_names(paths);
    let mut names = BTreeSet::from(["config.toml".into()]);
    names.extend(files.into_keys());
    names.extend(profile_names(paths.source)?);
    Ok((
        hash(&json_bytes(
            &json!({"inventory":inventory(paths.source,&names)?,"issues":issues}),
        )),
        issues,
    ))
}
fn source_config(native: &impl Native, root: &Path, relative: &str) -> Result<Value, Error> {
    let raw = read(&root.join(relative))?;
    if relative == "config.toml" && raw.is_none() {
        return Ok(json!({}));
    }
    let raw = raw.ok_or(Error::Invalid)?;
    let value = native
        .parse_toml(std::str::from_utf8(&raw).map_err(|_| Error::Unicode)?)
        .map_err(|_| Error::Toml)?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(Error::Invalid)
    }
}
async fn run(
    paths: Paths<'_>,
    force: bool,
    state: &mut Value,
    native: &mut impl Native,
) -> Result<Value, Error> {
    if fs::canonicalize(paths.source).unwrap_or(std::path::absolute(paths.source)?)
        == fs::canonicalize(paths.destination)?
    {
        return Ok(failure("codex_account_source_destination_conflict"));
    }
    let version = native
        .cli_version(force)
        .await
        .unwrap_or_else(|_| "indisponível".into());
    let (initial_inventory, inventory_issues) = source_inventory(paths)?;
    let mut public = public_state(state);
    let old_paths = destination_paths(state);
    let now = native.now();
    let checked = state
        .get("plugins_attempted_at")
        .or_else(|| state.get("plugins_checked_at"))
        .and_then(Value::as_f64);
    let non_plugin: Vec<Value> = public["issues"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| {
            v["code"].as_str().is_none_or(|s| {
                !s.starts_with("codex_account_plugin_") && s != "codex_account_unknown_preference"
            })
        })
        .cloned()
        .collect();
    if !force
        && inventory_issues.is_empty()
        && !blocking(&non_plugin)
        && matches!(public["status"].as_str(), Some("ready" | "partial"))
        && state["cli_version"] == version
        && state["source_inventory"] == initial_inventory
        && checked.is_some_and(|t| now - t >= 0.0 && now - t < 300.0)
    {
        let destination_inventory = inventory(paths.destination, &old_paths)?;
        if state["destination_inventory"] == destination_inventory {
            if public["trust_pending"] == true {
                let trust = native.check_trust().await.unwrap_or(None);
                let mut issues: Vec<Value> =
                    public["issues"].as_array().cloned().unwrap_or_default();
                if let Some(trust) = trust {
                    public["trust_pending"] = json!(trust);
                    issues.retain(|v| v["code"] != "codex_account_plugin_trust_unavailable");
                } else if !issues
                    .iter()
                    .any(|v| v["code"] == "codex_account_plugin_trust_unavailable")
                {
                    issues.push(issue("codex_account_plugin_trust_unavailable", json!({})));
                }
                public["status"] = json!(if blocking(&issues) {
                    "partial"
                } else {
                    "ready"
                });
                public["issues"] = json!(issues);
                let (latest, issues) = source_inventory(paths)?;
                if issues.is_empty()
                    && latest == initial_inventory
                    && inventory(paths.destination, &old_paths)? == destination_inventory
                {
                    if public != public_state(state) {
                        let mut next = state.clone();
                        next["public"] = public.clone();
                        storage::write_state(paths, &next)?;
                    }
                    return Ok(public);
                }
            } else {
                return Ok(public);
            }
        }
    }
    let source = source_config(native, paths.source, "config.toml");
    let profiles = profile_names(paths.source)?;
    let mut source_profiles = serde_json::Map::new();
    let mut source_error = source.as_ref().err().map(Error::kind);
    for name in &profiles {
        match source_config(native, paths.source, name) {
            Ok(value) => {
                source_profiles.insert(name.clone(), value);
            }
            Err(error) => {
                source_error.get_or_insert(error.kind());
            }
        }
    }
    if let Some(kind) = source_error {
        let value = status(
            "error",
            vec![issue("codex_account_source_invalid", json!({"path":kind}))],
            false,
            None,
            None,
        );
        let mut next = state.clone();
        next["public"] = value.clone();
        invalidate_cache(&mut next);
        let _ = storage::write_state(paths, &next);
        return Ok(value);
    }
    let source = source?;
    let (source_files, mut source_issues) = resources::collect(paths);
    let (files, overrides) = resources::transform(source_files, paths, native, &mut source_issues);
    let mut relative = BTreeSet::from(["config.toml".into()]);
    relative.extend(files.keys().cloned());
    relative.extend(profiles.iter().cloned());
    let (source_digest, source_snapshot) = resources::snapshot(paths.source, &relative, false)?;
    let private = storage::private(paths, true)?;
    let backups = private.join("backups");
    let mut issues = source_issues.clone();
    let trust = public_state(state)["trust_pending"]
        .as_bool()
        .unwrap_or(false);
    stage(paths, state, &issues, trust, "configuracoes")?;
    let available = files
        .keys()
        .cloned()
        .chain(profiles.iter().cloned())
        .collect();
    let config_result = match resources::safe_destination(paths.destination, "config.toml", false) {
        Ok(path) => {
            config::edit(
                native,
                &path,
                &source,
                paths,
                &state["config"],
                &available,
                &backups,
                &private,
                &mut issues,
            )
            .await?
        }
        Err(error) => {
            issues.push(issue(
                "codex_account_path_conflict",
                json!({"path":"config.toml","error":error.kind()}),
            ));
            state.get("config").cloned().unwrap_or(json!({}))
        }
    };
    state["config"] = config_result.clone();
    checkpoint(paths, state)?;
    let mut profile_results = serde_json::Map::new();
    for (name, profile) in &source_profiles {
        let result = match resources::safe_destination(paths.destination, name, false) {
            Ok(path) => {
                config::edit(
                    native,
                    &path,
                    profile,
                    paths,
                    &state["profiles"][name],
                    &available,
                    &backups,
                    &private,
                    &mut issues,
                )
                .await?
            }
            Err(error) => {
                issues.push(issue(
                    "codex_account_path_conflict",
                    json!({"path":name,"error":error.kind()}),
                ));
                state["profiles"].get(name).cloned().unwrap_or(json!({}))
            }
        };
        profile_results.insert(name.clone(), result.clone());
        state["profiles"][name] = result;
        checkpoint(paths, state)?;
    }
    for (name, old) in object(&state["profiles"]) {
        if source_profiles.contains_key(&name) || !old.is_object() {
            continue;
        }
        let result = async {
            let path = resources::safe_destination(paths.destination, &name, false)?;
            if read(&path)?.is_none() {
                return Ok(json!({"values":{}}));
            }
            config::remove(native, &path, &old, &backups, &private, &mut issues).await
        }
        .await;
        match result {
            Ok(value) => {
                if !object(&value["values"]).is_empty() {
                    profile_results.insert(name.clone(), value.clone());
                    state["profiles"][&name] = value;
                } else if let Some(profiles) = state["profiles"].as_object_mut() {
                    profiles.remove(&name);
                }
                checkpoint(paths, state)?;
            }
            Err(error) => {
                issues.push(issue(
                    "codex_account_path_conflict",
                    json!({"path":name,"error":error.kind()}),
                ));
                profile_results.insert(name, old);
            }
        }
    }
    state["profiles"] = json!(profile_results);
    stage(paths, state, &issues, trust, "recursos")?;
    let mut resources = resources::sync(
        &files,
        paths,
        &state["resources"],
        &source_snapshot,
        &backups,
        &mut issues,
        source_issues.is_empty(),
    );
    state["resources"] = resources.clone();
    checkpoint(paths, state)?;
    for (name, desired) in overrides {
        if resources.get(&name).is_none() {
            continue;
        }
        let path = resources::safe_destination(paths.destination, &name, false)?;
        let mut plan = |current: &Value| -> Result<Vec<Value>, String> {
            Ok(object(&desired).into_iter().filter(|(key,value)|current.get(key)!=Some(value)).map(|(key,value)|json!({"keyPath":serde_json::to_string(&key).unwrap_or_default(),"value":value,"mergeStrategy":"replace"})).collect())
        };
        native
            .edit_config(&path, &backups, &private, &mut plan)
            .await
            .map_err(|_| Error::Native)?;
        if let Some(raw) = read(&path)? {
            resources[&name]["hash"] = json!(hash(&raw));
            state["resources"] = resources.clone();
            checkpoint(paths, state)?;
        }
    }
    let mut plugins = state
        .get("plugins")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or(json!({}));
    let mut pending = trust;
    let plugins_configured = !object(&plugins).is_empty()
        || ["marketplaces", "plugins"]
            .iter()
            .any(|key| source[*key].is_object());
    if plugins_configured {
        stage(paths, state, &issues, trust, "plugins")?;
        let outcome = native
            .sync_plugins(paths, &plugins)
            .await
            .map_err(|_| Error::Native)?;
        if let Some(manifest) = outcome.get("manifest") {
            plugins = manifest.clone();
            state["plugins"] = plugins.clone();
            checkpoint(paths, state)?;
        }
        issues.extend(outcome["issues"].as_array().into_iter().flatten().cloned());
        if let Some(trust) = outcome["trust_pending"].as_bool() {
            pending = trust;
        }
    }
    let (final_files, mut final_issues) = resources::collect(paths);
    let (final_files, _) = resources::transform(final_files, paths, native, &mut final_issues);
    let mut final_source_paths = BTreeSet::from(["config.toml".into()]);
    final_source_paths.extend(final_files.into_keys());
    final_source_paths.extend(profile_names(paths.source)?);
    let (final_source_digest, final_source_snapshot) =
        resources::snapshot(paths.source, &final_source_paths, false)?;
    if final_issues != source_issues || final_source_digest != source_digest {
        return Err(Error::Changed);
    }
    let mut final_paths = BTreeSet::from(["config.toml".into()]);
    final_paths.extend(object(&resources).into_iter().map(|(key, _)| key));
    final_paths.extend(profile_results.keys().cloned());
    let (destination_digest, destination_snapshot) =
        resources::snapshot(paths.destination, &final_paths, true)?;
    if resources::snapshot(paths.destination, &final_paths, true)?.0 != destination_digest {
        return Err(Error::Changed);
    }
    let final_inventory = inventory(paths.destination, &final_paths)?;
    config::verify(
        native,
        &paths.destination.join("config.toml"),
        &config_result,
    )?;
    for (name, manifest) in &profile_results {
        if !object(&manifest["values"]).is_empty() || !object(&manifest["restrictions"]).is_empty()
        {
            config::verify(native, &paths.destination.join(name), manifest)?;
        }
    }
    for (name, manifest) in object(&resources) {
        let entry = &destination_snapshot["files"][&name];
        if manifest["target"].is_string() {
            if entry["kind"] != "symlink" || entry["target"] != manifest["target"] {
                return Err(Error::Changed);
            }
        } else if entry["kind"] != "file"
            || entry["hash"] != manifest["hash"]
            || cfg!(unix)
                && manifest.get("executable").is_some()
                && entry["executable"] != manifest["executable"]
        {
            return Err(Error::Changed);
        }
    }
    let count = |prefix: &str| {
        object(&resources)
            .keys()
            .filter(|name| name.starts_with(prefix))
            .count()
    };
    let inherited = json!({"skills":count("skills/"),"hooks":count("hooks/")+count(".hangar-hooks/")+usize::from(resources.get("hooks.json").is_some()),"agents":count("agents/"),"plugins":object(&plugins["plugins"]).len(),"mcps":object(&config_result["values"]["mcp_servers"]).len()});
    let ready = !blocking(&issues);
    let public = status(
        if ready { "ready" } else { "partial" },
        issues,
        pending,
        None,
        Some(inherited),
    );
    let mut result = json!({"public":public,"source_digest":final_source_digest,"destination_digest":destination_digest,"cli_version":version,"source_snapshot":final_source_snapshot,"destination_snapshot":destination_snapshot,"config":config_result,"profiles":profile_results,"resources":resources,"plugins":plugins,"plugins_checked_at":if plugins_configured&&ready&&!pending{Some(native.now())}else{None},"plugins_attempted_at":native.now()});
    let (source_inventory, inventory_issues) = source_inventory(paths)?;
    if source_inventory != initial_inventory
        || inventory(paths.destination, &final_paths)? != final_inventory
    {
        return Err(Error::Changed);
    }
    if inventory_issues.is_empty() {
        result["source_inventory"] = json!(source_inventory);
        result["destination_inventory"] = json!(final_inventory);
    }
    storage::write_state(paths, &result)?;
    Ok(public_state(&result))
}
fn stage(
    paths: Paths<'_>,
    state: &mut Value,
    issues: &[Value],
    trust: bool,
    name: &str,
) -> Result<(), Error> {
    state["public"] = status("running", issues.to_vec(), trust, Some(name), None);
    checkpoint(paths, state)
}

fn invalidate_cache(state: &mut Value) {
    if let Some(fields) = state.as_object_mut() {
        for key in [
            "source_inventory",
            "destination_inventory",
            "plugins_checked_at",
            "plugins_attempted_at",
        ] {
            fields.remove(key);
        }
    }
}

fn checkpoint(paths: Paths<'_>, state: &mut Value) -> Result<(), Error> {
    // O manifesto acompanha efeitos confirmados; a assinatura só volta após a verificação final.
    invalidate_cache(state);
    storage::write_state(paths, state)
}
