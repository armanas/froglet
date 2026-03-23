#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    froglet::operator::run().await
}
