use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};

use crate::api;
use crate::tools;

/// Max characters per tool result in context (except read_page which self-truncates).
const MAX_TOOL_RESULT_CHARS: usize = 2_000;

/// Default cost cap in cents.
const DEFAULT_MAX_COST_CENTS: u64 = 100; // $1.00

const SYSTEM_PROMPT: &str = r#"You are a research agent. Your job is to thoroughly research a given directive and produce structured findings.

You work in phases:
1. PLAN — Decompose the directive into 3-7 specific sub-questions. Check existing knowledge first.
2. SURVEY — Search broadly across sub-questions. Use the `research` tool for web information and `search_papers` for academic backing. Get the lay of the land before going deep.
3. DEEPEN — Follow the most important threads from SURVEY. Use `read_page` to get full content from key sources. Go deeper on high-value leads.
4. CHALLENGE — Actively search for counter-evidence and contradictions. Ask "What could make this wrong?" and "Who disagrees?"
5. SYNTHESIZE — Produce your structured findings (see output format below).
6. GAPS — Identify what you couldn't find, what remains uncertain, and suggest follow-up research.

Rules:
- You MUST attempt all phases, especially CHALLENGE. Don't skip the adversarial pass.
- For important claims, try to find 2-3 independent sources.
- Track source quality: primary data > analyst report > news article > blog post.
- State your current phase as [PHASE_NAME] at the start of each thinking block.
- Before synthesizing, review your accumulated findings and re-state the most important claims and their evidence. This ensures critical findings are fresh in context for synthesis.
- You MUST fill in `knowledge_influence` and `prior_knowledge_used` fields accurately. If check_knowledge returned useful data, set prior_knowledge_used=true on claims it informed. If it returned nothing, say so in influence_description. These fields are required, not optional.
- When you're ready to synthesize, output your findings in the JSON format described below.

When you are done researching, output EXACTLY this JSON block (no other text around it):
```json
{
  "status": "complete",
  "knowledge_influence": {
    "queries_made": 1,
    "results_received": true,
    "claims_informed": 3,
    "claims_total": 8,
    "influence_description": "Brief description of how prior knowledge shaped these findings"
  },
  "findings": [
    {
      "claim": "...",
      "confidence": "established|emerging|contested",
      "sources": [{"url": "...", "title": "...", "type": "primary_data|analyst_report|news|academic|blog", "date": "..."}],
      "supporting_evidence": ["..."],
      "counter_evidence": ["..."],
      "related_concepts": ["..."],
      "prior_knowledge_used": false
    }
  ],
  "gaps": ["..."],
  "suggested_follow_ups": ["..."]
}
```

REQUIRED: `prior_knowledge_used` must be true if check_knowledge data informed the claim, false otherwise. `knowledge_influence` must accurately count queries_made and claims_informed. These fields are mandatory."#;

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: Value,
}

pub struct AgentConfig {
    pub anthropic_api_key: String,
    pub model: String,
    pub research_model: Option<String>,
    pub output_path: Option<std::path::PathBuf>,
}

pub struct AgentResult {
    pub raw_output: String,
    pub findings_json: Option<Value>,
}

/// Known phases from the system prompt.
const PHASES: &[&str] = &[
    "PLAN",
    "SURVEY",
    "DEEPEN",
    "CHALLENGE",
    "SYNTHESIZE",
    "GAPS",
];

// --- Cost tracking ---

/// Model pricing in cents per token.
struct Pricing {
    input: f64,
    output: f64,
}

impl Pricing {
    fn for_model(model: &str) -> Self {
        let lower = model.to_ascii_lowercase();
        if lower.contains("haiku") {
            Self {
                input: 0.00008,
                output: 0.0004,
            } // $0.80/$4 per M
        } else if lower.contains("opus") {
            Self {
                input: 0.0015,
                output: 0.0075,
            } // $15/$75 per M
        } else {
            Self {
                input: 0.0003,
                output: 0.0015,
            } // $3/$15 per M (Sonnet default)
        }
    }

