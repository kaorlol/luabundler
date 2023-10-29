use std::{
    error::Error,
    path::PathBuf,
    cmp::min
};

use tokio::{
    fs::{
        File,
        read_to_string
    },
    io::AsyncWriteExt,
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
use async_recursion::async_recursion;

const REQUIRE_PATTERNS: &[&str] = &[
    // require("module.lua",...) : 'require("module.lua",...)'
    r#"['"]?require\s*\(\\*['"](.*?)\\*['"]\s*(?:,\s*(.*?))?\)\s*;?\s*([.(].*)?['"]?"#,

    // require"module.lua" : 'require"module.lua"'
    r#"['"]?require\s*\\*['"](.*?)\\*['"]\s*;?['"]?"#,
];

// Matches strings: "string", 'string'
const IN_STRING_PATTERN: &str = r#"^['"](.+)['"]$"#;

// Matches comments: --, --[[ ]], --[=[ ]=]
const IN_COMMENT_PATTERN: &str = r#"--\[=*\[[\s\S]*?\]=*\]|['"]*--\s*.*['"]?"#;

// Removes comments from a file
async fn remove_comments(contents: &str) -> Result<String, Box<dyn Error>> {
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
async fn parse_file(path: &str) -> Result<Vec<(String, String, String, String)>, Box<dyn Error>> {
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

// Replaces all require calls in a file with the contents of the file at the given path
async fn replace_requires(origin: &str, requires: Vec<(String, String, String, String)>) -> Result<String, Box<dyn Error>> {
    let origin_buf = PathBuf::from(origin);
    let main_buf = PathBuf::from(origin_buf.parent().unwrap().to_str().unwrap());
    let mut replaced_contents = remove_comments(&read_to_string(&origin_buf).await?).await?;  // Initialize with the original content

    for (mut matched, require, args, func_args) in requires {
        let require_path = main_buf.join(&require);
        let contents = read_to_string(&require_path).await?;

        // Check if the first and last characters are either ' or "
        let in_string_regex = Regex::new(IN_STRING_PATTERN)?;
        if in_string_regex.is_match(&matched) {
            // Replace the first and last characters with [[ and ]]
            let replaced = format!("[[{}]]", &matched[1..matched.len() - 1]);
            replaced_contents = replaced_contents.replace(&matched, &replaced);

            // Remove the first and last string in matched
            matched.remove(0);
            matched.pop();
        }

        // Wrap the contents in a function call with the require arguments as parameters
        let mut replaced = format!("(function(...)\n{}\nend)({});", contents, args);

        if !func_args.is_empty() {
            // Remove the last semicolon and add func_args
            replaced.pop();
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


// Writes the data to the file in chunks of the given size
async fn write_in_chunks(file_path: &str, data: &[u8], chunk_size: usize) -> Result<(), Box<dyn Error>> {
    let mut file = File::create(file_path).await?;
    let mut offset = 0;

    while offset < data.len() {
        let end = min(offset + chunk_size, data.len()); // Calculate the end of the chunk
        file.write_all(&data[offset..end]).await?; // Write the chunk
        offset += chunk_size; // Increment the offset
    }

    Ok(())
}

// Bundles the given file and writes the bundled code to the output file
pub async fn bundle(main_path: &str, bundle_path: &str, _minify: bool, noprocess: bool) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();

    // Parse the main file for require calls and replace them with the contents of the required files
    let calls = parse_file(main_path).await?;
    let bundled = replace_requires(main_path, calls).await?;

    // Write the bundled code to the output file
    write_in_chunks(bundle_path, bundled.as_bytes(), 1024 * 1024).await?;

    println!("{}", format!("{} {} {}", "Bundled".blue(), main_path, format!("in {:?}", start.elapsed()).dimmed()));

    // Process the bundled code if the -n flag is not present
    if !noprocess {
        let start = Instant::now();
        process_code(PathBuf::from(bundle_path), _minify);
        println!("{}", format!("{} {} {}", "Processed".blue(), bundle_path, format!("in {:?}", start.elapsed()).dimmed()));
    }

    Ok(())
}
