//! Pareamento impresso no boot e no CLI, sem iniciar o serviço.
use super::dotenv::{SettingsEnvironment, SettingsSnapshot};
use std::io::{IsTerminal, Write};

pub struct Settings {
    pub public_url: String,
    pub bind: String,
    pub token: String,
    pub port: String,
    pub front_port: String,
}
impl Settings {
    pub fn from_snapshot(snapshot: &SettingsSnapshot) -> Result<Self, String> {
        let value = |key, default: &str| snapshot.value(key).map(|value|value.unwrap_or_else(||default.into())).map_err(|_|format!("pairing_config_invalid: {key}"));
        let port = |key, default, empty| -> Result<String,String> {
            let raw = value(key,default)?;
            let raw = if empty && raw.trim().is_empty() { "0".into() } else { raw };
            crate::value_coercion::integer(&serde_json::Value::String(raw))
                .map(|value|value.to_string()).ok_or_else(||format!("pairing_config_invalid: {key}"))
        };
        Ok(Self {
            public_url: value("CP_PUBLIC_URL", "")?,
            bind: value("CP_LAN_BIND_IP", "127.0.0.1")?,
            token: value("CP_AUTH_TOKEN", "change-me")?,
            port: port("CP_PORT", "8765", false)?,
            front_port: port("CP_FRONT_PORT", "0", true)?,
        })
    }
    pub fn urls(&self, lan_ip: &str) -> (String,String) {
        let host = if ["127.0.0.1","localhost","::1","0.0.0.0","auto"].contains(&self.bind.as_str()) {lan_ip} else {&self.bind};
        let api = format!("http://{host}:{}",self.port);
        let front_port = if self.front_port == "0" {&self.port} else {&self.front_port};
        let front = if self.public_url.is_empty() {format!("http://{host}:{front_port}")} else {self.public_url.trim_end_matches('/').into()};
        let pwa = format!("{front}/?token={}",token_query(&self.token));
        let native = if self.public_url.is_empty() {format!("{api}/?token={}",token_query(&self.token))} else {format!("{pwa}&api={api}")};
        (pwa,native)
    }
}

/// A senha no QR vai codificada: `+`, `&`, `%` ou `#` crus quebram a leitura no celular.
pub(crate) fn token_query(token: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~');
    percent_encoding::utf8_percent_encode(token, SET).to_string()
}
pub(crate) fn qr_text(url:&str)->Result<String,String> {
    let qr=qrcode::QrCode::new(url.as_bytes()).map_err(|_|"pairing_qr_invalid".to_owned())?;
    let width=qr.width();
    let mut result=String::new();
    for y in (0..width+2).step_by(2) {
        for x in 0..width+2 {
            let white=|y| y==0||x==0||y>width||x>width||qr[(x-1,y-1)]!=qrcode::Color::Dark;
            result.push(match (white(y),white(y+1)){(true,true)=>'█',(true,false)=>'▀',(false,true)=>'▄',_=>'\u{a0}'});
        }
        result.push('\n');
    }
    Ok(result)
}
pub fn print(settings:&Settings,lan_ip:&str,tty:bool,writer:&mut impl Write)->Result<(),String> {
    let (pwa,api)=settings.urls(lan_ip);
    let mut emit=|text:String| -> Result<(),String> {
        writer.write_all(text.as_bytes()).and_then(|_|writer.flush()).map_err(|e|e.to_string())
    };
    if tty {
        emit(format!("{}\n",qr_text(&pwa)?))?;
        emit(format!("  Scan to pair, or open: {pwa}\n\n"))?;
        emit(format!("{}\n",qr_text(&api)?))?;
    } else {
        emit(format!("  Pareamento (abra no celular): {pwa}\n\n"))?;
    }
    emit(format!("  App nativo: {api}\n\n"))
}
pub fn cli(args:&[String])->std::process::ExitCode {
    let backend=match args { []=>crate::session_open::cli::backend_dir(), [flag,path] if flag=="--backend"=>Some(std::path::PathBuf::from(path)), _=>{eprintln!("uso: hangar-server pairing [--backend <pasta>]");return 2.into();} };
    let environment=SettingsEnvironment::capture();
    let home=std::env::home_dir().unwrap_or_default();
    let result=SettingsSnapshot::capture(&home,backend.as_deref(),environment)
        .map_err(|error|format!("pairing_config_unavailable: {error}"))
        .and_then(|snapshot|Settings::from_snapshot(&snapshot))
        .and_then(|settings|print(&settings,&super::network::lan_ip(),std::io::stdout().is_terminal(),&mut std::io::stdout().lock()));
    match result {Ok(())=>0.into(),Err(error)=>{eprintln!("{error}");1.into()}}
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn pairing_urls_encode_the_password() {
        let local = Settings { public_url: String::new(), bind: "10.0.0.5".into(), token: "a+b&c%d#".into(), port: "8765".into(), front_port: "5173".into() };
        assert_eq!(local.urls("ignored"), ("http://10.0.0.5:5173/?token=a%2Bb%26c%25d%23".into(), "http://10.0.0.5:8765/?token=a%2Bb%26c%25d%23".into()));
        let public = Settings { public_url: "https://casa.ts.net/".into(), ..local };
        assert_eq!(public.urls("ignored").1, "https://casa.ts.net/?token=a%2Bb%26c%25d%23&api=http://10.0.0.5:8765");
    }
}
