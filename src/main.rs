mod bundler;

#[tokio::main]
async fn main() {
    bundler::bundle("lua/test.lua", "lua/bundled.lua", true).await.unwrap();
}