    fn cost_cents(&self, usage: &api::Usage) -> f64 {
        usage.input_tokens as f64 * self.input + usage.output_tokens as f64 * self.output
    }
}

struct CostTracker {
    main_usage: api::Usage,
    nested_usage: api::Usage,
    main_pricing: Pricing,
    nested_pricing: Pricing,
    max_cents: u64,
}

impl CostTracker {
    fn new(main_model: &str, research_model: &str, max_cents: u64) -> Self {
        Self {
            main_usage: api::Usage::default(),
            nested_usage: api::Usage::default(),
            main_pricing: Pricing::for_model(main_model),
            nested_pricing: Pricing::for_model(research_model),
            max_cents,
        }
    }

    fn track_main(&mut self, usage: &api::Usage) {
        self.main_usage.input_tokens += usage.input_tokens;
        self.main_usage.output_tokens += usage.output_tokens;
    }

    fn track_nested(&mut self, usage: &api::Usage) {
        self.nested_usage.input_tokens += usage.input_tokens;
        self.nested_usage.output_tokens += usage.output_tokens;
    }

    fn total_cents(&self) -> f64 {
        self.main_pricing.cost_cents(&self.main_usage)
            + self.nested_pricing.cost_cents(&self.nested_usage)
    }

    fn over_budget(&self) -> bool {
        self.total_cents() > self.max_cents as f64
    }

    fn summary(&self) -> String {
        format!(
            "main: {}+{} tokens, nested: {}+{} tokens, total: ${:.3}",
            self.main_usage.input_tokens,
            self.main_usage.output_tokens,
            self.nested_usage.input_tokens,
            self.nested_usage.output_tokens,
            self.total_cents() / 100.0
        )
    }
}

// --- Phase detection ---

fn detect_phase(text: &str) -> Option<&'static str> {
    let mut best: Option<(usize, &str)> = None;
    for phase in PHASES {
        let marker = format!("[{phase}]");
        if let Some(pos) = text.rfind(&marker)
            && best.is_none_or(|(best_pos, _)| pos > best_pos)
        {
            best = Some((pos, phase));
        }
    }
    best.map(|(_, p)| p)
}

// --- Context restructuring ---

/// Restructure accumulated messages for the next phase.
/// Strips structural overhead (tool_use request blocks, message wrapping).
/// Preserves all tool results and assistant analysis verbatim.
/// Note: loses the structural link between tool requests and their results,
/// but preserves all content. This is an acceptable tradeoff for reducing
/// context noise — the model can still see what data it has.
fn restructure_context(messages: &[Message]) -> String {
    let mut sections = Vec::new();

    for msg in messages {
        if msg.role == "user"
            && let Some(arr) = msg.content.as_array()
        {
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                    && let Some(text) = item.get("content").and_then(|c| c.as_str())
                {
                    sections.push(text.to_string());
                }
            }
        }

        if msg.role == "assistant"
            && let Some(arr) = msg.content.as_array()
        {
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(text) = item.get("text").and_then(|t| t.as_str())
                {
                    sections.push(text.to_string());
                }
            }
        }
    }

    if sections.is_empty() {
        "No content from this phase.".to_string()
    } else {
        sections.join("\n\n")
    }
}

// --- Tool execution ---

/// Result of executing a tool, including optional nested API usage.
struct ToolOutput {
    text: String,
    nested_usage: Option<api::Usage>,
}

