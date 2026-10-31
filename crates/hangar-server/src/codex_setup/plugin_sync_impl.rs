use super::{Actions, Paths};
use crate::codex_setup::{
    files, instructions_toml, messages,
    phase::{Error, State},
    plugin_selection as selection,
};
use crate::orchestration::codec::clone_value;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn error(kind: &'static str, message: impl Into<String>) -> Error {
    Error {
        kind,
        message: message.into(),
    }
}
fn selected(e: selection::Error) -> Error {
    error(e.kind, e.message)
}
fn object(v: &Value) -> Result<&serde_json::Map<String, Value>, Error> {
    v.as_object().ok_or_else(|| {
        error(
            "AttributeError",
            "Objeto inválido na reconciliação de plugins",
        )
    })
}
fn field<'a>(v: &'a Value, k: &str) -> Result<&'a Value, Error> {
    object(v)?
        .get(k)
        .ok_or_else(|| error("KeyError", format!("'{k}'")))
}
fn text(v: &Value) -> Result<&str, Error> {
    v.as_str()
        .ok_or_else(|| error("TypeError", "Identidade de plugin precisa ser texto"))
}
fn set(v: Option<&Value>) -> Result<BTreeSet<String>, Error> {
    match v {
        None => Ok(BTreeSet::new()),
        Some(Value::Array(v)) => v.iter().map(|v| text(v).map(str::to_owned)).collect(),
        _ => Err(error(
            "TypeError",
            "Seleção de plugins precisa ser uma lista",
        )),
    }
}
fn message(code: &str, params: Value) -> Value {
    messages::message(code, params.as_object().expect("Parâmetros internos"))
        .expect("Mensagem versionada")
}
fn stage(state: &State, now: f64, code: &str, params: Value) {
    state.update(|s| {
        s["etapa"] = message(code, params);
        s["etapa_desde"] = json!(now);
    });
}
fn report(state: &State, code: &str, params: Value) {
    state.update(|s| {
        s["erros"]
            .as_array_mut()
            .expect("Estado inicializado")
            .push(message(code, params))
    });
}
fn progress(state: &State, at: usize, total: usize) {
    state.update(|s| {
        if crate::orchestration::truthy(&s["progresso"]) {
            s["progresso"]["sub"] = json!({"atual":at,"total":total});
        }
    });
}
fn caught(e: &Error, key: bool) -> bool {
    matches!(
        e.kind,
        "OSError" | "ValueError" | "RuntimeError" | "CodexNativoErro"
    ) || key && e.kind == "KeyError"
}
fn markets(paths: &Paths<'_>) -> Result<Value, Error> {
    let Some(raw) = files::read(&paths.codex.join("config.toml"))? else {return Ok(json!({}));};
    let raw = std::str::from_utf8(&raw).map_err(|e| error("UnicodeDecodeError",e.to_string()))?;
    let v = instructions_toml::parse(raw)?;
    Ok(v.get("marketplaces")
        .map(clone_value)
        .unwrap_or_else(|| json!({})))
}
fn inventory(items: Vec<Value>) -> Result<Value, Error> {
    let mut out = serde_json::Map::new();
    for item in items {
        out.insert(text(field(&item, "pluginId")?)?.into(), item);
    }
    Ok(Value::Object(out))
}
fn origin(id: &str, known: &Value, paths: &Paths<'_>) -> Result<selection::Origin, Error> {
    let market = id.rsplit_once('@').expect("Identidade validada").1;
    selection::origin(known.get(market).unwrap_or(&json!({})), true, paths.home)
        .map_err(selected)?
        .ok_or_else(|| error("ValueError", "Origem do marketplace indisponível"))
}
fn timestamp(reg: &Value, key: &str, default: f64) -> Result<f64, Error> {
    match reg.get(key) {
        None => Ok(default),
        Some(Value::Bool(v)) => Ok(u8::from(*v) as f64),
        Some(v) => v
            .as_f64()
            .ok_or_else(|| error("TypeError", "Data de atualização inválida")),
    }
}
fn iso(value: f64) -> Result<Option<String>, Error> {
    crate::codex_setup::timestamp::iso(value).map_err(|failure| error(failure.kind, failure.message))
}

pub(super) async fn run(
    paths: &Paths<'_>,
    registry: &mut Value,
    state: &State,
    force: bool,
    actions: &mut impl Actions,
) -> Result<(), Error> {
    for attempt in 0..3 {
        let raw = files::read(&paths.home.join(".claude/settings.json"))?.ok_or_else(|| {
            error(
                "ValueError",
                "Fonte do Claude desapareceu durante a integração",
            )
        })?;
        let settings = crate::orchestration::body::parse(&raw)
            .map_err(|_| error("ValueError", "Fonte do Claude inválida"))?;
        let desired = selection::desired(&settings).map_err(selected)?;
        match reconcile(paths, registry, state, force, actions, &desired, &raw).await {
            Err(e) if e.kind == "AlteradoExternamente" && attempt < 2 => continue,
            result => return result,
        }
    }
    unreachable!()
}

