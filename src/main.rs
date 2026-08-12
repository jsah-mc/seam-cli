use clap::{Parser, Subcommand};
use std::{
  env, fs, io,
  io::{IsTerminal, Read},
  path::{Path, PathBuf},
};
mod utils {
  pub mod ai;
  pub mod colors;
  pub mod load;
  pub mod secret;
}

#[derive(Parser)]
#[command(
  name = "seam-cli",
  version = "1.0.0",
  about = "Material You Theme Engine CLI"
)]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(Subcommand)]
enum Commands {
  /// Inspect or message the configured AI model
  Ai {
    #[command(subcommand)]
    command: Option<AiCommands>,
  },

  /// Store an API key for a provider (ChatGPT uses Codex OAuth)
  Set {
    #[arg(value_parser = ["openai", "gemini", "groq", "openrouter"])]
    provider: String,
  },

  /// Set a wallpaper and generate a dynamic Material 3 theme
  Wall {
    #[command(subcommand)]
    command: WallCommands,
  },

  /// Select a bundled Base16 color scheme
  Scheme {
    #[command(subcommand)]
    command: SchemeCommands,
  },

  /// Manage the configured AI provider's credential
  Secret {
    #[command(subcommand)]
    command: SecretCommands,
  },

  /// Extract a theme palette from an image file
  Generate {
    #[arg(short, long, help = "Path to your desktop wallpaper image")]
    image: PathBuf,

    #[arg(
      long,
      value_name = "FILE",
      help = "Use a Base16 YAML palette for all color variables"
    )]
    base16: Option<PathBuf>,

    #[arg(
      long,
      value_name = "NAME",
      conflicts_with = "base16",
      help = "Use a scheme: dynamic-tonal-spot, catppuccin, gruvbox, rosepine, or tokyonight"
    )]
    scheme: Option<String>,

    #[arg(
      short,
      long,
      help = "Render a Tera template using {{ color.format }} values"
    )]
    template: Option<PathBuf>,

    #[arg(
      short,
      long,
      requires = "template",
      help = "Write the rendered template to this file instead of stdout"
    )]
    output: Option<PathBuf>,

    #[arg(long, help = "Use the light scheme when rendering a template")]
    light: bool,
  },
}

#[derive(Subcommand)]
enum AiCommands {
  /// Send a message and stream the response to the terminal
  Msg { text: String },
  /// Inspect models available for the configured provider
  Models {
    #[command(subcommand)]
    command: AiModelsCommands,
  },
}

#[derive(Subcommand)]
enum AiModelsCommands {
  /// Print preset model mappings as JSON
  List,
}

#[derive(Subcommand)]
enum WallCommands {
  Set {
    wallpaper: PathBuf,

    #[arg(
      long = "type",
      default_value = "tonal-spot",
      value_parser = [
        "auto",
        "tonal-spot",
        "content",
        "expressive",
        "fidelity",
        "fruit-salad",
        "monochrome",
        "neutral",
        "rainbow",
        "vibrant"
      ]
    )]
    scheme_type: String,

    #[arg(long)]
    light: bool,
  },
}

#[derive(Subcommand)]
enum SchemeCommands {
  Set {
    scheme: String,

    #[arg(long)]
    light: bool,
  },
}

#[derive(Subcommand)]
enum SecretCommands {
  /// Read an API key or OAuth token from stdin and save it to the OS keyring
  Set,
  /// Check whether a credential is stored without displaying it
  Status,
  /// Remove the stored API key
  Remove,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  // Load and validate the configured provider/model once for all commands.
  let ai = utils::load::load_ai()?;
  let cli = Cli::parse();

  match cli.command {
    Commands::Ai { command } => match command {
      Some(AiCommands::Msg { text }) => utils::ai::stream_message(&text).await?,
      Some(AiCommands::Models {
        command: AiModelsCommands::List,
      }) => println!("{}", utils::ai::list_models_json(&ai)?),
      None => {
        let selected = utils::ai::select_model()?;
        println!("Provider: {}", selected.completion.provider());
        println!("Model: {}", selected.model);
        debug_assert_eq!(selected.provider, selected.completion.provider());
      }
    },
    Commands::Set { provider } => {
      let key = read_secret(&format!("Enter API key for {provider}: "))?;
      utils::secret::SecretManager::new(&provider)?.save_key(&key)?;
      println!("Saved API key for {provider}");
    }
    Commands::Secret { command } => {
      let manager = utils::secret::SecretManager::for_ai(&ai)?;
      match command {
        SecretCommands::Set => {
          let key = read_secret(&format!(
            "Enter {} for {}: ",
            credential_name(&ai.provider),
            ai.provider
          ))?;
          manager.save_key(&key)?;
          println!(
            "Saved {} for {}",
            credential_name(&ai.provider),
            ai.provider
          );
        }
        SecretCommands::Status => match utils::secret::load_ai_credential(&ai) {
          Ok(_) => println!(
            "{} for {} is configured",
            credential_name(&ai.provider),
            ai.provider
          ),
          Err(keyring::Error::NoEntry) => {
            println!(
              "No {} configured for {}",
              credential_name(&ai.provider),
              ai.provider
            )
          }
          Err(error) => return Err(error.into()),
        },
        SecretCommands::Remove => {
          manager.delete_key()?;
          println!(
            "Removed {} for {}",
            credential_name(&ai.provider),
            ai.provider
          );
        }
      }
    }
    Commands::Wall {
      command: WallCommands::Set {
        wallpaper,
        scheme_type,
        light,
      },
    } => {
      save_wallpaper(&wallpaper)?;
      let templates = utils::load::load_templates(None, None)?;
      utils::colors::generate(
        wallpaper,
        None,
        Some(format!("dynamic-{scheme_type}")),
        templates,
        light,
      )?
    }
    Commands::Scheme {
      command: SchemeCommands::Set { scheme, light },
    } => {
      let templates = utils::load::load_templates(None, None)?;
      utils::colors::generate(PathBuf::new(), None, Some(scheme), templates, light)?
    }
    Commands::Generate {
      image,
      base16,
      scheme,
      template,
      output,
      light,
    } => {
      let templates = utils::load::load_templates(template, output)?;
      utils::colors::generate(image, base16, scheme, templates, light)?
    }
  }
  Ok(())
}

fn credential_name(provider: &str) -> &'static str {
  if provider == "chatgpt" {
    "OAuth token"
  } else {
    "API key"
  }
}

fn read_secret(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
  let value = if io::stdin().is_terminal() {
    rpassword::prompt_password(prompt)?
  } else {
    let mut value = String::new();
    io::stdin().read_to_string(&mut value)?;
    value
  };
  let value = value.trim().to_owned();
  if value.is_empty() {
    return Err(io::Error::new(io::ErrorKind::InvalidInput, "credential cannot be empty").into());
  }
  Ok(value)
}

fn save_wallpaper(wallpaper: &Path) -> io::Result<()> {
  let state_home = env::var_os("XDG_STATE_HOME")
    .map(PathBuf::from)
    .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
  let directory = state_home.join("seam");
  fs::create_dir_all(&directory)?;
  fs::write(
    directory.join("wallpaper.txt"),
    format!("{}\n", wallpaper.display()),
  )
}
