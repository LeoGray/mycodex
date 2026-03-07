use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    mycodex::cli::run().await
}
