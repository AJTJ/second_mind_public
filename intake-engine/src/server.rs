use std::sync::Arc;

use chrono::Utc;
use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

use crate::backend::Backend;
use crate::library::Library;
use crate::log::IntakeLog;
use crate::prompts::PromptRegistry;
use crate::search_log::{SearchEntry, SearchLog};
use crate::types::{
    EntryStatus, IngestParams, LogEntry, LogQueryParams, ReplayParams, SearchParams,
};

#[derive(Clone)]
pub struct IntakeEngine {
    backend: Arc<dyn Backend>,
    log: Arc<IntakeLog>,
    search_log: Arc<SearchLog>,
    library: Arc<Library>,
    prompts: Arc<PromptRegistry>,
    /// Max document size in estimated tokens before rejecting.
    /// Derived from the embedding model's context window.
    max_doc_tokens: usize,
    tool_router: ToolRouter<Self>,
}

impl IntakeEngine {
    pub fn new(
        backend: Arc<dyn Backend>,
        log: Arc<IntakeLog>,
        search_log: Arc<SearchLog>,
        library: Arc<Library>,
        prompts: Arc<PromptRegistry>,
        max_doc_tokens: usize,
    ) -> Self {
        Self {
            backend,
            log,
            search_log,
            library,
            prompts,
            max_doc_tokens,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl IntakeEngine {
    #[tool(
        description = "Ingest sources into the knowledge base. Accepts batch of text, URLs, files, or audio. Logs the decision and forwards to the graph engine for integration."
    )]
    async fn ingest(&self, Parameters(params): Parameters<IngestParams>) -> String {
        let entry_id = ulid::Ulid::new().to_string();

        // Resolve prompt
        let prompt = match self.prompts.resolve(&params.prompt_name) {
            Ok(p) => p,
            Err(e) => return format!("Failed to resolve prompt '{}': {e}", params.prompt_name),
        };

        // Store each source
        let mut stored = Vec::new();
        for (i, source) in params.sources.iter().enumerate() {
            let source_id = format!("{entry_id}_{i}");
            match self.library.store(&source_id, source).await {
                Ok(s) => stored.push(s),
                Err(e) => return format!("Failed to store source {i}: {e}"),
            }
        }

        // Create log entry
        let mut entry = LogEntry {
            id: entry_id.clone(),
            timestamp: Utc::now(),
            sources: stored.clone(),
            datasets: params.datasets.clone(),
            prompt: prompt.clone(),
            backend: "graph-engine".to_string(),
            status: EntryStatus::Pending,
            error: None,
            replayed_from: None,
        };

        if let Err(e) = self.log.append(&entry) {
            return format!("Failed to write log: {e}");
        }

        // Forward each source to Cognee add()
        for source in &stored {
            let content = match self.library.read_source(&source.stored_path) {
                Ok(c) => c,
                Err(e) => {
                    entry.status = EntryStatus::Failed;
                    entry.error = Some(format!("Failed to read stored source: {e}"));
                    let _ = self.log.append(&entry);
                    return format!("Failed to read stored source: {e}");
                }
            };

            // Proactive size check: reject documents that exceed the embedding
            // model's context window. Estimated as chars/4 (conservative).
            let estimated_tokens = content.len() / 4;
            if estimated_tokens > self.max_doc_tokens {
                let msg = format!(
                    "Document {} is ~{}K tokens, exceeding the embedding model limit of {}K tokens. \
                     Split the document before ingesting.",
                    source.stored_path,
                    estimated_tokens / 1000,
                    self.max_doc_tokens / 1000,
                );
                entry.status = EntryStatus::Failed;
                entry.error = Some(msg.clone());
                let _ = self.log.append(&entry);
                return msg;
            }

            for dataset in &params.datasets {
                if let Err(e) = self
                    .backend
                    .add(&content, dataset, &source.stored_path)
                    .await
                {
                    entry.status = EntryStatus::Failed;
                    entry.error = Some(format!("Cognee add failed: {e}"));
                    let _ = self.log.append(&entry);
                    return format!("Cognee add failed for dataset '{dataset}': {e}");
                }
            }
        }

        entry.status = EntryStatus::Added;
        let _ = self.log.append(&entry);

        // Integrate (entity extraction + embedding)
        if let Err(e) = self
            .backend
            .integrate(&params.datasets, &prompt.content)
            .await
        {
            entry.status = EntryStatus::AddedNotIntegrated;
            entry.error = Some(format!("Integration failed: {e}"));
            let _ = self.log.append(&entry);
            return format!("Data added but integration failed (recoverable via replay): {e}");
        }

        entry.status = EntryStatus::Integrated;
        let _ = self.log.append(&entry);

        format!(
            "Ingested {} source(s) into dataset(s) [{}] with prompt '{}'. Entry: {entry_id}",
            stored.len(),
            params.datasets.join(", "),
            prompt.name,
        )
    }

    #[tool(description = "Search the knowledge base via the graph engine.")]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        let result = self
            .backend
            .search(
                &params.query,
                params.datasets.as_deref(),
                params.search_type.as_deref(),
                params.top_k,
            )
            .await;

        let output = match result {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
            Err(e) => format!("Search error: {e}"),
        };

        // Log the search for comparative analysis
        let preview: String = output.chars().take(500).collect();
        let _ = self.search_log.append(&SearchEntry {
            timestamp: chrono::Utc::now(),
            query: params.query,
            datasets: params.datasets,
            search_type: params.search_type,
            top_k: params.top_k,
            result_preview: preview,
            result_chars: output.len(),
        });

        output
    }

