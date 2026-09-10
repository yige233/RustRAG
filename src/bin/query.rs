use anyhow::{Context, Result, bail};
use clap::Parser;
use rustrag::config::Config;
use rustrag::db::Db;
use rustrag::embedder::Embedder;
use rustrag::embedder::api::ApiEmbedder;

/// Ad-hoc semantic search against a local RustRAG index.
#[derive(Parser, Debug)]
#[command(name = "rustrag-query", about = "Query a RustRAG vector index")]
struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.json")]
    config: String,

    /// Query string (required)
    #[arg(required = true)]
    query: Vec<String>,

    /// Number of results
    #[arg(short, long, default_value_t = 3)]
    top_k: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let query = cli.query.join(" ");

    let config = Config::load(&cli.config).context("Failed to load config")?;
    config.validate().context("Invalid configuration")?;

    let db = Db::open(&config.db_path, config.embedding.dimensions)
        .map_err(|e| anyhow::anyhow!("Failed to open database: {}", e))?;

    let embedder = ApiEmbedder::new(&config.embedding)
        .map_err(|e| anyhow::anyhow!("Failed to create embedder: {e}"))?;

    println!("Query: {query}");
    let emb = embedder
        .embed(&query)
        .map_err(|e| anyhow::anyhow!("Failed to embed query: {e}"))?;
    let results = db
        .search(&emb, cli.top_k)
        .map_err(|e| anyhow::anyhow!("Failed to search database: {e}"))?;

    if results.is_empty() {
        bail!("No results (index empty or query unmatched)");
    }

    for (i, r) in results.iter().enumerate() {
        let preview = r
            .chunk_content
            .chars()
            .take(120)
            .collect::<String>()
            .replace('\n', " ");
        println!(
            "   [{}] Score: {:.4} | File: {}",
            i + 1,
            r.similarity,
            r.document_name
        );
        println!("       -> {preview}");
    }

    Ok(())
}
