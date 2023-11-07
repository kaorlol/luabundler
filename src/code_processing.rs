use darklua_core::{Configuration, GeneratorParameters, Resources, Options};
use stacker::maybe_grow;
use std::{path::PathBuf, error::Error};
use tokio::time::Instant;
use regex::Regex;
use colored::Colorize;
use crate::file_processing::{write_in_chunks, read_file};
use crate::require_parser::{parse_file, remove_comments, IN_STRING_PATTERN};

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
    maybe_grow(1024 * 1024, 1024 * 1024, || {
        darklua_core::process(&resources, process_options);
    });
}


// Replaces all require calls in a file with the contents of the file at the given path
async fn replace_requires(origin: &str, requires: Vec<(String, String, String, String)>) -> Result<String, Box<dyn Error>> {
    let origin_buf = PathBuf::from(origin);
    let main_buf = PathBuf::from(origin_buf.parent().unwrap().to_str().unwrap());
    let mut replaced_contents = remove_comments(&read_file(&origin_buf).await?).await?;  // Initialize with the original content

    for (mut matched, require, args, func_args) in requires {
        let require_path = main_buf.join(&require);
        let contents = read_file(&require_path).await?;

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
        let mut replaced = format!("(function(...)\n\t{}\nend)({});", contents, args);

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
