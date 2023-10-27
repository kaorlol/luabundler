use std::{
    error::Error,
    path::PathBuf
};

use tokio::{
    fs::{
        read_to_string,
        File
    },
    io::copy,
    time::Instant
};

use darklua_core::{
    Configuration,
    GeneratorParameters,
    Resources,
    Options
};

use colored::Colorize;
use stacker::maybe_grow;
use regex::Regex;

const REQUIRE_PATTERNS: &[&str] = &[
    // Same as below, but with quotes
    r#"/*['"]require\s*\(\\*['"](.*?)\\*['"]\s*(?:,\s*(.*?))?\)\s*;?/*['"]\s*;?"#,
    r#"/*['"]require\s*\\*['"](.*?)\\*['"]\s*;?/*['"]\s*;?"#,

    // require("module.lua",...)
    r#"require\s*\(\\*['"](.*?)\\*['"]\s*(?:,\s*(.*?))?\)\s*;?"#,

    // require"module.lua"
    r#"require\s*\\*['"](.*?)\\*['"]\s*;?"#,
];

const COMMENT_PATTERN: &str = r#"(--\[\[.*?\]\])|(--[^\n]*)|(\/\*.*?\*\/)|(\[\[.*?\]\])"#;

// Recursively parses a file for require calls, and returns a vector of (require, args) tuples
#[async_recursion::async_recursion]
async fn parse_file(path: &str) -> Result<Vec<(String, String, String)>, Box<dyn Error>> {
    let contents = read_to_string(path).await?;
    let mut calls = Vec::new();

    for pattern in REQUIRE_PATTERNS {
        let regex = Regex::new(pattern)?;
        for cap in regex.captures_iter(&contents) {
            // Check if there is a Lua comment preceding the require statement
            let comment_regex = Regex::new(COMMENT_PATTERN)?;
            let start_index = cap.get(0).unwrap().start();
            let preceding_text = &contents[..start_index];
            
            if comment_regex.is_match(preceding_text) {
                continue; // Skip the require statement if it's within a comment
            }
            
            let matched = cap.get(0).unwrap().as_str().to_string();
            let require = cap.get(1).unwrap().as_str().trim().to_string();
            let args = cap.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();

            let mut require_path = PathBuf::from(path);
            require_path.pop();
            require_path.push(&require);

            calls.push((matched, require, args));

            // Recursively parse the require file
            if require_path.exists() {
                calls.append(&mut parse_file(require_path.to_str().unwrap()).await?);
            }
        }
    }

    Ok(calls)
}

// Replaces all require calls in a file with the contents of the file at the given path
async fn replace_requires(origin: &str, requires: Vec<(String, String, String)>) -> Result<String, Box<dyn Error>> {
    let origin_buf = PathBuf::from(origin);
    let main_dir = origin_buf.parent().unwrap().to_str().unwrap();
    let mut replaced_contents = read_to_string(&origin_buf).await?;  // Initialize with the original content

    for (mut matched, require, args) in requires {
        let require_path = PathBuf::from(main_dir).join(&require);
        let contents = read_to_string(&require_path).await?;

        // Check if the require statement ends with a semicolon, and remove it if it does
        let ends_with_semicolon = matched.ends_with(';');
        if ends_with_semicolon {
            matched.pop();
        }

        // Check if the first and last characters are either ' or "
        if matched.starts_with('"') && matched.ends_with('"') || matched.starts_with('\'') && matched.ends_with('\'') {
            // Check if the string ends with a semicolon
            let last_char_index = matched.len() - 1;

            // Replace the first and last characters with [[ and ]]
            let mut replaced = String::from("[[");
            replaced.push_str(&matched[1..matched.len() - 1]);
            replaced.push_str("]]");

            // Replace the matched string in the contents
            replaced_contents = replaced_contents.replace(&matched, &replaced);

            // Remove the first and last string in matched
            matched.remove(0);
            matched.remove(last_char_index - 1);
        }

        // Wrap the contents in a function call with the require arguments as parameters
        let mut replaced = format!("(function({})\n{}\nend)({});", args, contents, args);

        // If the require call was multiline, indent the contents of the required file
        if matched.contains("\n") {
            replaced = replaced.lines().map(|line| format!("    {}", line)).collect::<Vec<String>>().join("\n");
        }

        // Replace the matched require statement with the contents and accumulate in the result
        replaced_contents = replaced_contents.replace(&matched, &replaced);
    }

    Ok(replaced_contents)
}

fn process_code(buffer: PathBuf, minify: bool) {
    let resources = Resources::from_file_system();
    let generator_parameters = if minify {
        GeneratorParameters::default_dense()
    } else {
        GeneratorParameters::default_readable()
    };

    let configuration = Configuration::empty().with_generator(generator_parameters);
    let process_options = Options::new(buffer.clone()).with_output(buffer).with_configuration(configuration);

    maybe_grow(1024 * 1024, 32 * 1024 * 1024, || {
        darklua_core::process(&resources, process_options);
    });
}

pub async fn bundle(main_path: &str, bundle_path: &str, _minify: bool, noprocess: bool) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();

    let calls = parse_file(main_path).await?;
    let bundled = replace_requires(main_path, calls).await?;

    // Write the bundled code to the output file
    let mut file = File::create(bundle_path).await?;
    copy(&mut bundled.as_bytes(), &mut file).await?;

    println!("{}", format!("{} {} {}", "Bundled".blue(), main_path, format!("in {:?}", start.elapsed()).dimmed()));

    if !noprocess {
        let start = Instant::now();
        process_code(PathBuf::from(bundle_path), _minify);
        println!("{}", format!("{} {} {}", "Processed".blue(), bundle_path, format!("in {:?}", start.elapsed()).dimmed()));
    }

    Ok(())
}
