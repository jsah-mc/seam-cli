use serde::Deserialize;
use std::{collections::BTreeMap, env, fs, io, path::PathBuf};

use super::colors::TemplateJob;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
  #[serde(default)]
  templates: BTreeMap<String, TemplateEntry>,

  #[serde(default)]
  pub ai: AiConfig,

  #[serde(default)]
  pub mcp: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
  pub command: String,
  #[serde(default)]
  pub args: Vec<String>,
  #[serde(default)]
  pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AiConfig {
  pub provider: String,
  pub model: String,
}

impl Default for AiConfig {
  fn default() -> Self {
    Self {
      provider: "openai".to_owned(),
      model: "gpt-4.1-mini".to_owned(),
    }
  }
}

#[derive(Debug, Clone, Deserialize)]
struct TemplateEntry {
  input_path: PathBuf,
  output_path: PathBuf,
}

pub fn load_templates(
  input: Option<PathBuf>,
  output: Option<PathBuf>,
) -> Result<Vec<TemplateJob>, Box<dyn std::error::Error>> {
  if let Some(input) = input {
    return Ok(vec![TemplateJob { input, output }]);
  }

  let Some((config, home)) = load_config_with_home()? else {
    return Ok(Vec::new());
  };
  Ok(
    config
      .templates
      .into_values()
      .map(|entry| TemplateJob {
        input: expand_home(entry.input_path, &home),
        output: Some(expand_home(entry.output_path, &home)),
      })
      .collect(),
  )
}

pub fn load_ai() -> Result<AiConfig, Box<dyn std::error::Error>> {
  let ai = load_config_with_home()?
    .map(|(config, _)| config.ai)
    .unwrap_or_default();
  validate_ai(&ai)?;
  Ok(ai)
}

pub fn load_mcp() -> Result<BTreeMap<String, McpServerConfig>, Box<dyn std::error::Error>> {
  Ok(
    load_config_with_home()?
      .map(|(config, _)| config.mcp)
      .unwrap_or_default(),
  )
}

fn load_config_with_home()
-> Result<Option<(Config, std::ffi::OsString)>, Box<dyn std::error::Error>> {
  let home = env::var_os("HOME")
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
  let config_path = PathBuf::from(&home).join(".config/seam/config.toml");
  if !config_path.exists() {
    return Ok(None);
  }
  let config = toml::from_str(&fs::read_to_string(config_path)?)?;
  Ok(Some((config, home)))
}

fn validate_ai(ai: &AiConfig) -> io::Result<()> {
  if ai.model.trim().is_empty() {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      "ai.model cannot be empty",
    ));
  }
  match ai.provider.as_str() {
    "openai" | "chatgpt" | "gemini" | "groq" | "openrouter" => Ok(()),
    provider => Err(io::Error::new(
      io::ErrorKind::InvalidData,
      format!(
        "unsupported AI provider '{provider}'; use openai, chatgpt, gemini, groq, or openrouter"
      ),
    )),
  }
}

fn expand_home(path: PathBuf, home: &std::ffi::OsStr) -> PathBuf {
  let text = path.to_string_lossy();
  if text == "~" {
    PathBuf::from(home)
  } else if let Some(rest) = text.strip_prefix("~/") {
    PathBuf::from(home).join(rest)
  } else {
    path
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_ai_provider_and_model() {
    let config: Config = toml::from_str(
      r#"
        [ai]
        provider = "gemini"
        model = "gemini-2.5-flash"
      "#,
    )
    .unwrap();

    assert_eq!(config.ai.provider, "gemini");
    assert_eq!(config.ai.model, "gemini-2.5-flash");
    validate_ai(&config.ai).unwrap();
  }

  #[test]
  fn defaults_ai_when_section_is_missing() {
    let config: Config = toml::from_str("").unwrap();
    assert_eq!(config.ai.provider, "openai");
    assert_eq!(config.ai.model, "gpt-4.1-mini");
  }

  #[test]
  fn parses_stdio_mcp_servers() {
    let config: Config = toml::from_str(
      r#"
        [mcp.filesystem]
        command = "npx"
        args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

        [mcp.filesystem.env]
        LOG_LEVEL = "error"
      "#,
    )
    .unwrap();
    let server = &config.mcp["filesystem"];

    assert_eq!(server.command, "npx");
    assert_eq!(server.args[2], "/tmp");
    assert_eq!(server.env["LOG_LEVEL"], "error");
  }
}
