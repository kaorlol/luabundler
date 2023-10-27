use std::{
    error::Error,
    path::PathBuf
};

use tokio::{
    fs::{
        read_to_string,
        File
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

const REQUIRE_PATTERNS: &[&str] = &[
    // require("module.lua",...)
    r#"require\s*\(\\*['"](.*?)\\*['"]\s*(?:,\s*(.*?))?\)"#,

    // require"module.lua"
    r#"require\s*\\*['"](.*?)\\*['"]"#,
];

// Recursively parses a file for require calls, and returns a vector of (require, args) tuples.
#[async_recursion::async_recursion]
async fn parse_file(path: &str) -> Result<Vec<(String, String, String)>, Box<dyn Error>> {
    let contents = read_to_string(path).await?;
    let mut calls = Vec::new();

    for pattern in REQUIRE_PATTERNS {
        let regex = Regex::new(pattern)?;
        for cap in regex.captures_iter(&contents) {
            let matched = cap.get(0).unwrap().as_str().to_string();
            let require = cap.get(1).unwrap().as_str().trim().to_string();
            let args = cap.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();

            let mut require_path = PathBuf::from(path);
            require_path.pop();
            require_path.push(&require);

            calls.push((matched, require, args));

            // Recursively parse the require file.
            if require_path.exists() {
                calls.append(&mut parse_file(require_path.to_str().unwrap()).await?);
            }
        }
    }

    Ok(calls)
}

// Replaces all require calls in a file with the contents of the file at the given path.
async fn replace_requires(origin: &str, requires: Vec<(String, String, String)>) -> Result<String, Box<dyn Error>> {
    let origin_buf = PathBuf::from(origin);
    let main_dir = origin_buf.parent().unwrap().to_str().unwrap();
    let mut replaced_contents = read_to_string(&origin_buf).await?;  // Initialize with the original content

    for (matched, require, args) in requires {
        let require_path = PathBuf::from(main_dir).join(&require);
        let contents = read_to_string(&require_path).await?;

        let mut replaced = format!("(function({})\n{}\nend)({});", args, contents, args);

        // If the require call was multiline, indent the contents of the required file.
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

pub async fn bundle(main_path: &str, bundle_path: &str, _minify: bool) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();

    let calls = parse_file(main_path).await?;
    let bundled = replace_requires(main_path, calls).await?;

    // Write the bundled code to the output file
    let mut file = File::create(bundle_path).await?;
    file.write_all(bundled.as_bytes()).await?;

    process_code(PathBuf::from(bundle_path), _minify);

    println!("{} {} {}", "Bundled".blue(), main_path, format!("in {:?}", start.elapsed()).dimmed());

    Ok(())
}