async fn reconcile(
    paths: &Paths<'_>,
    reg: &mut Value,
    state: &State,
    force: bool,
    a: &mut impl Actions,
    desired: &BTreeSet<String>,
    source: &[u8],
) -> Result<(), Error> {
    let previous = reg
        .get("plugins")
        .map(clone_value)
        .unwrap_or_else(|| json!({}));
    object(&previous)?;
    let config = paths.codex.join("config.toml");
    if let Some(raw) = files::read(&config)? {
        files::backup(&config, &raw, paths.backups)?;
    }
    let known = files::object(&paths.home.join(".claude/plugins/known_marketplaces.json"))?;
    let mut market = markets(paths)?;
    let mut blocked = BTreeSet::new();
    for id in desired {
        let name = id.rsplit_once('@').unwrap().1;
        let source = selection::origin(known.get(name).unwrap_or(&json!({})), true, paths.home)
            .map_err(selected)?;
        let destination =
            selection::origin(market.get(name).unwrap_or(&json!({})), false, paths.home)
                .map_err(selected)?;
        if source.is_none() || object(&market)?.contains_key(name) && source != destination {
            blocked.insert(id.clone());
            report(
                state,
                "erro_marketplace_origem",
                json!({"marketplace":name}),
            );
        }
    }
    let mut candidates = desired
        .difference(&blocked)
        .cloned()
        .collect::<BTreeSet<_>>();
    stage(state, a.now(), "etapa_lendo_plugins", json!({}));
    let mut installed = inventory(a.installed().await?)?;
    let mut present = BTreeSet::new();
    for id in &candidates {
        match selection::identity(
            id,
            &origin(id, &known, paths)?,
            &market,
            &installed,
            paths.home,
        )
        .map_err(selected)
        {
            Ok(native) if object(&installed)?.contains_key(&native) => {
                present.insert(id.clone());
            }
            Ok(_) => {}
            Err(e) if e.kind == "ValueError" => {}
            Err(e) => return Err(e),
        }
    }
    let missing = candidates
        .difference(&present)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut items = vec![];
    let mut labels = vec![];
    if !missing.is_empty() {
        stage(state, a.now(), "etapa_detectando", json!({}));
        for mut item in a.detect().await? {
            if item.get("itemType").and_then(Value::as_str) != Some("PLUGINS") {
                continue;
            }
            let mut groups = vec![];
            if let Some(raw) = item["details"].get("plugins") {
                for group in raw
                    .as_array()
                    .ok_or_else(|| error("TypeError", "Grupos de plugins inválidos"))?
                {
                    let name = group
                        .get("marketplaceName")
                        .map(crate::orchestration::codec::python_string)
                        .unwrap_or_else(|| "None".into());
                    let mut names = vec![];
                    if let Some(raw) = group.get("pluginNames") {
                        for plugin in raw
                            .as_array()
                            .ok_or_else(|| error("TypeError", "Nomes de plugins inválidos"))?
                        {
                            let label = format!(
                                "{}@{name}",
                                crate::orchestration::codec::python_string(plugin)
                            );
                            if missing.contains(&label) {
                                names.push(clone_value(plugin));
                                labels.push(label);
                            }
                        }
                    }
                    if !names.is_empty() {
                        let mut group = clone_value(group);
                        group["pluginNames"] = json!(names);
                        groups.push(group);
                    }
                }
            }
            if !groups.is_empty() {
                item["details"]["plugins"] = json!(groups);
                items.push(item);
            }
        }
    }
    if !items.is_empty() {
        stage(
            state,
            a.now(),
            "etapa_importando_plugins",
            json!({"plugins":labels.join(", ")}),
        );
        let result = a.import(items).await?;
        if let Some(types) = result.get("itemTypeResults") {
            for kind in types
                .as_array()
                .ok_or_else(|| error("TypeError", "Resultados de importação inválidos"))?
            {
                if crate::orchestration::truthy(&kind["failures"]) {
                    report(state, "erro_plugins_incompletos", json!({}));
                }
            }
        }
        market = markets(paths)?;
        installed = inventory(a.installed().await?)?;
    }
    let mut identities = BTreeMap::new();
    for id in &candidates {
        match selection::identity(
            id,
            &origin(id, &known, paths)?,
            &market,
            &installed,
            paths.home,
        )
        .map_err(selected)
        {
            Ok(native) => {
                identities.insert(id.clone(), native);
            }
            Err(e) if e.kind == "ValueError" => {
                blocked.insert(id.clone());
                report(state, "erro_plugin", json!({"id":id}));
            }
            Err(e) => return Err(e),
        }
    }
    candidates.retain(|id| !blocked.contains(id));
    let now = a.now();
    let last = timestamp(reg, "marketplaces_em", 0.0)?.max(timestamp(
        reg,
        "marketplaces_tentativa_em",
        0.0,
    )?);
    let old_failed = set(reg.get("marketplaces_pendentes"))?;
    let update = force || now - last >= 21600.0 || !old_failed.is_empty() && now - last >= 300.0;
    let targets = if update {
        candidates
            .iter()
            .map(|id| identities[id].rsplit_once('@').unwrap().1.to_owned())
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let total = targets.len() + candidates.len();
    let mut failed = BTreeSet::new();
    if update {
        for (i, name) in targets.iter().enumerate() {
            progress(state, i + 1, total);
            let source = markets(paths)?;
            if source[name]["source_type"].as_str() != Some("git") {
                continue;
            }
            stage(
                state,
                a.now(),
                "etapa_marketplace",
                json!({"marketplace":name}),
            );
            match a.upgrade(name).await {
                Ok(v) if !crate::orchestration::truthy(&v["errors"]) => {}
                Err(e) if !caught(&e, false) => return Err(e),
                _ => {
                    report(
                        state,
                        "erro_marketplace_atualizar",
                        json!({"marketplace":name}),
                    );
                    failed.insert(name.clone());
                }
            }
        }
        reg["marketplaces_tentativa_em"] = json!(now);
        reg["marketplaces_pendentes"] = json!(failed);
        if failed.is_empty() {
            reg["marketplaces_em"] = json!(now);
        }
        installed = inventory(a.installed().await?)?;
    } else {
        for name in old_failed {
            report(
                state,
                "erro_marketplace_pendente",
                json!({"marketplace":name}),
            );
        }
    }
    let mut pending = set(reg.get("plugins_pendentes"))?
        .intersection(desired)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut plugins = serde_json::Map::new();
    for id in &blocked {
        if let Some(v) = previous.get(id) {
            plugins.insert(id.clone(), clone_value(v));
        }
    }
    for (i, id) in candidates.iter().enumerate() {
        progress(state, targets.len() + i + 1, total);
        let result=async {
            let native=&identities[id];let old=previous.get(id);let live=installed.get(native);
            let reinstall=update||pending.contains(id)||old.is_none_or(|v|!crate::orchestration::truthy(v))||live.is_none_or(|v|!crate::orchestration::truthy(v))||old.unwrap().get("id_codex").and_then(Value::as_str).unwrap_or(id)!=native||live.unwrap().get("version")!=old.unwrap().get("versao")||!Path::new(old.unwrap().get("path").and_then(Value::as_str).unwrap_or("")).is_dir();
            let value=if reinstall{stage(state,a.now(),"etapa_plugin",json!({"id":id}));let data=a.add(native).await?;json!({"path":field(&data,"installedPath")?,"versao":data.get("version").unwrap_or(&json!("")),"origem":native.rsplit_once('@').unwrap().1,"id_codex":native})}else{clone_value(old.unwrap())};
            plugins.insert(id.clone(),clone_value(&value));
            if reg.get("plugins").is_none(){reg["plugins"]=json!({});}reg["plugins"][id]=clone_value(&value);
            a.hooks(Path::new(text(field(&value,"path")?)?)).await?;pending.remove(id);a.checkpoint(reg,state)?;Ok::<(),Error>(())
        }.await;
        if let Err(e) = result {
            if !caught(&e, true) {
                return Err(e);
            }
            report(state, "erro_plugin", json!({"id":id}));
            pending.insert(id.clone());
            if let Some(old) = previous.get(id) {
                plugins.insert(id.clone(), clone_value(old));
            }
        }
    }
    if files::read(&paths.home.join(".claude/settings.json"))?.as_deref() != Some(source) {
        return Err(error(
            "AlteradoExternamente",
            "Plugins do Claude mudaram durante a integração",
        ));
    }
    let active = plugins
        .iter()
        .map(|(id, v)| {
            v.get("id_codex")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    let removed = object(&previous)?
        .keys()
        .filter(|id| !desired.contains(*id))
        .collect::<Vec<_>>();
    if !removed.is_empty() {
        let choices = removed
            .into_iter()
            .map(|id| {
                previous[id]
                    .get("id_codex")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned()
            })
            .filter(|id| !active.contains(id))
            .map(|id| (id, false))
            .collect();
        a.edit(choices).await?;
    }
    a.edit(
        plugins
            .iter()
            .filter(|(id, _)| candidates.contains(*id))
            .map(|(id, v)| {
                (
                    v.get("id_codex")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_owned(),
                    true,
                )
            })
            .collect(),
    )
    .await?;
    state.set_confirmed(
        plugins
            .keys()
            .filter(|id| candidates.contains(*id))
            .cloned()
            .collect(),
    );
    let mut summary = vec![];
    for (id, v) in &plugins {
        summary.push(json!({"id":v.get("id_codex").unwrap_or(&json!(id)),"versao":field(v,"versao")?,"origem":field(v,"origem")?}));
    }
    reg["plugins"] = Value::Object(plugins);
    reg["plugins_pendentes"] = json!(pending);
    let next = iso(timestamp(reg, "marketplaces_em", 0.0)?.max(timestamp(
        reg,
        "marketplaces_tentativa_em",
        now,
    )?) + if crate::orchestration::truthy(&reg["marketplaces_pendentes"]) {
        300.0
    } else {
        21600.0
    })?;
    state.update(|s| {
        s["plugins"] = json!(summary);
        s["proxima_atualizacao"] = json!(next);
    });
    Ok(())
}
