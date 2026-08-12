use futures::StreamExt;
use rig::tool::{rmcp::McpClientHandler, server::ToolServer};
use rig::{
  AgentBuilder,
  agent::{MultiTurnStreamItem, Text},
  client::{CompletionClient, ProviderClient},
  completion::CompletionModel,
  providers::{chatgpt, gemini, groq, openai, openrouter},
  streaming::{StreamedAssistantContent, StreamingPrompt},
  tool::ToolExecutionError,
};
use rmcp::{model::ClientInfo, transport::TokioChildProcess};
use serde::{Deserialize, Serialize};
use std::{
  env, fs,
  io::{self, Write},
  path::PathBuf,
};

use super::{load, secret};

/// Concrete completion model selected by `[ai].provider` and `[ai].model`.
pub enum SelectedModel {
  OpenAi(openai::responses_api::ResponsesCompletionModel),
  ChatGpt(chatgpt::ResponsesCompletionModel),
  Gemini(gemini::CompletionModel),
  Groq(groq::CompletionModel),
  OpenRouter(openrouter::CompletionModel),
}

impl SelectedModel {
  pub fn provider(&self) -> &'static str {
    match self {
      Self::OpenAi(model) => {
        let _ = model;
        "openai"
      }
      Self::ChatGpt(model) => {
        let _ = model;
        "chatgpt"
      }
      Self::Gemini(model) => {
        let _ = model;
        "gemini"
      }
      Self::Groq(model) => {
        let _ = model;
        "groq"
      }
      Self::OpenRouter(model) => {
        let _ = model;
        "openrouter"
      }
    }
  }
}

pub struct ModelSelection {
  pub provider: String,
  pub model: String,
  pub completion: SelectedModel,
}

#[derive(Serialize)]
struct ModelList<'a> {
  provider: &'a str,
  selected: &'a str,
  models: Vec<ModelPreset<'a>>,
}

#[derive(Serialize)]
struct ModelPreset<'a> {
  preset: &'static str,
  id: &'a str,
}

#[rig::rig_tool(
  name = "web_search",
  description = "Search the web for current information. Returns source URLs and short snippets.",
  required(query)
)]
async fn web_search(
  /// Search query
  query: String,
) -> Result<String, ToolExecutionError> {
  let client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(15))
    .user_agent("seam/1.0 web-search")
    .build()
    .map_err(|error| ToolExecutionError::other(format!("web search setup failed: {error}")))?;
  let response = client
    .get("https://html.duckduckgo.com/html/")
    .query(&[("q", query.as_str())])
    .send()
    .await
    .map_err(|error| ToolExecutionError::other(format!("web search failed: {error}")))?
    .error_for_status()
    .map_err(|error| ToolExecutionError::other(format!("web search failed: {error}")))?
    .text()
    .await
    .map_err(|error| {
      ToolExecutionError::other(format!("could not read search results: {error}"))
    })?;

  let results = parse_duckduckgo_html(&response, 8);

  if results.is_empty() {
    Err(ToolExecutionError::other(format!(
      "search returned no parseable results for: {query}"
    )))
  } else {
    Ok(results.join("\n\n"))
  }
}

fn parse_duckduckgo_html(html: &str, limit: usize) -> Vec<String> {
  html
    .split("class=\"result results_links")
    .skip(1)
    .filter_map(|result| {
      let anchor_marker = result.find("result__a")?;
      let anchor_start = result[..anchor_marker].rfind("<a")?;
      let anchor = &result[anchor_start..];
      let tag_end = anchor.find('>')?;
      let href = html_attribute(&anchor[..tag_end], "href")?;
      let title_end = anchor[tag_end + 1..].find("</a>")? + tag_end + 1;
      let title = strip_html(&anchor[tag_end + 1..title_end]);
      let url = duckduckgo_destination(&href);
      let description = result
        .find("result__snippet")
        .and_then(|start| result[start..].find('>').map(|offset| start + offset + 1))
        .and_then(|start| {
          result[start..]
            .find("</a>")
            .map(|end| &result[start..start + end])
        })
        .map(strip_html)
        .unwrap_or_default();
      Some(format!("{title}\n{description}\n{url}"))
    })
    .take(limit)
    .collect()
}

fn html_attribute(tag: &str, name: &str) -> Option<String> {
  let marker = format!("{name}=\"");
  let value = tag.split_once(&marker)?.1.split_once('"')?.0;
  Some(decode_html(value))
}

fn duckduckgo_destination(href: &str) -> String {
  let absolute = if href.starts_with("//") {
    format!("https:{href}")
  } else {
    href.to_owned()
  };
  url::Url::parse(&absolute)
    .ok()
    .and_then(|url| {
      url
        .query_pairs()
        .find(|(name, _)| name == "uddg")
        .map(|(_, value)| value.into_owned())
    })
    .unwrap_or(absolute)
}

fn strip_html(input: &str) -> String {
  let mut output = String::new();
  let mut in_tag = false;
  for character in input.chars() {
    match character {
      '<' => in_tag = true,
      '>' => in_tag = false,
      _ if !in_tag => output.push(character),
      _ => {}
    }
  }
  decode_html(output.trim())
}

