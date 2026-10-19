use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

pub const KEY_ENV: &str = "DEEPSEEK_API_KEY";

pub fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("cannot locate the platform config directory")?
        .join("seekfetch");
    Ok(dir.join("credentials"))
}

pub fn set_key(stdin: bool, from_env: Option<&str>) -> Result<()> {
    let key = if let Some(name) = from_env {
        std::env::var(name).with_context(|| format!("environment variable {name} is not set"))?
    } else if stdin || !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read the API key from stdin")?;
        buf
    } else {
        rpassword::prompt_password("DeepSeek API key: ").context("failed to read the API key")?
    };

    let key = normalize_key(&key)?;
    let _ = key;

    // TODO: encrypt with a machine-bound key and write to `path()` with 0600.
    bail!("credential storage is not implemented yet")
}

pub fn import_dotenv(dotenv: &Path, name: &str) -> Result<()> {
    let text = std::fs::read_to_string(dotenv)
        .with_context(|| format!("failed to read {}", dotenv.display()))?;

    let value = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then(|| value.to_string())
        })
        .next()
        .with_context(|| format!("{name} not found in {}", dotenv.display()))?;

    let key = normalize_key(&value)?;
    let _ = key;

    // TODO: encrypt and persist through the same path as `set_key`.
    bail!("credential storage is not implemented yet")
}

pub fn show() -> Result<()> {
    let path = path()?;
    println!("credentials: {}", path.display());
    match load_key() {
        Ok(key) => println!("api key: {}", mask_key(&key)),
        Err(err) => println!("api key: {err}"),
    }
    Ok(())
}

pub fn clear() -> Result<()> {
    let path = path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        println!("removed {}", path.display());
    } else {
        println!("no credential file at {}", path.display());
    }
    Ok(())
}

pub fn load_key() -> Result<String> {
    if let Ok(key) = std::env::var(KEY_ENV) {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }

    // TODO: read and decrypt the credential file before falling back to an error.
    bail!("no API key configured; run `seekfetch config set-key`")
}

fn normalize_key(raw: &str) -> Result<String> {
    let key = raw.trim();
    if key.is_empty() {
        bail!("API key is empty");
    }
    if key.contains(['\r', '\n']) {
        bail!("API key must be a single line");
    }
    Ok(key.to_string())
}

fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("****{tail}")
}
