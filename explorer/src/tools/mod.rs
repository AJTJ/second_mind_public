pub mod knowledge;
pub mod papers;
pub mod reader;
pub mod web;

use serde_json::{Value, json};

/// Build the tools array for Claude's Messages API.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "research",
            "description": "Search the web for information on a topic. Returns a synthesized answer with citations. Use for current events, market data, industry reports.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "search_papers",
            "description": "Search academic papers via Semantic Scholar. Returns titles, abstracts, TLDRs, and citation counts. Use for scientific claims and research backing.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query for academic papers"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default 5)"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "read_page",
            "description": "Fetch and read a web page, returning its content as text. Use when you need to read a specific URL from citations or search results.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "check_knowledge",
            "description": "Search the existing knowledge base for what we already know about a topic. Use before searching externally to avoid duplicate research.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to search for in existing knowledge"
                    }
                },
                "required": ["query"]
            }
        }),
    ]
}
