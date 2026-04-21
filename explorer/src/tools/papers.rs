use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<Paper>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct Paper {
    #[serde(rename = "paperId")]
    paper_id: Option<String>,
    title: Option<String>,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
    year: Option<i32>,
    #[serde(rename = "citationCount")]
    citation_count: Option<i32>,
    url: Option<String>,
    tldr: Option<Tldr>,
}

#[derive(Deserialize)]
struct Tldr {
    text: Option<String>,
}

pub async fn search(client: &Client, query: &str, limit: Option<i32>) -> anyhow::Result<String> {
    let limit = limit.unwrap_or(5).min(20);

    let resp = client // client already has timeout from api::build_client()
        .get("https://api.semanticscholar.org/graph/v1/paper/search")
        .query(&[
            ("query", query),
            ("limit", &limit.to_string()),
            ("fields", "title,abstract,year,citationCount,url,tldr"),
        ])
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let err_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Semantic Scholar API error ({status}): {err_text}");
    }

    let parsed: SearchResponse = resp.json().await?;

    if parsed.data.is_empty() {
        return Ok("No papers found.".to_string());
    }

    let mut result = String::new();
    for (i, paper) in parsed.data.iter().enumerate() {
        let title = paper.title.as_deref().unwrap_or("(untitled)");
        let year = paper.year.map(|y| y.to_string()).unwrap_or_default();
        let citations = paper.citation_count.unwrap_or(0);
        let url = paper.url.as_deref().unwrap_or("");
        let tldr = paper
            .tldr
            .as_ref()
            .and_then(|t| t.text.as_deref())
            .unwrap_or("");
        let abstract_text = paper.abstract_text.as_deref().unwrap_or("");

        result.push_str(&format!(
            "{}. {} ({})\n   Citations: {}\n   URL: {}\n",
            i + 1,
            title,
            year,
            citations,
            url
        ));
        if !tldr.is_empty() {
            result.push_str(&format!("   TLDR: {}\n", tldr));
        } else if !abstract_text.is_empty() {
            let truncated: String = abstract_text.chars().take(300).collect();
            result.push_str(&format!("   Abstract: {}...\n", truncated));
        }
        result.push('\n');
    }

    Ok(result)
}