fn decode_html(value: &str) -> String {
  value
    .replace("&amp;", "&")
    .replace("&quot;", "\"")
    .replace("&#x27;", "'")
    .replace("&#39;", "'")
    .replace("&lt;", "<")
    .replace("&gt;", ">")
}

/// Return the configured provider's portable model presets as formatted JSON.
pub fn list_models_json(config: &load::AiConfig) -> Result<String, Box<dyn std::error::Error>> {
  let models = ["fast", "balanced", "smart", "coding"]
    .into_iter()
    .map(|preset| {
      Ok(ModelPreset {
        preset,
        id: resolve_model(&config.provider, preset)?,
      })
    })
    .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

  Ok(serde_json::to_string_pretty(&ModelList {
    provider: &config.provider,
    selected: &config.model,
    models,
  })?)
}

/// Load config and credentials, then construct the configured Rig model.
pub fn select_model() -> Result<ModelSelection, Box<dyn std::error::Error>> {
  let config = load::load_ai()?;
  if config.provider == "chatgpt" {
    return select_chatgpt_model(config);
  }
  let stored_credential = secret::load_ai_credential(&config)?;
  let credential = normalize_credential(&stored_credential)?;
  select_model_with(config, credential)
}

#[derive(Deserialize)]
struct CodexAuth {
  tokens: CodexTokens,
}

#[derive(Deserialize)]
struct CodexTokens {
  access_token: String,
  account_id: Option<String>,
}

fn select_chatgpt_model(
  config: load::AiConfig,
) -> Result<ModelSelection, Box<dyn std::error::Error>> {
  let home = env::var_os("HOME")
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
  let auth_path = env::var_os("CODEX_HOME")
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from(home).join(".codex"))
    .join("auth.json");
  let auth: CodexAuth = serde_json::from_slice(&fs::read(&auth_path).map_err(|error| {
    io::Error::new(
      error.kind(),
      format!(
        "could not read Codex credentials at {}: {error}; sign in with Codex first",
        auth_path.display()
      ),
    )
  })?)?;
  let access_token = normalize_credential(&auth.tokens.access_token)?;
  let model = resolve_model("chatgpt", &config.model)?.to_owned();
  let client = chatgpt::Client::from_val(chatgpt::ChatGPTAuth::AccessToken {
    access_token,
    account_id: auth.tokens.account_id,
  })?;

  Ok(ModelSelection {
    provider: config.provider,
    completion: SelectedModel::ChatGpt(client.completion_model(&model)),
    model,
  })
}

fn normalize_credential(stored: &str) -> Result<String, Box<dyn std::error::Error>> {
  let trimmed = stored.trim();
  let unquoted = if trimmed.len() >= 2
    && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
      || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
  {
    &trimmed[1..trimmed.len() - 1]
  } else {
    trimmed
  };

  if unquoted.is_empty() {
    return Err("the stored AI credential is empty; run `seam set <provider>` again".into());
  }
  if unquoted.chars().any(char::is_whitespace) {
    return Err(
      "the stored AI credential contains whitespace; run `seam set <provider>` and paste only the key"
        .into(),
    );
  }

  Ok(unquoted.to_owned())
}

/// Send one prompt to the configured model and print chunks as they arrive.
pub async fn stream_message(prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
  let selected = select_model()?;

  match selected.completion {
    SelectedModel::OpenAi(model) => stream_model(model, prompt).await?,
    SelectedModel::ChatGpt(model) => stream_model(model, prompt).await?,
    SelectedModel::Gemini(model) => stream_model(model, prompt).await?,
    SelectedModel::Groq(model) => stream_model(model, prompt).await?,
    SelectedModel::OpenRouter(model) => stream_model(model, prompt).await?,
  }

  Ok(())
}

async fn stream_model<M>(model: M, prompt: &str) -> Result<(), Box<dyn std::error::Error>>
where
  M: CompletionModel + 'static,
{
  let tool_server = ToolServer::new().tool(WebSearch).run();
  let mut mcp_connections = Vec::new();
  for (name, server) in load::load_mcp()? {
    if server.command.trim().is_empty() {
      return Err(format!("MCP server '{name}' has an empty command").into());
    }
    let mut command = tokio::process::Command::new(&server.command);
    command.args(&server.args).envs(&server.env);
    let transport = TokioChildProcess::new(command)
      .map_err(|error| format!("could not start MCP server '{name}': {error}"))?;
    let connection = McpClientHandler::new(ClientInfo::default(), tool_server.clone())
      .connect(transport)
      .await
      .map_err(|error| format!("could not connect to MCP server '{name}': {error}"))?;
    mcp_connections.push(connection);
  }

  let agent = AgentBuilder::new(model)
    .preamble(
      "You have live internet access through the web_search tool. Never claim that you cannot browse or search. Call web_search whenever the user asks you to search, browse, look up, verify, or provide current information. Use web_search at most twice per user message. After receiving search results, do not keep retrying: synthesize the best supported answer, clearly note weak or irrelevant results, and cite useful returned URLs.",
    )
    .tool_server_handle(tool_server)
    .build();
  // Two searches require three model calls: search, optional refined search,
  // and a final answer. Keep extra room for provider continuations.
  let mut stream = agent.stream_prompt(prompt).max_turns(8).await;
  let mut stdout = io::stdout().lock();

  while let Some(item) = stream.next().await {
    if let MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(Text {
      text,
      ..
    })) = item?
    {
      write!(stdout, "{text}")?;
      stdout.flush()?;
    }
  }
  writeln!(stdout)?;
  drop(mcp_connections);
  Ok(())
}

