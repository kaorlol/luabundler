mod bundler;

use std::env;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let minify = args.contains(&String::from("-m")) || args.contains(&String::from("--minify"));
    let no_process = args.contains(&String::from("-n")) || args.contains(&String::from("--no-process"));

    bundler::bundle("lua/test.lua", "lua/bundled.lua", minify, no_process).await.unwrap();
}