mod cache;
mod chunker;
mod communities;
mod embedder;
mod extractor;
mod graph;
mod pipeline;
mod resolver;
mod schema;
mod server;
mod store;
mod temporal;
mod types;
mod vectors;

use std::sync::Arc;

use clap::{Parser, Subcommand};

use store::{GraphStore, VectorStore};

#[derive(Parser)]
#[command(name = "second-mind", about = "Knowledge graph engine")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Serve,
    Migrate,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("second_mind=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    let backend = std::env::var("GRAPH_BACKEND").unwrap_or_else(|_| "postgres".to_string());

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://cognee:cognee@localhost:5432/cognee".to_string());

    let (graph_store, vector_store, _pg_pool): (Arc<dyn GraphStore>, Arc<dyn VectorStore>, sqlx::PgPool) =
        match backend.as_str() {
            "neo4j" => {
                let uri = std::env::var("NEO4J_URI")
                    .unwrap_or_else(|_| "bolt://localhost:7687".to_string());
                let user =
                    std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string());
                let pass =
                    std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "password".to_string());
                let neo4j = store::neo4j::Neo4jGraphStore::new(&uri, &user, &pass).await?;
                // Neo4j handles graph, Postgres still handles vectors (pgvector)
                let pool = sqlx::PgPool::connect(&database_url).await?;
                let pg_vectors = store::postgres::PostgresVectorStore::new(pool.clone());
                (
                    Arc::new(neo4j) as Arc<dyn GraphStore>,
                    Arc::new(pg_vectors) as Arc<dyn VectorStore>,
                    pool,
                )
            }
            _ => {
                let pool = sqlx::PgPool::connect(&database_url).await?;
                let pg_graph = store::postgres::PostgresGraphStore::new(pool.clone());
                let pg_vectors = store::postgres::PostgresVectorStore::new(pool.clone());
                (Arc::new(pg_graph), Arc::new(pg_vectors) as Arc<dyn VectorStore>, pool)
            }
        };

    match cli.command.unwrap_or(Commands::Serve) {
        Commands::Migrate => {
            tracing::info!("Running migrations...");
            graph_store.initialize().await?;
            tracing::info!("Migrations complete.");
        }
        Commands::Serve => {
            graph_store.initialize().await?;

            let embed_endpoint = std::env::var("EMBEDDING_ENDPOINT")
                .unwrap_or_else(|_| "http://ollama:11434/api/embed".to_string());
            let embed_model = std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "qwen3-embedding:4b".to_string());

            let max_gleanings: u32 = std::env::var("MAX_GLEANINGS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);

            let extractor_instance: Arc<dyn extractor::Extractor> =
                match std::env::var("ANTHROPIC_API_KEY") {
                    Ok(api_key) if !api_key.is_empty() && api_key != "your_anthropic_api_key" => {
                        let model = std::env::var("LLM_MODEL")
                            .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());
                        tracing::info!("Using LlmExtractor (model: {model}, gleanings: {max_gleanings})");
                        Arc::new(
                            extractor::LlmExtractor::new(api_key, model)
                                .with_max_gleanings(max_gleanings)
                                .with_cache(graph_store.clone()),
                        )
                    }
                    _ => {
                        tracing::warn!(
                            "ANTHROPIC_API_KEY not set — using MockExtractor (test data only)"
                        );
                        Arc::new(extractor::MockExtractor)
                    }
                };

            let state = Arc::new(server::AppState {
                graph_store,
                vector_store,
                embedder: Arc::new(embedder::OllamaEmbedder::new(embed_endpoint, embed_model)),
                extractor: extractor_instance,
                chunker_config: chunker::ChunkerConfig::default(),
            });

            let app = server::router(state);
            let port = std::env::var("PORT").unwrap_or_else(|_| "8000".to_string());
            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;

            tracing::info!("Second Mind listening on :{port}");
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}
