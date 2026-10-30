//! Preparo das dependências de produção do plugin ECC.
use super::files;
use crate::{
    accounts::environment::AccountEnvironment,
    command_capture::{self, ByteOutput, CaptureFailure, CaptureOptions},
    orchestration::{codec, truthy},
};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};

pub trait Runner {
    fn resolve(&self, program: &str) -> Option<PathBuf>;
    fn run(
        &self,
        args: Vec<String>,
        budget: Duration,
        options: CaptureOptions,
    ) -> impl Future<Output = Result<ByteOutput, CaptureFailure>> + Send;
}
pub struct NativeRunner(pub AccountEnvironment);
impl Runner for NativeRunner {
    fn resolve(&self, program: &str) -> Option<PathBuf> {
        crate::cli_probe::resolve_program(program, &self.0)
    }
    async fn run(
        &self,
        args: Vec<String>,
        budget: Duration,
        options: CaptureOptions,
    ) -> Result<ByteOutput, CaptureFailure> {
        command_capture::capture_bytes(args, self.0.clone(), budget, options).await
    }
}
fn object(path: &Path) -> Result<Value, String> {
    files::object(path).map_err(|error| error.to_string())
}
const PROBE: &str = "const r=require('module').createRequire(require('path').join(process.argv[1],'package.json'));for(const n of JSON.parse(process.argv[2]))r(n)";
async fn healthy(
    root: &Path,
    node: &Path,
    expected: &[(String, String)],
    runner: &impl Runner,
) -> Result<bool, String> {
    for (name, version) in expected {
        if object(&root.join("node_modules").join(name).join("package.json"))?.get("version")
            != Some(&Value::String(version.clone()))
        {
            return Ok(false);
        }
    }
    let args = vec![
        node.to_string_lossy().into_owned(),
        "-e".into(),
        PROBE.into(),
        root.to_string_lossy().into_owned(),
        codec::dumps(
            &json!(expected.iter().map(|(name, _)| name).collect::<Vec<_>>()),
            false,
        ),
    ];
    match runner
        .run(
            args,
            Duration::from_secs(15),
            CaptureOptions {
                preserve_account: true,
                inherit_stdin: true,
                ..Default::default()
            },
        )
        .await
    {
        Ok(output) => Ok(output.success),
        Err(error) if error.kind == "timeout" => Ok(false),
        Err(error) => Err(error.kind),
    }
}
pub async fn prepare(root: &Path, backups: &Path, runner: &impl Runner) -> Result<bool, String> {
    let package = object(&root.join("package.json"))?;
    let dependencies = package.get("dependencies").unwrap_or(&Value::Null);
    if !truthy(dependencies) {
        return Ok(false);
    }
    let dependencies = dependencies
        .as_object()
        .ok_or("Dependências do ECC inválidas")?;
    let lock = object(&root.join("package-lock.json"))?;
    let empty = json!({});
    let locked = lock
        .get("packages")
        .unwrap_or(&empty)
        .as_object()
        .ok_or("Registro de dependências do ECC inválido")?;
    let name_pattern = regex::Regex::new(r"\A(?:@[a-z0-9._-]+/)?[a-z0-9._-]+\z").unwrap();
    let version_pattern = regex::Regex::new(r"\A\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?\z").unwrap();
    let mut expected = Vec::new();
    for (name, declared) in dependencies {
        if !name_pattern.is_match(name) {
            return Err("Nome de dependência do ECC inválido".into());
        }
        let entry = locked
            .get(&format!("node_modules/{name}"))
            .unwrap_or(&empty)
            .as_object()
            .ok_or("Registro de dependência do ECC inválido")?;
        let version = entry.get("version").unwrap_or(declared);
        let version = version
            .as_str()
            .filter(|_| !codec::contains_nonfinite(version))
            .filter(|text| version_pattern.is_match(text))
            .ok_or("Dependência do ECC sem versão fixada")?;
        expected.push((name.clone(), version.to_owned()));
    }
    let (Some(node), Some(npm)) = (runner.resolve("node"), runner.resolve("npm")) else {
        return Err("O ECC precisa de Node e npm para concluir o preparo".into());
    };
    if healthy(root, &node, &expected, runner).await? {
        return Ok(false);
    }
    let candidates = [
        std::fs::canonicalize(&npm).unwrap_or_else(|_| npm.clone()),
        npm.parent()
            .unwrap_or(Path::new("."))
            .join("node_modules/npm/bin/npm-cli.js"),
    ];
    let cli = candidates
        .into_iter()
        .find(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|text| text.to_str()),
                    Some("js" | "cjs" | "mjs")
                )
        })
        .ok_or("O executável JavaScript do npm não foi encontrado")?;
    let mut args = vec![
        node.to_string_lossy().into_owned(),
        cli.to_string_lossy().into_owned(),
        "install".into(),
        "--prefix".into(),
        root.to_string_lossy().into_owned(),
    ];
    args.extend(
        [
            "--omit=dev",
            "--ignore-scripts",
            "--no-bin-links",
            "--no-save",
            "--no-audit",
            "--no-fund",
        ]
        .map(String::from),
    );
    let output = match runner
        .run(
            args,
            Duration::from_secs(120),
            CaptureOptions {
                cwd: Some(root.to_owned()),
                preserve_account: true,
                ..Default::default()
            },
        )
        .await
    {
        Ok(output) => output,
        Err(mut error) if error.kind == "timeout" => {
            error.stderr.extend_from_slice(b"\nnpm deadline exceeded");
            ByteOutput {
                success: false,
                stdout: error.stdout,
                stderr: error.stderr,
            }
        }
        Err(error) => return Err(error.kind),
    };
    if !output.success || !healthy(root, &node, &expected, runner).await? {
        let diagnostic = backups.join("ecc-runtime-npm.log");
        let prior = files::read(&diagnostic).map_err(|error| error.to_string())?;
        let mut bytes = output.stderr;
        bytes.extend(output.stdout);
        files::write(&diagnostic, &bytes, prior.as_deref(), None)
            .map_err(|error| error.to_string())?;
        return Err("As dependências do ECC não foram confirmadas".into());
    }
    Ok(true)
}
