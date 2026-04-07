#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    froglet_marketplace::run().await
}
