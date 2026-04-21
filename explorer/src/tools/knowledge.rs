use tokio::process::Command;

use crate::util;

/// Search the existing knowledge base via the intake engine CLI.
/// All Cognee access must go through the intake engine — never call Cognee directly.
/// Falls back gracefully if the intake engine is unavailable.
///
/// Returns results with a structured header for traceability.
pub async fn check(query: &str) -> anyhow::Result<String> {
    let ie_bin = util::intake_engine_bin();

    let output = match Command::new(&ie_bin)
        .args(["search", query, "-t", "GRAPH_COMPLETION"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(_) => {
            tracing::info!("[KB] query=\"{}\" result=unavailable", query);
            return Ok("[PRIOR KNOWLEDGE — unavailable]\n\
                 Knowledge base not available (intake engine not found). \
                 Proceeding without existing knowledge."
                .to_string());
        }
    };

    if !output.status.success() {
        tracing::info!("[KB] query=\"{}\" result=error", query);
        return Ok("[PRIOR KNOWLEDGE — error]\n\
             Knowledge base search failed. Proceeding without existing knowledge."
            .to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();

    if trimmed.is_empty() || trimmed == "[]" || trimmed == "null" {
        tracing::info!("[KB] query=\"{}\" result=empty", query);
        return Ok(format!(
            "[PRIOR KNOWLEDGE — 0 results for \"{}\"]\n\
             No existing knowledge found for this query.",
            query
        ));
    }

    let char_count = trimmed.len();
    let token_estimate = char_count / 4;
    tracing::info!(
        "[KB] query=\"{}\" result=found chars={} ~{}tokens",
        query,
        char_count,
        token_estimate
    );

    Ok(format!(
        "[PRIOR KNOWLEDGE — results for \"{}\" ({} chars)]\n{}",
        query, char_count, trimmed
    ))
}
