mod backend;
mod cognee;
mod graph_engine;
mod library;
mod log;
mod prompts;
mod search_log;
mod server;
mod types;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::backend::Backend;
use crate::cognee::CogneeAdapter;
use crate::graph_engine::GraphEngineAdapter;
use crate::library::Library;
use crate::log::IntakeLog;
use crate::prompts::PromptRegistry;
use crate::server::IntakeEngine;
use crate::types::{EntryStatus, LogEntry, LogQueryParams, SourceInput, SourceType};

#[derive(Parser)]
#[command(
    name = "intake-engine",
    about = "Learning Engine intake — log, store, and forward knowledge to Cognee"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server (default if no command given)
    Serve,

    /// Ingest a file into the knowledge base
    Ingest {
        /// File path to ingest
        file: String,
        /// Target dataset (e.g. "research", "personal")
        #[arg(short, long, default_value = "research")]
        dataset: String,
        /// Prompt name from channels/ directory
        #[arg(short, long, default_value = "research")]
        prompt: String,
        /// Add data without running integration (use `integrate` command separately)
        #[arg(long)]
        no_integrate: bool,
    },

    /// Search the knowledge base
    Search {
        /// Search query
        query: String,
        /// Dataset to search
        #[arg(short, long)]
        dataset: Option<String>,
        /// Search type: SUMMARIES, CHUNKS, GRAPH_COMPLETION
        #[arg(short = 't', long, default_value = "SUMMARIES")]
        search_type: String,
    },

    /// Show intake log
    Log {
        /// Filter by dataset
        #[arg(short, long)]
        dataset: Option<String>,
        /// Max entries
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Replay logged entries
    Replay {
        /// Specific entry ID (omit for all)
        entry_id: Option<String>,
        /// Override prompt name
        #[arg(short, long)]
        prompt: Option<String>,
    },

    /// Run integration on a dataset (entity extraction + embedding)
    Integrate {
        /// Dataset to integrate
        dataset: String,
        /// Prompt name from channels/ directory
        #[arg(short, long, default_value = "research")]
        prompt: String,
    },

    /// Delete a dataset from Cognee and mark log entries
    DeleteDataset {
        /// Dataset name to delete
        dataset: String,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    /// Compare search results between two datasets for the same query
    Compare {
        /// Search query
        query: String,
        /// First dataset
        dataset_a: String,
        /// Second dataset
        dataset_b: String,
        /// Search type
        #[arg(short = 't', long, default_value = "GRAPH_COMPLETION")]
        search_type: String,
    },

    /// View search history
    SearchHistory {
        /// Max entries
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Wipe all data from Cognee and reset the intake log
    Wipe {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    /// List available prompts
    Prompts,

    /// Compact the intake log (deduplicate in-place)
    Compact,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::from_path("../.env").ok();
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    let cognee_url =
        std::env::var("COGNEE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string());
    let cognee_user =
        std::env::var("COGNEE_USER").unwrap_or_else(|_| "admin@secondmind.local".to_string());
    let cognee_pass =
        std::env::var("COGNEE_PASS").unwrap_or_else(|_| "second-mind-local".to_string());
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let prompts_dir = std::env::var("PROMPTS_DIR").unwrap_or_else(|_| "../channels".to_string());

    // Embedding model context limit in tokens. Default: 32K (conservative for qwen3-embedding:4b's 40K).
    // Set EMBEDDING_MAX_TOKENS to override (e.g. when changing models).
    let max_doc_tokens: usize = std::env::var("EMBEDDING_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(32_000);

    let data_path = PathBuf::from(&data_dir);
    std::fs::create_dir_all(&data_path)?;

    let backend_type = std::env::var("BACKEND").unwrap_or_else(|_| "graph-engine".to_string());
    let backend: Arc<dyn Backend> = match backend_type.as_str() {
        "cognee" => Arc::new(CogneeAdapter::new(cognee_url, cognee_user, cognee_pass)),
        _ => Arc::new(GraphEngineAdapter::new(cognee_url)),
    };
    let log = Arc::new(IntakeLog::new(data_path.join("intake.jsonl")));
    let search_log = Arc::new(search_log::SearchLog::new(data_path.join("searches.jsonl")));
    let library = Arc::new(Library::new(data_path.join("library"))?);
    let prompt_registry = Arc::new(PromptRegistry::new(PathBuf::from(&prompts_dir)));

    match cli.command.unwrap_or(Commands::Serve) {
        Commands::Serve => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::from_default_env()
                        .add_directive("intake_engine=info".parse()?),
                )
                .init();

            let port = std::env::var("INTAKE_PORT").unwrap_or_else(|_| "8001".to_string());

            let available = prompt_registry.list().unwrap_or_default();
            tracing::info!("Available prompts: {}", available.join(", "));
            tracing::info!("Listening on :{port}/mcp");

            let engine = IntakeEngine::new(
                backend,
                log,
                search_log.clone(),
                library,
                prompt_registry,
                max_doc_tokens,
            );

            let ct = CancellationToken::new();
            let config = StreamableHttpServerConfig::default()
                .with_sse_keep_alive(Some(Duration::from_secs(15)))
                .with_cancellation_token(ct.child_token());

            let service = StreamableHttpService::new(
                move || Ok(engine.clone()),
                LocalSessionManager::default().into(),
                config,
            );

            let router = axum::Router::new().nest_service("/mcp", service);
            let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await?;

            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    tokio::signal::ctrl_c().await.ok();
                    ct.cancel();
                })
                .await?;
        }

        Commands::Ingest {
            file,
            dataset,
            prompt,
            no_integrate,
        } => {
            let prompt_ref = prompt_registry.resolve(&prompt)?;
            let entry_id = ulid::Ulid::new().to_string();

            let source = SourceInput {
                source_type: SourceType::File,
                content: None,
                path: Some(file.clone()),
            };

            // Store in library
            let stored = library.store(&format!("{entry_id}_0"), &source).await?;
            let content = library.read_source(&stored.stored_path)?;
            let stored_path = stored.stored_path.clone();

            // Log the entry
            let mut entry = LogEntry {
                id: entry_id.clone(),
                timestamp: chrono::Utc::now(),
                sources: vec![stored],
                datasets: vec![dataset.clone()],
                prompt: prompt_ref.clone(),
                backend: "graph-engine".to_string(),
                status: EntryStatus::Pending,
                error: None,
                replayed_from: None,
            };
            log.append(&entry)?;

            // Forward to Cognee
            eprintln!("Adding to dataset '{dataset}'...");
            backend.add(&content, &dataset, &stored_path).await?;
            entry.status = EntryStatus::Added;
            log.append(&entry)?;

            if no_integrate {
                eprintln!(
                    "Skipping integration (--no-integrate). Run `intake-engine integrate` to process."
                );
                eprintln!("Done. Entry: {entry_id}");
                eprintln!("  Source: {file}");
                eprintln!("  Dataset: {dataset}");
                eprintln!("  Prompt: {prompt}");
            } else {
                eprintln!("Integrating with prompt '{prompt}'...");
                backend
                    .integrate(std::slice::from_ref(&dataset), &prompt_ref.content)
                    .await?;
                entry.status = EntryStatus::Integrated;
                log.append(&entry)?;

                eprintln!("Done. Entry: {entry_id}");
                eprintln!("  Source: {file}");
                eprintln!("  Dataset: {dataset}");
                eprintln!("  Prompt: {prompt}");
            }
        }

        Commands::Search {
            query,
            dataset,
            search_type,
        } => {
            let datasets: Option<Vec<String>> = dataset.clone().map(|d| vec![d]);
            let result = backend
                .search(&query, datasets.as_deref(), Some(&search_type), Some(5))
                .await?;

            let output = serde_json::to_string_pretty(&result)?;
            println!("{output}");

            // Log for comparative analysis
            let preview: String = output.chars().take(500).collect();
            let _ = search_log.append(&search_log::SearchEntry {
                timestamp: chrono::Utc::now(),
                query,
                datasets: dataset.map(|d| vec![d]),
                search_type: Some(search_type),
                top_k: Some(5),
                result_preview: preview,
                result_chars: output.len(),
            });
        }

        Commands::Log { dataset, limit } => {
            let params = LogQueryParams {
                dataset,
                prompt: None,
                limit: Some(limit),
            };
            let entries = log.query(&params)?;

            if entries.is_empty() {
                eprintln!("No entries found.");
            } else {
                for entry in &entries {
                    let sources: Vec<&str> = entry
                        .sources
                        .iter()
                        .map(|s| s.original_ref.as_str())
                        .collect();
                    println!(
                        "{} | {:?} | {} | {} | {}",
                        &entry.id[..8],
                        entry.status,
                        entry.datasets.join(","),
                        entry.prompt.name,
                        sources.join(", ")
                    );
                }
                eprintln!("\n{} entries shown.", entries.len());
            }
        }

        Commands::Replay { entry_id, prompt } => {
            let entries = if let Some(ref id) = entry_id {
                match log.get(id)? {
                    Some(e) => vec![e],
                    None => {
                        // Try prefix match
                        let all = log.read_all()?;
                        let matches: Vec<_> =
                            all.into_iter().filter(|e| e.id.starts_with(id)).collect();
                        if matches.is_empty() {
                            anyhow::bail!("No entry found matching '{id}'");
                        }
                        matches
                    }
                }
            } else {
                log.read_all()?
            };

            let entries: Vec<_> = entries
                .into_iter()
                .filter(|e| {
                    e.status == EntryStatus::Integrated
                        || e.status == EntryStatus::AddedNotIntegrated
                })
                .collect();

            if entries.is_empty() {
                eprintln!("No integrated entries to replay.");
                return Ok(());
            }

            eprintln!("Replaying {} entries...", entries.len());

            for original in &entries {
                let prompt_ref = if let Some(ref name) = prompt {
                    prompt_registry.resolve(name)?
                } else {
                    original.prompt.clone()
                };

                let replay_id = ulid::Ulid::new().to_string();

                // Re-add each source
                for source in &original.sources {
                    let content = library.read_source(&source.stored_path)?;
                    for dataset in &original.datasets {
                        backend.add(&content, dataset, &source.stored_path).await?;
                    }
                }

                // Integrate
                backend
                    .integrate(&original.datasets, &prompt_ref.content)
                    .await?;

                let entry = LogEntry {
                    id: replay_id.clone(),
                    timestamp: chrono::Utc::now(),
                    sources: original.sources.clone(),
                    datasets: original.datasets.clone(),
                    prompt: prompt_ref,
                    backend: "graph-engine".to_string(),
                    status: EntryStatus::Integrated,
                    error: None,
                    replayed_from: Some(original.id.clone()),
                };
                log.append(&entry)?;

                eprintln!("  Replayed {} → {}", &original.id[..8], &replay_id[..8]);
            }

            eprintln!("Done.");
        }

        Commands::Integrate { dataset, prompt } => {
            let prompt_ref = prompt_registry.resolve(&prompt)?;
            eprintln!("Integrating dataset '{dataset}' with prompt '{prompt}'...");
            backend
                .integrate(std::slice::from_ref(&dataset), &prompt_ref.content)
                .await?;
            eprintln!("Done.");
        }

        Commands::DeleteDataset { dataset, force } => {
            let params = LogQueryParams {
                dataset: Some(dataset.clone()),
                prompt: None,
                limit: None,
            };
            let entries = log.query(&params)?;
            let active_count = entries
                .iter()
                .filter(|e| e.status != EntryStatus::DatasetDeleted)
                .count();

            if !force {
                eprintln!(
                    "About to delete dataset '{}' from Cognee ({} log entries will be marked deleted).",
                    dataset, active_count
                );
                eprint!("Type 'yes' to confirm: ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if input.trim() != "yes" {
                    eprintln!("Aborted.");
                    return Ok(());
                }
            }

            backend.delete_dataset(&dataset).await?;
            let marked = log.mark_dataset_deleted(&dataset)?;
            eprintln!(
                "Deleted dataset '{}' from Cognee. Marked {} log entries as deleted.",
                dataset, marked
            );
        }

        Commands::Compare {
            query,
            dataset_a,
            dataset_b,
            search_type,
        } => {
            eprintln!("Comparing: \"{}\"", query);
            eprintln!("  Dataset A: {dataset_a}");
            eprintln!("  Dataset B: {dataset_b}");
            eprintln!();

            let result_a = backend
                .search(
                    &query,
                    Some(std::slice::from_ref(&dataset_a)),
                    Some(&search_type),
                    Some(5),
                )
                .await;
            let result_b = backend
                .search(
                    &query,
                    Some(std::slice::from_ref(&dataset_b)),
                    Some(&search_type),
                    Some(5),
                )
                .await;

            println!("=== {} ===", dataset_a);
            match result_a {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
                Err(e) => println!("Error: {e}"),
            }
            println!();
            println!("=== {} ===", dataset_b);
            match result_b {
                Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
                Err(e) => println!("Error: {e}"),
            }
        }

        Commands::SearchHistory { limit } => {
            let entries = search_log.read_all()?;
            if entries.is_empty() {
                eprintln!("No search history.");
            } else {
                let start = entries.len().saturating_sub(limit);
                for entry in &entries[start..] {
                    let ds = entry
                        .datasets
                        .as_ref()
                        .map(|d| d.join(","))
                        .unwrap_or_else(|| "all".to_string());
                    let st = entry.search_type.as_deref().unwrap_or("?");
                    println!(
                        "{} | {} | [{}] {} | {} chars",
                        &entry.timestamp.format("%Y-%m-%d %H:%M"),
                        ds,
                        st,
                        entry.query,
                        entry.result_chars
                    );
                }
                eprintln!("\n{} entries shown.", entries[start..].len());
            }
        }

        Commands::Wipe { force } => {
            if !force {
                eprintln!("This will DELETE all data from Cognee and reset the intake log.");
                eprint!("Type 'yes' to confirm: ");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                if input.trim() != "yes" {
                    eprintln!("Aborted.");
                    return Ok(());
                }
            }

            backend.delete_all_datasets().await?;

            // Reset the intake log
            let log_path = data_path.join("intake.jsonl");
            std::fs::write(&log_path, "")?;

            eprintln!("Wiped all Cognee data and reset intake log.");
        }

        Commands::Prompts => {
            let available = prompt_registry.list().unwrap_or_default();
            if available.is_empty() {
                eprintln!("No prompts found in {prompts_dir}");
            } else {
                for name in &available {
                    println!("  {name}");
                }
            }
        }

        Commands::Compact => {
            let removed = log.compact()?;
            if removed == 0 {
                eprintln!("Log already compact (no duplicates).");
            } else {
                eprintln!("Compacted: removed {removed} duplicate entries.");
            }
        }
    }

    Ok(())
}
