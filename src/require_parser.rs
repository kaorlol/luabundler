use async_recursion::async_recursion;
use regex::Regex;
use std::error::Error;
use std::path::PathBuf;
use tokio::fs::read_to_string;

const REQUIRE_PATTERNS: &[&str] = &[
    // require("module.lua",...) : 'require("module.lua",...)'
    r#"['"]?require\s*\(\\*['"](.*?)\\*['"]\s*(?:,\s*(.*?))?\)\s*;?\s*([.(].*)?['"]?"#,

    // require"module.lua" : 'require"module.lua"'
    r#"['"]?require\s*\\*['"](.*?)\\*['"]\s*;?['"]?"#,
];

// Matches strings: "string", 'string'
pub const IN_STRING_PATTERN: &str = r#"^['"](.+)['"]$"#;

// Matches comments: --, --[[ ]], --[=[ ]=]
const IN_COMMENT_PATTERN: &str = r#"--\[=*\[[\s\S]*?\]=*\]|['"]*--\s*.*['"]?"#;

// Removes comments from a file
pub async fn remove_comments(contents: &str) -> Result<String, Box<dyn Error>> {
    // Create a regex instance for the comment pattern
    let re: Regex = Regex::new(IN_COMMENT_PATTERN)?;
    let mut cleaned_contents = String::from(contents);

    // Replace all matched comments with an empty string
    for cap in re.captures_iter(contents) {
        // check if the comment is in a string, if so, don't remove it
        let matched = cap.get(0).unwrap().as_str().trim().to_string();
        let in_string_regex = Regex::new(IN_STRING_PATTERN)?;
        if !in_string_regex.is_match(&matched) {
            cleaned_contents = cleaned_contents.replace(&matched, "");
        }
    }

    Ok(cleaned_contents.into())
}

// Recursively parses a file for require calls, and returns a vector of (require, args) tuples
#[async_recursion]
pub async fn parse_file(path: &str) -> Result<Vec<(String, String, String, String)>, Box<dyn Error>> {
    let mut require_path = PathBuf::from(path);
    let contents = remove_comments(&read_to_string(require_path.clone()).await?).await?;
    let mut calls = Vec::new();

    for pattern in REQUIRE_PATTERNS {
        let regex = Regex::new(pattern)?;
        for cap in regex.captures_iter(&contents) {
            let matched = cap.get(0).unwrap().as_str().trim().to_string();
            let require = cap.get(1).unwrap().as_str().trim().to_string();
            let args = cap.get(2).map_or(String::new(), |m| m.as_str().trim().to_string());
            let func_args = cap.get(3).map_or(String::new(), |m| m.as_str().trim().to_string());

            require_path.pop();
            require_path.push(&require);

            calls.push((matched, require, args, func_args));

            // Recursively parse the require file and append the results to the vector
            if require_path.exists() {
                calls.append(&mut parse_file(require_path.to_str().unwrap()).await?);
            }
        }
    }

    Ok(calls)
}