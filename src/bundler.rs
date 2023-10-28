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

// Regex on top. Just understand it tbh.
use regex::Regex;

const REQUIRE_PATTERNS: &[&str] = &[
    // require("module.lua",...) : 'require("module.lua",...)'
    r#"['"]?require\s*\(\\*['"](.*?)\\*['"]\s*(?:,\s*(.*?))?\)\s*;?\s*([.(].*)?['"]?"#,

    // require"module.lua" : 'require"module.lua"'
    r#"['"]?require\s*\\*['"](.*?)\\*['"]\s*;?['"]?"#,
];

// Matches strings: "string", 'string'
const IN_STRING_PATTERN: &str = r#"^['"](.+)['"]$"#;

// Chatgpted because im not doing allat :money_mouth: THANK YOU DADDY GPT :heart:
async fn remove_all_comments(contents: &str) -> Result<String, Box<dyn Error>> {
    let mut new_contents = String::new();
    let mut in_string = false;
    let mut in_multiline_comment = false;
    let mut in_singleline_comment = false;

    for (i, c) in contents.chars().enumerate() {
        if in_string {
            if c == '"' || c == '\'' {
                in_string = false;
            }
        } else if in_multiline_comment {
            if c == ']' {
                if contents.chars().nth(i + 1).unwrap_or_default() == ']' {
                    in_multiline_comment = false;
                }
            }
        } else if in_singleline_comment {
            if c == '\n' {
                in_singleline_comment = false;
            }
        } else {
            if c == '"' || c == '\'' {
                in_string = true;
            } else if c == '-' {
                if contents.chars().nth(i + 1).unwrap_or_default() == '-' {
                    in_singleline_comment = true;
                } else if contents.chars().nth(i + 1).unwrap_or_default() == '[' {
                    if contents.chars().nth(i + 2).unwrap_or_default() == '[' {
                        in_multiline_comment = true;
                    }
                }
            }
        }

        if !in_multiline_comment && !in_singleline_comment {
            new_contents.push(c);
        }
    }

    Ok(new_contents)
}

// Recursively parses a file for require calls, and returns a vector of (require, args) tuples
#[async_recursion::async_recursion]
async fn parse_file(path: &str) -> Result<Vec<(String, String, String, String)>, Box<dyn Error>> {
    let contents = remove_all_comments(read_to_string(path).await?.as_str()).await?;
    let mut calls = Vec::new();

    for pattern in REQUIRE_PATTERNS {
        let regex = Regex::new(pattern)?;
        for cap in regex.captures_iter(&contents) {
            let matched = cap.get(0).unwrap().as_str().trim().to_string();
            let require = cap.get(1).unwrap().as_str().trim().to_string();
            let args = cap.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
            let func_args = cap.get(3).map_or(String::new(), |m| m.as_str().trim().to_string());

            let mut require_path = PathBuf::from(path);
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

// Replaces all require calls in a file with the contents of the file at the given path
async fn replace_requires(origin: &str, requires: Vec<(String, String, String, String)>) -> Result<String, Box<dyn Error>> {
    let origin_buf = PathBuf::from(origin);
    let main_dir = origin_buf.parent().unwrap().to_str().unwrap();
    let mut replaced_contents = read_to_string(&origin_buf).await?;  // Initialize with the original content

    for (mut matched, require, args, func_args) in requires {
        let require_path = PathBuf::from(main_dir).join(&require);
        let contents = read_to_string(&require_path).await?;

        // Check if the first and last characters are either ' or "
        let in_string_regex = Regex::new(IN_STRING_PATTERN)?;
        if in_string_regex.is_match(&matched) {
            // Replace the first and last characters with [[ and ]]
            let mut replaced = String::from("[[");
            replaced.push_str(&matched[1..matched.len() - 1]);
            replaced.push_str("]]");

            replaced_contents = replaced_contents.replace(&matched, &replaced);

            // Remove the first and last string in matched
            matched.remove(0);
            matched.pop();
        }

        // Wrap the contents in a function call with the require arguments as parameters
        let mut replaced = format!("(function(...)\n{}\nend)({});", contents, args);

        if !func_args.is_empty() {
            replaced.pop(); // Remove the last semicolon
            replaced.push_str(&func_args);
        }

        // If the require call was multiline, indent the contents of the required file
        if matched.contains("\n") {
            replaced = replaced.lines().map(|line| format!("    {}", line)).collect::<Vec<String>>().join("\n");
        }

        // Replace the matched require statement with the contents and accumulate in the result
        replaced_contents = replaced_contents.replace(&matched, &replaced);
    }

    Ok(replaced_contents)
}

// Processes the code in the given buffer
fn process_code(buffer: PathBuf, minify: bool) {
    // Initialize the resources and parameters
    let resources = Resources::from_file_system();
    let generator_parameters = if minify {
        GeneratorParameters::default_dense()
    } else {
        GeneratorParameters::default_readable()
    };

    // Initialize the configuration and options
    let configuration = Configuration::empty().with_generator(generator_parameters);
    let process_options = Options::new(buffer.clone()).with_output(buffer).with_configuration(configuration);

    // Process the code (using stacker to prevent stack overflow)
    maybe_grow(1024 * 1024, 32 * 1024 * 1024, || {
        darklua_core::process(&resources, process_options);
    });
}

// Bundles the given file and writes the bundled code to the output file
pub async fn bundle(main_path: &str, bundle_path: &str, _minify: bool, noprocess: bool) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();

    // Parse the main file for require calls and replace them with the contents of the required files
    let calls = parse_file(main_path).await?;
    let bundled = replace_requires(main_path, calls).await?;

    // Write the bundled code to the output file
    let mut file = File::create(bundle_path).await?;
    copy(&mut bundled.as_bytes(), &mut file).await?;

    println!("{}", format!("{} {} {}", "Bundled".blue(), main_path, format!("in {:?}", start.elapsed()).dimmed()));

    // Process the bundled code if the -n flag is not present
    if !noprocess {
        let start = Instant::now();
        process_code(PathBuf::from(bundle_path), _minify);
        println!("{}", format!("{} {} {}", "Processed".blue(), bundle_path, format!("in {:?}", start.elapsed()).dimmed()));
    }

    Ok(())
}
