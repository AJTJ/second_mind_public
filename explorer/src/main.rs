mod agent;
mod api;
mod review;
mod tools;
mod util;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::agent::AgentConfig;
use crate::review::{ReviewStatus, ReviewStore};

#[derive(Parser)]
#[command(
    name = "explorer",
    about = "Autonomous research agent for the Learning Engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a research directive
    Run {
        /// The research directive
        directive: String,
        /// Run in async mode (write to review queue, don't prompt)
        #[arg(long, short)]
        r#async: bool,
    },
    /// List pending reviews
    Review,
    /// Approve a review entry
    Approve {
        /// Review entry ID
        id: String,
    },
    /// Reject a review entry
    Reject {
        /// Review entry ID
        id: String,
    },
    /// View a review entry's raw output
    View {
        /// Review entry ID
        id: String,
    },
}

fn build_config() -> anyhow::Result<AgentConfig> {
    Ok(AgentConfig {
        anthropic_api_key: require_env("ANTHROPIC_API_KEY")?,
        model: std::env::var("EXPLORER_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string()),
        research_model: Some(
            std::env::var("EXPLORER_RESEARCH_MODEL")
                .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string()),
        ),
        output_path: None, // Set per-run in the Run command handler
    })
}

/// Forward approved findings to Cognee via the intake engine CLI.
/// All Cognee access must go through the intake engine — never call Cognee directly.
async fn ingest_findings(_config: &AgentConfig, entry: &review::ReviewEntry) -> anyhow::Result<()> {
    let findings = entry
        .findings_json
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No structured findings to ingest"))?;

    let content = serde_json::to_string_pretty(findings)?;

    let tmp_dir = std::env::temp_dir();
    let filename = format!("explorer_{}.json", &entry.id[..8]);
    let tmp_path = tmp_dir.join(&filename);
    tokio::fs::write(&tmp_path, &content).await?;

    let ie_bin = util::intake_engine_bin();

    let tmp_str = tmp_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("temp path contains invalid UTF-8: {:?}", tmp_path))?;

    let output = tokio::process::Command::new(&ie_bin)
        .args([
            "ingest",
            tmp_str,
            "--dataset",
            "research",
            "--prompt",
            "research",
        ])
        .output()
        .await?;

    let _ = tokio::fs::remove_file(&tmp_path).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Intake engine ingest failed: {stderr}");
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env from project root (../. relative to explorer/)
    dotenvy::from_path("../.env").ok();
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("explorer=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "../data".to_string());
    let reviews = ReviewStore::new(PathBuf::from(&data_dir).join("reviews"))?;

    match cli.command {
        Commands::Run { directive, r#async } => {
            let mut config = build_config()?;

            // Set up partial output path for crash recovery
            let partial_path = PathBuf::from(&data_dir).join("partial_output.md");
            config.output_path = Some(partial_path);

            eprintln!("Starting research: {directive}");
            eprintln!(
                "Model: {} (research: {})",
                config.model,
                config.research_model.as_deref().unwrap_or(&config.model)
            );
            eprintln!();

            let result = agent::run(&config, &directive).await?;
            let id = reviews.save(&directive, result.findings_json.clone(), &result.raw_output)?;

            if r#async {
                eprintln!("\nFindings saved for review: {id}");
                eprintln!("Run `explorer review` to see pending reviews.");
            } else {
                let entry = reviews.get(&id)?;
                let status = review::interactive_review(&entry)?;
                reviews.update_status(&id, status.clone())?;

                match status {
                    ReviewStatus::Approved => {
                        eprintln!("\nApproved. Ingesting findings into knowledge base...");
                        match ingest_findings(&config, &entry).await {
                            Ok(()) => eprintln!("Findings ingested successfully."),
                            Err(e) => eprintln!(
                                "Warning: ingest failed: {e}\nFindings are saved locally and can be retried."
                            ),
                        }
                    }
                    ReviewStatus::Rejected => {
                        eprintln!("\nRejected.");
                    }
                    _ => {}
                }
            }
        }

        Commands::Review => {
            let pending = reviews.list_pending()?;
            if pending.is_empty() {
                println!("No pending reviews.");
            } else {
                println!("{} pending review(s):\n", pending.len());
                for entry in &pending {
                    let claims = entry
                        .findings_json
                        .as_ref()
                        .and_then(|f| f["findings"].as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    println!(
                        "  {} — \"{}\" ({} claims, {})",
                        &entry.id[..8],
                        entry.directive,
                        claims,
                        entry.timestamp
                    );
                }
                println!("\nUse `explorer approve <id>` or `explorer reject <id>`.");
            }
        }

        Commands::Approve { id } => {
            let config = build_config()?;
            let full_id = resolve_id(&reviews, &id)?;
            let entry = reviews.get(&full_id)?;
            reviews.update_status(&full_id, ReviewStatus::Approved)?;
            eprintln!("Approved: {full_id}");

            eprintln!("Ingesting findings into knowledge base...");
            match ingest_findings(&config, &entry).await {
                Ok(()) => eprintln!("Findings ingested successfully."),
                Err(e) => eprintln!(
                    "Warning: ingest failed: {e}\nFindings are saved locally and can be retried."
                ),
            }
        }

        Commands::Reject { id } => {
            let full_id = resolve_id(&reviews, &id)?;
            reviews.update_status(&full_id, ReviewStatus::Rejected)?;
            eprintln!("Rejected: {full_id}");
        }

        Commands::View { id } => {
            let full_id = resolve_id(&reviews, &id)?;
            let entry = reviews.get(&full_id)?;
            let raw = std::fs::read_to_string(&entry.raw_output_path)?;
            println!("{raw}");
        }
    }

    Ok(())
}

fn require_env(name: &str) -> anyhow::Result<String> {
    std::env::var(name).map_err(|_| anyhow::anyhow!("{name} environment variable is required"))
}

/// Allow short ID prefixes (first 8 chars).
fn resolve_id(reviews: &ReviewStore, prefix: &str) -> anyhow::Result<String> {
    let pending = reviews.list_pending()?;
    for entry in &pending {
        if entry.id.starts_with(prefix) {
            return Ok(entry.id.clone());
        }
    }
    // Try non-pending too
    anyhow::bail!("No review found matching '{prefix}'")
}
