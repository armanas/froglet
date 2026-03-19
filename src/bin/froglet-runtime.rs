#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    froglet::server::run_runtime().await
}