async fn execute_tool(
    client: &Client,
    config: &AgentConfig,
    name: &str,
    input: &Value,
    retry_config: &api::RetryConfig,
) -> ToolOutput {
    let result = match name {
        "research" => {
            let query = match input["query"].as_str() {
                Some(q) if !q.is_empty() => q,
                _ => {
                    return ToolOutput {
                        text: "Error: 'query' parameter is required and must be non-empty.".into(),
                        nested_usage: None,
                    };
                }
            };
            let model = config.research_model.as_deref().unwrap_or(&config.model);
            match tools::web::search(
                client,
                &config.anthropic_api_key,
                model,
                query,
                retry_config,
            )
            .await
            {
                Ok(sr) => {
                    return ToolOutput {
                        text: sr.text,
                        nested_usage: Some(sr.usage),
                    };
                }
                Err(e) => {
                    return ToolOutput {
                        text: format!("Tool error: {e}"),
                        nested_usage: None,
                    };
                }
            }
        }
        "search_papers" => {
            let query = match input["query"].as_str() {
                Some(q) if !q.is_empty() => q,
                _ => {
                    return ToolOutput {
                        text: "Error: 'query' parameter is required and must be non-empty.".into(),
                        nested_usage: None,
                    };
                }
            };
            let limit = input["limit"].as_i64().map(|n| n as i32);
            tools::papers::search(client, query, limit).await
        }
        "read_page" => {
            let url = match input["url"].as_str() {
                Some(u) if !u.is_empty() => u,
                _ => {
                    return ToolOutput {
                        text: "Error: 'url' parameter is required and must be non-empty.".into(),
                        nested_usage: None,
                    };
                }
            };
            tools::reader::read_page(client, url).await
        }
        "check_knowledge" => {
            let query = match input["query"].as_str() {
                Some(q) if !q.is_empty() => q,
                _ => {
                    return ToolOutput {
                        text: "Error: 'query' parameter is required and must be non-empty.".into(),
                        nested_usage: None,
                    };
                }
            };
            tools::knowledge::check(query).await
        }
        _ => Ok(format!("Unknown tool: {name}")),
    };

    ToolOutput {
        text: match result {
            Ok(t) => t,
            Err(e) => format!("Tool error: {e}"),
        },
        nested_usage: None,
    }
}

/// Truncate tool result, preferring sentence boundaries.
fn truncate_tool_result(text: &str, name: &str) -> String {
    // read_page already self-truncates with semantic boundaries
    if name == "read_page" {
        return text.to_string();
    }

    if text.chars().count() <= MAX_TOOL_RESULT_CHARS {
        return text.to_string();
    }

    // Find a char-safe byte boundary
    let byte_limit = text
        .char_indices()
        .nth(MAX_TOOL_RESULT_CHARS)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let slice = &text[..byte_limit];

    let cut = slice
        .rfind(". ")
        .or_else(|| slice.rfind(".\n"))
        .map(|p| p + 1)
        .unwrap_or(byte_limit);

    let truncated = &text[..cut];
    format!("{truncated}\n\n[Truncated from {} chars]", text.len())
}

// --- Main agent loop ---