fn select_model_with(
  config: load::AiConfig,
  credential: String,
) -> Result<ModelSelection, Box<dyn std::error::Error>> {
  let model = resolve_model(&config.provider, &config.model)?;
  let completion = match config.provider.as_str() {
    "openai" => {
      let client = openai::Client::from_val(credential.into())?;
      SelectedModel::OpenAi(client.completion_model(model))
    }
    "chatgpt" => return Err("ChatGPT must use the existing Codex OAuth credential".into()),
    "gemini" => {
      let client = gemini::Client::from_val(credential.into())?;
      SelectedModel::Gemini(client.completion_model(model))
    }
    "groq" => {
      let client = groq::Client::from_val(credential)?;
      SelectedModel::Groq(client.completion_model(model))
    }
    "openrouter" => {
      let client = openrouter::Client::from_val(credential.into())?;
      SelectedModel::OpenRouter(client.completion_model(model))
    }
    provider => return Err(format!("unsupported AI provider '{provider}'").into()),
  };

  Ok(ModelSelection {
    provider: config.provider,
    model: model.to_owned(),
    completion,
  })
}

/// Resolve a portable preset, or pass through an exact provider model ID.
pub fn resolve_model<'a>(
  provider: &str,
  configured: &'a str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
  let preset = match (provider, configured) {
    ("openai", "fast") => "gpt-5-nano",
    ("openai", "balanced") => "gpt-5-mini",
    ("openai", "smart") => "gpt-5.6",
    ("openai", "coding") => "gpt-5.6-sol",
    ("chatgpt", "fast") => "gpt-4o",
    ("chatgpt", "balanced") => "gpt-5.4",
    ("chatgpt", "smart" | "coding") => "gpt-5.5",
    ("gemini", "fast") => "gemini-3.1-flash-lite-preview",
    ("gemini", "balanced") => "gemini-2.5-flash",
    ("gemini", "smart" | "coding") => "gemini-2.5-pro-preview-06-05",
    ("groq", "fast") => "llama-3.1-8b-instant",
    ("groq", "balanced") => "llama-3.2-70b-versatile",
    ("groq", "smart" | "coding") => "deepseek-r1-distill-llama-70b",
    ("openrouter", "fast") => "google/gemini-2.0-flash-001",
    ("openrouter", "balanced") => "qwen/qwq-32b",
    ("openrouter", "smart") => "anthropic/claude-3.7-sonnet",
    ("openrouter", "coding") => "qwen/qwq-32b",
    (_, "fast" | "balanced" | "smart" | "coding") => {
      return Err(format!("preset '{configured}' is unavailable for provider '{provider}'").into());
    }
    (_, exact) => exact,
  };
  Ok(preset)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn resolves_presets_and_preserves_exact_ids() {
    assert_eq!(resolve_model("openai", "fast").unwrap(), "gpt-5-nano");
    assert_eq!(resolve_model("chatgpt", "coding").unwrap(), "gpt-5.5");
    assert_eq!(
      resolve_model("gemini", "custom-model").unwrap(),
      "custom-model"
    );
  }

  #[test]
  fn normalizes_pasted_credentials() {
    assert_eq!(normalize_credential("  AIza-test\n").unwrap(), "AIza-test");
    assert_eq!(normalize_credential("\"AIza-test\"").unwrap(), "AIza-test");
    assert_eq!(normalize_credential("'AIza-test'").unwrap(), "AIza-test");
    assert!(normalize_credential("AIza bad").is_err());
  }

  #[test]
  fn lists_models_as_json() {
    let config = load::AiConfig {
      provider: "chatgpt".to_owned(),
      model: "balanced".to_owned(),
    };
    let value: serde_json::Value =
      serde_json::from_str(&list_models_json(&config).unwrap()).unwrap();

    assert_eq!(value["provider"], "chatgpt");
    assert_eq!(value["selected"], "balanced");
    assert_eq!(value["models"][0]["preset"], "fast");
    assert_eq!(value["models"][1]["id"], "gpt-5.4");
  }

  #[test]
  fn parses_duckduckgo_html_results() {
    let html = r#"<div class="result results_links"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F">Rust &amp; Cargo</a><a class="result__snippet">Official <b>site</b></a></div>"#;
    let results = parse_duckduckgo_html(html, 8);

    assert_eq!(
      results,
      vec!["Rust & Cargo\nOfficial site\nhttps://www.rust-lang.org/"]
    );
  }
}