    #[tool(
        description = "Replay previously logged ingestion entries. Re-runs sources through the backend with original or overridden prompts. Omit entry_id to replay all entries."
    )]
    async fn replay(&self, Parameters(params): Parameters<ReplayParams>) -> String {
        let entries = if let Some(ref id) = params.entry_id {
            match self.log.get(id) {
                Ok(Some(e)) => vec![e],
                Ok(None) => return format!("Entry '{id}' not found"),
                Err(e) => return format!("Failed to read log: {e}"),
            }
        } else {
            match self.log.read_all() {
                Ok(e) => e,
                Err(e) => return format!("Failed to read log: {e}"),
            }
        };

        // Replay integrated and added-not-integrated entries
        let entries: Vec<_> = entries
            .into_iter()
            .filter(|e| {
                e.status == EntryStatus::Integrated || e.status == EntryStatus::AddedNotIntegrated
            })
            .collect();

        if entries.is_empty() {
            return "No integrated entries to replay.".to_string();
        }

        let mut replayed = 0;
        let mut failed = 0;

        for original in &entries {
            let prompt = if let Some(ref override_name) = params.prompt_override {
                match self.prompts.resolve(override_name) {
                    Ok(p) => p,
                    Err(e) => {
                        failed += 1;
                        tracing::error!("Failed to resolve prompt override: {e}");
                        continue;
                    }
                }
            } else {
                original.prompt.clone()
            };

            let replay_id = ulid::Ulid::new().to_string();
            let mut entry = LogEntry {
                id: replay_id,
                timestamp: Utc::now(),
                sources: original.sources.clone(),
                datasets: original.datasets.clone(),
                prompt: prompt.clone(),
                backend: "graph-engine".to_string(),
                status: EntryStatus::Pending,
                error: None,
                replayed_from: Some(original.id.clone()),
            };

            let _ = self.log.append(&entry);

            // Re-add each source
            let mut add_ok = true;
            for source in &original.sources {
                let content = match self.library.read_source(&source.stored_path) {
                    Ok(c) => c,
                    Err(e) => {
                        entry.status = EntryStatus::Failed;
                        entry.error = Some(format!("Missing source: {e}"));
                        let _ = self.log.append(&entry);
                        add_ok = false;
                        break;
                    }
                };

                for dataset in &original.datasets {
                    if let Err(e) = self
                        .backend
                        .add(&content, dataset, &source.stored_path)
                        .await
                    {
                        entry.status = EntryStatus::Failed;
                        entry.error = Some(format!("Cognee add failed: {e}"));
                        let _ = self.log.append(&entry);
                        add_ok = false;
                        break;
                    }
                }
                if !add_ok {
                    break;
                }
            }

            if !add_ok {
                failed += 1;
                continue;
            }

            // Integrate with (possibly overridden) prompt
            if let Err(e) = self
                .backend
                .integrate(&original.datasets, &prompt.content)
                .await
            {
                entry.status = EntryStatus::Failed;
                entry.error = Some(format!("Integration failed: {e}"));
                let _ = self.log.append(&entry);
                failed += 1;
                continue;
            }

            entry.status = EntryStatus::Integrated;
            let _ = self.log.append(&entry);
            replayed += 1;
        }

        format!(
            "Replay complete: {replayed} succeeded, {failed} failed (out of {} entries)",
            entries.len()
        )
    }

    #[tool(
        description = "View intake log history. Filter by dataset, prompt name, or limit results."
    )]
    async fn log(&self, Parameters(params): Parameters<LogQueryParams>) -> String {
        match self.log.query(&params) {
            Ok(entries) => {
                if entries.is_empty() {
                    return "No entries found.".to_string();
                }
                serde_json::to_string_pretty(&entries)
                    .unwrap_or_else(|_| format!("{} entries found", entries.len()))
            }
            Err(e) => format!("Failed to query log: {e}"),
        }
    }
}

#[tool_handler]
impl ServerHandler for IntakeEngine {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Intake engine for the Learning Engine. \
                 Logs every knowledge ingestion decision for replay. \
                 Tools: ingest (add knowledge), search (query), replay (re-process), log (view history)."
                    .to_string(),
            )
    }
}