pub async fn run(config: &AgentConfig, directive: &str) -> anyhow::Result<AgentResult> {
    let client = api::build_client();
    let tool_defs = tools::tool_definitions();
    let retry_config = api::RetryConfig::from_env();

    let max_cost_cents: u64 = std::env::var("EXPLORER_MAX_COST_CENTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_COST_CENTS);

    let research_model = config.research_model.as_deref().unwrap_or(&config.model);
    let mut cost = CostTracker::new(&config.model, research_model, max_cost_cents);

    let mut messages: Vec<Message> = vec![Message {
        role: "user".to_string(),
        content: json!(format!(
            "Research directive: {directive}\n\n\
             Begin with [PLAN] phase. Check existing knowledge first, \
             then decompose into sub-questions."
        )),
    }];

    let mut full_output = String::new();
    let mut findings_json: Option<Value> = None;
    let max_iterations = 30;
    let mut current_phase: Option<&str> = None;
    let mut consecutive_no_tool_iters: u32 = 0;

    for iteration in 0..max_iterations {
        let ctx_estimate = messages
            .iter()
            .map(|m| m.content.to_string().len())
            .sum::<usize>()
            / 4;

        tracing::info!(
            "Agent iteration {}/{} (~{}K ctx, {}, ~{:.0}¢)",
            iteration + 1,
            max_iterations,
            ctx_estimate / 1000,
            cost.summary(),
            cost.total_cents()
        );

        if cost.over_budget() {
            tracing::warn!(
                "Cost cap reached: {:.1}¢ > {}¢ limit. Requesting final synthesis.",
                cost.total_cents(),
                max_cost_cents
            );
            // Give the model one last chance to synthesize before stopping
            if findings_json.is_none() {
                messages.push(Message {
                    role: "user".to_string(),
                    content: json!("BUDGET EXCEEDED. You must produce your final structured findings JSON NOW with whatever you have. No more tool calls."),
                });
                let body = json!({
                    "model": config.model,
                    "max_tokens": 4096,
                    "system": SYSTEM_PROMPT,
                    "messages": messages,
                });
                if let Ok(resp) =
                    api::call_with_retry(&client, &config.anthropic_api_key, &body, &retry_config)
                        .await
                {
                    for block in &resp.content {
                        if let api::ContentBlock::Text { text } = block {
                            full_output.push_str(text);
                            full_output.push('\n');
                            if let Some(parsed) = try_extract_findings(text) {
                                findings_json = Some(parsed);
                            }
                        }
                    }
                }
            }
            break;
        }

        let body = json!({
            "model": config.model,
            "max_tokens": 4096,
            "system": SYSTEM_PROMPT,
            "tools": tool_defs,
            "messages": messages,
        });

        let api_resp =
            api::call_with_retry(&client, &config.anthropic_api_key, &body, &retry_config).await?;

        if let Some(ref usage) = api_resp.usage {
            cost.track_main(usage);
        }

        // Process response blocks
        let mut assistant_content: Vec<Value> = Vec::new();
        let mut tool_results: Vec<Value> = Vec::new();
        let mut iteration_text = String::new();

        for block in &api_resp.content {
            match block {
                api::ContentBlock::Text { text } => {
                    for line in text.lines() {
                        if line.starts_with('[') && line.contains(']') {
                            eprintln!("  {line}");
                        }
                    }
                    full_output.push_str(text);
                    full_output.push('\n');
                    iteration_text.push_str(text);

                    assistant_content.push(json!({"type": "text", "text": text}));

                    if let Some(parsed) = try_extract_findings(text) {
                        findings_json = Some(parsed);
                    }
                }
                api::ContentBlock::ToolUse { id, name, input } => {
                    eprintln!("  [TOOL] {name}: {}", summarize_input(input));

                    assistant_content.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input,
                    }));

                    let output = execute_tool(&client, config, name, input, &retry_config).await;

                    // Track nested API usage (web search)
                    if let Some(ref usage) = output.nested_usage {
                        cost.track_nested(usage);
                    }

                    // Annotate raw output for KB influence tracking
                    if name == "check_knowledge" {
                        let query_str = input["query"].as_str().unwrap_or("?");
                        let has_results = !output.text.contains("0 results")
                            && !output.text.contains("unavailable")
                            && !output.text.contains("error");
                        full_output.push_str(&format!(
                            "\n[KB_QUERY] \"{}\"\n[KB_RESULT] {}\n",
                            query_str,
                            if has_results {
                                format!("{} chars", output.text.len())
                            } else {
                                "empty".to_string()
                            }
                        ));
                    }

                    let truncated = truncate_tool_result(&output.text, name);

                    tool_results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": truncated,
                    }));
                }
            }
        }

        // Phase transition detection and context restructuring
        if let Some(new_phase) = detect_phase(&iteration_text) {
            if current_phase.is_some() && current_phase != Some(new_phase) {
                let old_phase = current_phase.unwrap_or("?");
                tracing::info!("Phase transition: {} → {}", old_phase, new_phase);

                let prior_content = restructure_context(&messages);
                let old_tokens = messages
                    .iter()
                    .map(|m| m.content.to_string().len())
                    .sum::<usize>()
                    / 4;
                let new_tokens = prior_content.len() / 4;

                tracing::info!(
                    "Restructured: ~{} → ~{} tokens ({} lines)",
                    old_tokens,
                    new_tokens + 50,
                    prior_content.lines().count()
                );

                messages = vec![Message {
                    role: "user".to_string(),
                    content: json!(format!(
                        "Research directive: {directive}\n\n\
                         Previous phases ({old_phase}) produced:\n\n\
                         {prior_content}\n\n\
                         Continue with [{new_phase}] phase."
                    )),
                }];
            }
            current_phase = Some(new_phase);
        }

        // Add assistant message
        messages.push(Message {
            role: "assistant".to_string(),
            content: json!(assistant_content),
        });

        // Flush partial output for crash recovery (atomic: write tmp then rename)
        if let Some(ref path) = config.output_path {
            let tmp = path.with_extension("tmp");
            if tokio::fs::write(&tmp, &full_output).await.is_ok() {
                let _ = tokio::fs::rename(&tmp, path).await;
            }
        }

        // Tool calls: add results and continue
        if !tool_results.is_empty() {
            consecutive_no_tool_iters = 0;
            messages.push(Message {
                role: "user".to_string(),
                content: json!(tool_results),
            });
            continue;
        }

        // Circuit breaker
        consecutive_no_tool_iters += 1;

        if api_resp.stop_reason.as_deref() == Some("end_turn") {
            if findings_json.is_some() {
                break;
            }
            if consecutive_no_tool_iters >= 2 {
                tracing::info!(
                    "Circuit breaker: {} no-tool iterations. Forcing synthesis.",
                    consecutive_no_tool_iters
                );
            }
            messages.push(Message {
                role: "user".to_string(),
                content: json!("Please produce your final structured findings JSON now."),
            });
        }
    }

    tracing::info!("Run complete: {}", cost.summary());

    Ok(AgentResult {
        raw_output: full_output,
        findings_json,
    })
}

fn try_extract_findings(text: &str) -> Option<Value> {
    let start = text.find("```json")?;
    let json_start = start + 7;
    let end = text[json_start..].find("```")?;
    let json_str = text[json_start..json_start + end].trim();
    serde_json::from_str(json_str).ok()
}

fn summarize_input(input: &Value) -> String {
    if let Some(q) = input["query"].as_str() {
        format!("\"{}\"", q.chars().take(60).collect::<String>())
    } else if let Some(u) = input["url"].as_str() {
        u.chars().take(60).collect()
    } else {
        input.to_string().chars().take(60).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- detect_phase ---

    #[test]
    fn detect_phase_single() {
        assert_eq!(detect_phase("[PLAN] decompose the question"), Some("PLAN"));
    }

    #[test]
    fn detect_phase_multiple_returns_last() {
        let text = "[PLAN] first\n[SURVEY] second\n[DEEPEN] third";
        assert_eq!(detect_phase(text), Some("DEEPEN"));
    }

    #[test]
    fn detect_phase_none() {
        assert_eq!(detect_phase("no markers here"), None);
    }

    #[test]
    fn detect_phase_unknown_marker() {
        assert_eq!(detect_phase("[BANANA] not a phase"), None);
    }

    #[test]
    fn detect_phase_mixed_known_unknown() {
        assert_eq!(
            detect_phase("[BANANA] nope [CHALLENGE] real"),
            Some("CHALLENGE")
        );
    }

    // --- restructure_context ---

    #[test]
    fn restructure_context_empty() {
        assert_eq!(restructure_context(&[]), "No content from this phase.");
    }

    #[test]
    fn restructure_context_tool_results_and_text() {
        let messages = vec![
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type": "tool_result", "tool_use_id": "1", "content": "tool output A"},
                    {"type": "tool_result", "tool_use_id": "2", "content": "tool output B"}
                ]),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type": "text", "text": "assistant analysis"},
                    {"type": "tool_use", "id": "3", "name": "research", "input": {}}
                ]),
            },
        ];
        let result = restructure_context(&messages);
        assert!(result.contains("tool output A"));
        assert!(result.contains("tool output B"));
        assert!(result.contains("assistant analysis"));
        // tool_use blocks should NOT appear in output
        assert!(!result.contains("research"));
    }

    #[test]
    fn restructure_context_ignores_plain_string_content() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: json!("plain string directive"),
        }];
        assert_eq!(
            restructure_context(&messages),
            "No content from this phase."
        );
    }

    // --- try_extract_findings ---

    #[test]
    fn try_extract_findings_valid() {
        let text =
            "Here are findings:\n```json\n{\"status\": \"complete\", \"findings\": []}\n```\nDone.";
        let result = try_extract_findings(text).unwrap();
        assert_eq!(result["status"], "complete");
    }

    #[test]
    fn try_extract_findings_no_block() {
        assert!(try_extract_findings("no json here").is_none());
    }

    #[test]
    fn try_extract_findings_malformed_json() {
        let text = "```json\n{not valid json\n```";
        assert!(try_extract_findings(text).is_none());
    }

    #[test]
    fn try_extract_findings_empty_block() {
        let text = "```json\n\n```";
        assert!(try_extract_findings(text).is_none());
    }

    // --- truncate_tool_result ---

    #[test]
    fn truncate_under_limit_returns_as_is() {
        let short = "Hello world.";
        assert_eq!(truncate_tool_result(short, "research"), short);
    }

    #[test]
    fn truncate_over_limit_sentence_boundary() {
        // Build a string that exceeds MAX_TOOL_RESULT_CHARS with sentence boundaries
        let sentence = "This is a sentence. ";
        let text: String = sentence.repeat(200); // 200 * 20 = 4000 chars
        let result = truncate_tool_result(&text, "research");
        assert!(result.contains("[Truncated from"));
        assert!(result.ends_with(&format!("[Truncated from {} chars]", text.len())));
        // Should end at a sentence boundary (period) before the truncation notice
        let before_notice = result.split("\n\n[Truncated").next().unwrap();
        assert!(before_notice.ends_with('.'));
    }

    #[test]
    fn truncate_read_page_bypasses() {
        let long_text: String = "x".repeat(5000);
        let result = truncate_tool_result(&long_text, "read_page");
        assert_eq!(result, long_text); // returned as-is
    }

    // --- summarize_input ---

    #[test]
    fn summarize_input_query() {
        let input = json!({"query": "what is AI"});
        assert_eq!(summarize_input(&input), "\"what is AI\"");
    }

    #[test]
    fn summarize_input_url() {
        let input = json!({"url": "https://example.com/page"});
        assert_eq!(summarize_input(&input), "https://example.com/page");
    }

    #[test]
    fn summarize_input_other() {
        let input = json!({"foo": "bar"});
        let result = summarize_input(&input);
        assert!(result.contains("foo"));
    }

    #[test]
    fn summarize_input_long_query_truncated() {
        let long_query = "a".repeat(100);
        let input = json!({"query": long_query});
        let result = summarize_input(&input);
        // 60 chars of query + quotes
        assert_eq!(result.len(), 62);
    }

    // --- Pricing::for_model ---

    #[test]
    fn pricing_haiku() {
        let p = Pricing::for_model("claude-3-haiku-20240307");
        assert!((p.input - 0.00008).abs() < f64::EPSILON);
        assert!((p.output - 0.0004).abs() < f64::EPSILON);
    }

    #[test]
    fn pricing_opus() {
        let p = Pricing::for_model("claude-opus-4-20250514");
        assert!((p.input - 0.0015).abs() < f64::EPSILON);
    }

    #[test]
    fn pricing_sonnet_default() {
        let p = Pricing::for_model("claude-sonnet-4-20250514");
        assert!((p.input - 0.0003).abs() < f64::EPSILON);
    }

    #[test]
    fn pricing_unknown_defaults_to_sonnet() {
        let p = Pricing::for_model("some-unknown-model");
        assert!((p.input - 0.0003).abs() < f64::EPSILON);
        assert!((p.output - 0.0015).abs() < f64::EPSILON);
    }

    #[test]
    fn pricing_case_insensitive() {
        let p = Pricing::for_model("Claude-3-HAIKU");
        assert!((p.input - 0.00008).abs() < f64::EPSILON);
    }

    // --- CostTracker ---

    #[test]
    fn cost_tracker_initial_zero() {
        let ct = CostTracker::new("claude-sonnet-4-20250514", "claude-3-haiku-20240307", 100);
        assert!((ct.total_cents() - 0.0).abs() < f64::EPSILON);
        assert!(!ct.over_budget());
    }

    #[test]
    fn cost_tracker_tracks_main() {
        let mut ct = CostTracker::new("claude-sonnet-4-20250514", "claude-3-haiku-20240307", 100);
        ct.track_main(&api::Usage {
            input_tokens: 1000,
            output_tokens: 500,
        });
        assert!(ct.total_cents() > 0.0);
        assert_eq!(ct.main_usage.input_tokens, 1000);
        assert_eq!(ct.main_usage.output_tokens, 500);
    }

    #[test]
    fn cost_tracker_tracks_nested() {
        let mut ct = CostTracker::new("claude-sonnet-4-20250514", "claude-3-haiku-20240307", 100);
        ct.track_nested(&api::Usage {
            input_tokens: 2000,
            output_tokens: 1000,
        });
        assert_eq!(ct.nested_usage.input_tokens, 2000);
        assert_eq!(ct.nested_usage.output_tokens, 1000);
    }

    #[test]
    fn cost_tracker_accumulates() {
        let mut ct = CostTracker::new("claude-sonnet-4-20250514", "claude-3-haiku-20240307", 100);
        ct.track_main(&api::Usage {
            input_tokens: 100,
            output_tokens: 50,
        });
        ct.track_main(&api::Usage {
            input_tokens: 200,
            output_tokens: 100,
        });
        assert_eq!(ct.main_usage.input_tokens, 300);
        assert_eq!(ct.main_usage.output_tokens, 150);
    }

    #[test]
    fn cost_tracker_over_budget() {
        let mut ct = CostTracker::new("claude-opus-4-20250514", "claude-opus-4-20250514", 1);
        // Opus: 0.0015 per input token, 0.0075 per output token
        // 1000 input = 1.5 cents, already over 1 cent budget
        ct.track_main(&api::Usage {
            input_tokens: 1000,
            output_tokens: 0,
        });
        assert!(ct.over_budget());
    }

    #[test]
    fn cost_tracker_summary_format() {
        let mut ct = CostTracker::new("claude-sonnet-4-20250514", "claude-3-haiku-20240307", 100);
        ct.track_main(&api::Usage {
            input_tokens: 1000,
            output_tokens: 200,
        });
        ct.track_nested(&api::Usage {
            input_tokens: 500,
            output_tokens: 100,
        });
        let s = ct.summary();
        assert!(s.contains("main: 1000+200 tokens"));
        assert!(s.contains("nested: 500+100 tokens"));
        assert!(s.contains("total: $"));
    }

    // --- Pricing::cost_cents ---

    #[test]
    fn pricing_cost_cents_calculation() {
        let p = Pricing::for_model("claude-sonnet-4-20250514");
        // Sonnet: input=0.0003, output=0.0015
        let usage = api::Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
        };
        let cost = p.cost_cents(&usage);
        // 10000 * 0.0003 + 1000 * 0.0015 = 3.0 + 1.5 = 4.5
        assert!((cost - 4.5).abs() < 0.001);
    }
}
