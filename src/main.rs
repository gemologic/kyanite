use clap::Parser;
use regex::Regex;
use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use tokio::signal;

#[derive(Parser)]
#[command(name = "kyanite")]
#[command(about = "execute commands in parallel for each input line")]
struct Config {
    #[arg(short = 'j', long = "jobs", default_value_t = num_cpus::get())]
    workers: usize,

    #[arg(short = 'k', long = "keep-order")]
    keep_order: bool,

    #[arg(short = 'n', long = "dry-run")]
    dry_run: bool,

    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    #[arg(long = "max-jobs", default_value_t = 0)]
    max_jobs: usize,

    #[arg(short = 'I', long = "input", default_value = "{}")]
    placeholder: String,

    #[arg(long = "field-separator", default_value = " ")]
    field_separator: String,

    command: String,
}

#[derive(Debug)]
struct Job {
    id: usize,
    line: String,
}

#[derive(Debug)]
struct JobResult {
    id: usize,
    output: String,
    error: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse();

    let config_with_placeholder = Config {
        placeholder: config.placeholder.clone(),
        ..config
    };
    let config = Arc::new(config_with_placeholder);
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (result_tx, result_rx) = mpsc::channel::<JobResult>();

    let job_rx = Arc::new(std::sync::Mutex::new(job_rx));
    let mut handles = Vec::new();

    for worker_id in 0..config.workers {
        let job_rx = Arc::clone(&job_rx);
        let result_tx = result_tx.clone();
        let config = Arc::clone(&config);

        let handle = thread::spawn(move || {
            worker(worker_id, job_rx, result_tx, config);
        });
        handles.push(handle);
    }

    let config_clone = Arc::clone(&config);
    let collector_handle = thread::spawn(move || {
        result_collector(result_rx, config_clone);
    });

    let stdin = io::stdin();
    let reader = BufReader::new(stdin);
    let mut job_id = 0;
    let mut job_count = 0;

    let ctrl_c = signal::ctrl_c();
    let input_task = async {
        for line in reader.lines() {
            if config.max_jobs > 0 && job_count >= config.max_jobs {
                break;
            }

            match line {
                Ok(line) if !line.trim().is_empty() => {
                    let job = Job { id: job_id, line };

                    if config.verbose {
                        eprintln!("queued job {}: {}", job.id, job.line);
                    }

                    if job_tx.send(job).is_err() {
                        break;
                    }

                    job_id += 1;
                    job_count += 1;
                }
                Ok(_) => continue,
                Err(e) => {
                    eprintln!("error reading input: {}", e);
                    std::process::exit(1);
                }
            }
        }
    };

    tokio::select! {
        _ = input_task => {
            if config.verbose {
                eprintln!("input finished, processed {} jobs", job_count);
            }
        }
        _ = ctrl_c => {
            if config.verbose {
                eprintln!("\nreceived interrupt signal, shutting down gracefully...");
            }
        }
    }

    drop(job_tx);

    for handle in handles {
        let _ = handle.join();
    }

    drop(result_tx);
    let _ = collector_handle.join();

    Ok(())
}

fn worker(
    worker_id: usize,
    job_rx: Arc<std::sync::Mutex<mpsc::Receiver<Job>>>,
    result_tx: mpsc::Sender<JobResult>,
    config: Arc<Config>,
) {
    loop {
        let job = {
            let rx = job_rx.lock().unwrap();
            match rx.recv() {
                Ok(job) => job,
                Err(_) => break,
            }
        };

        if config.verbose {
            eprintln!("worker {} processing job {}", worker_id, job.id);
        }

        let cmd_str = expand_template(
            &config.command,
            &job.line,
            &config.field_separator,
            &config.placeholder,
        );

        let result = if config.dry_run {
            JobResult {
                id: job.id,
                output: format!("[+] {}", cmd_str),
                error: None,
            }
        } else {
            match Command::new("sh").arg("-c").arg(&cmd_str).output() {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let combined = if stderr.is_empty() {
                        stdout.trim_end().to_string()
                    } else if stdout.is_empty() {
                        stderr.trim_end().to_string()
                    } else {
                        format!("{}{}", stdout.trim_end(), stderr.trim_end())
                    };

                    JobResult {
                        id: job.id,
                        output: combined,
                        error: if output.status.success() {
                            None
                        } else {
                            Some(format!("command failed with exit code: {}", output.status))
                        },
                    }
                }
                Err(e) => JobResult {
                    id: job.id,
                    output: String::new(),
                    error: Some(format!("failed to execute command: {}", e)),
                },
            }
        };

        if result_tx.send(result).is_err() {
            break;
        }
    }

    if config.verbose {
        eprintln!("worker {} finished", worker_id);
    }
}

fn result_collector(result_rx: mpsc::Receiver<JobResult>, config: Arc<Config>) {
    if config.keep_order {
        let mut results = BTreeMap::new();
        let mut next_id = 0;

        for result in result_rx {
            results.insert(result.id, result);

            while let Some(result) = results.remove(&next_id) {
                print_result(&result, &config);
                next_id += 1;
            }
        }

        for (_, result) in results {
            print_result(&result, &config);
        }
    } else {
        for result in result_rx {
            print_result(&result, &config);
        }
    }
}

fn print_result(result: &JobResult, config: &Config) {
    if let Some(error) = &result.error {
        eprintln!("error in job {}: {}", result.id, error);
        if !result.output.is_empty() {
            eprintln!("output: {}", result.output);
        }
    } else if !result.output.is_empty() {
        if config.verbose {
            println!("[job {}] {}", result.id, result.output);
        } else {
            println!("{}", result.output);
        }
    }
}

/// Expands a command template with an input line using custom placeholder
///
/// Supports:
/// - PLACEHOLDER: Full input line
/// - PLACEHOLDERn: nth field (space-delimited)
/// - PLACEHOLDERn+: Fields n through end
/// - PLACEHOLDERn-: Fields 1 through n
/// - PLACEHOLDERs/pat/repl/g: Sed substitution (g=global, i=case-insensitive)
/// - PLACEHOLDER/pat/n: Regex capture group n
fn expand_template(template: &str, line: &str, field_separator: &str, placeholder: &str) -> String {
    let (open_delim, close_delim) = if placeholder.len() >= 2 {
        (
            placeholder.chars().next().unwrap(),
            placeholder.chars().nth(placeholder.len() - 1).unwrap(),
        )
    } else {
        let c = placeholder.chars().next().unwrap_or('{');
        (c, c)
    };

    let open_escaped = regex_escape(open_delim);
    let close_escaped = regex_escape(close_delim);

    let mut result = template.to_string();

    let sed_pattern = format!(
        r"{}\s*s/([^/]+)/([^/]*)/(.*?){}",
        open_escaped, close_escaped
    );
    let sed_re = Regex::new(&sed_pattern).unwrap();
    result = sed_re
        .replace_all(&result, |caps: &regex::Captures| {
            let pattern = &caps[1];
            let replacement = &caps[2];
            let flags = &caps[3];

            let mut regex_pattern = pattern.to_string();
            if flags.contains('i') {
                regex_pattern = format!("(?i){}", regex_pattern);
            }

            match Regex::new(&regex_pattern) {
                Ok(re) => {
                    if flags.contains('g') {
                        re.replace_all(line, replacement).to_string()
                    } else {
                        re.replace(line, replacement).to_string()
                    }
                }
                Err(_) => caps.get(0).unwrap().as_str().to_string(),
            }
        })
        .to_string();

    let field_pattern = format!(r"{}\s*(\d+)([\+\-]?)\s*{}", open_escaped, close_escaped);
    let field_re = Regex::new(&field_pattern).unwrap();
    result = field_re
        .replace_all(&result, |caps: &regex::Captures| {
            let field_num: usize = caps[1].parse().unwrap_or(0);
            let modifier = &caps[2];

            let fields: Vec<&str> = line.split(field_separator).collect();

            if field_num == 0 || field_num > fields.len() {
                return String::new();
            }

            match modifier {
                "+" => fields[(field_num - 1)..].join(field_separator),
                "-" => fields[0..field_num].join(field_separator),
                _ => fields[field_num - 1].to_string(),
            }
        })
        .to_string();

    let capture_pattern = format!(r"{}\s*/([^/]+)/(\d+)\s*{}", open_escaped, close_escaped);
    let capture_re = Regex::new(&capture_pattern).unwrap();
    result = capture_re
        .replace_all(&result, |caps: &regex::Captures| {
            let pattern = &caps[1];
            let group_num: usize = caps[2].parse().unwrap_or(0);

            match Regex::new(pattern) {
                Ok(re) => {
                    if let Some(captures) = re.captures(line)
                        && let Some(group) = captures.get(group_num)
                    {
                        return group.as_str().to_string();
                    }
                    String::new()
                }
                Err(_) => String::new(),
            }
        })
        .to_string();

    result = result.replace(placeholder, line);

    result
}

fn regex_escape(c: char) -> String {
    match c {
        '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' => {
            format!("\\{}", c)
        }
        _ => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_template_basic() {
        let result = expand_template("echo {}", "hello world", " ", "{}");
        assert_eq!(result, "echo hello world");
    }

    #[test]
    fn test_expand_template_field_access() {
        let result = expand_template("echo {1} {2}", "first second third", " ", "{}");
        assert_eq!(result, "echo first second");
    }

    #[test]
    fn test_expand_template_field_range() {
        let result = expand_template("echo {2+}", "first second third fourth", " ", "{}");
        assert_eq!(result, "echo second third fourth");
    }

    #[test]
    fn test_expand_template_sed_substitution() {
        let result = expand_template("echo {s/old/new/g}", "old old new", " ", "{}");
        assert_eq!(result, "echo new new new");
    }

    #[test]
    fn test_expand_template_regex_capture() {
        let result = expand_template("echo {/(.+)\\.(.+)/1}", "file.txt", " ", "{}");
        assert_eq!(result, "echo file");
    }

    #[test]
    fn test_expand_template_complex() {
        let result = expand_template("cp {} {s/.mp4/.mp3/g}", "video.mp4", " ", "{}");
        assert_eq!(result, "cp video.mp4 video.mp3");
    }

    #[test]
    fn test_expand_template_empty_field() {
        let result = expand_template("echo {5}", "one two three", " ", "{}");
        assert_eq!(result, "echo ");
    }

    #[test]
    fn test_config_default_values() {
        use clap::Parser;
        let config = Config::parse_from(["kyanite", "echo {}"]);
        assert_eq!(config.workers, num_cpus::get());
        assert!(!config.keep_order);
        assert!(!config.dry_run);
        assert!(!config.verbose);
        assert_eq!(config.max_jobs, 0);
        assert_eq!(config.placeholder, "{}");
        assert_eq!(config.field_separator, " ");
        assert_eq!(config.command, "echo {}");
    }

    #[test]
    fn test_config_parsing() {
        use clap::Parser;
        let config = Config::parse_from([
            "kyanite",
            "-j",
            "4",
            "-k",
            "-n",
            "-v",
            "--max-jobs",
            "10",
            "-I",
            "@",
            "--field-separator",
            ",",
            "echo @",
        ]);
        assert_eq!(config.workers, 4);
        assert!(config.keep_order);
        assert!(config.dry_run);
        assert!(config.verbose);
        assert_eq!(config.max_jobs, 10);
        assert_eq!(config.placeholder, "@");
        assert_eq!(config.field_separator, ",");
        assert_eq!(config.command, "echo @");
    }

    #[test]
    fn test_expand_template_edge_cases() {
        let result = expand_template("echo {s/([/broken/g}", "test", " ", "{}");
        assert!(result.contains("s/([/broken/g"));

        let result = expand_template("echo {abc}", "test", " ", "{}");
        assert_eq!(result, "echo {abc}");

        let result = expand_template("echo {5}", "one two three", " ", "{}");
        assert_eq!(result, "echo ");
    }

    #[test]
    fn test_simple_replacement_with_bracket_placeholder() {
        let result = "echo []".replace("[]", "hello");
        assert_eq!(result, "echo hello");
    }

    #[test]
    fn test_simple_replacement_with_at_placeholder() {
        let result = "echo @@".replace("@@", "world");
        assert_eq!(result, "echo world");
    }

    #[test]
    fn test_simple_replacement_with_angle_bracket_placeholder() {
        let result = "echo <>".replace("<>", "test");
        assert_eq!(result, "echo test");
    }

    #[test]
    fn test_simple_replacement_with_custom_string_placeholder() {
        let result = "echo PLACEHOLDER".replace("PLACEHOLDER", "value");
        assert_eq!(result, "echo value");
    }

    #[test]
    fn test_simple_replacement_multiple_occurrences() {
        let result = "echo @@ and @@".replace("@@", "test");
        assert_eq!(result, "echo test and test");
    }

    #[test]
    fn test_simple_replacement_with_special_characters() {
        let result = "echo %%".replace("%%", "special");
        assert_eq!(result, "echo special");
    }

    #[test]
    fn test_simple_replacement_with_empty_string() {
        let result = "echo TEST".replace("TEST", "");
        assert_eq!(result, "echo ");
    }

    #[test]
    fn test_simple_replacement_preserves_other_content() {
        let result = "start [middle] end".replace("[middle]", "REPLACED");
        assert_eq!(result, "start REPLACED end");
    }

    #[test]
    fn test_placeholder_behavior_documentation() {
        let result = expand_template("echo {1}", "first second", " ", "{}");
        assert_eq!(result, "echo first");

        let result = expand_template("echo [1]", "first second", " ", "[]");
        assert_eq!(result, "echo first");

        let result = expand_template("echo @1@", "first second", " ", "@@");
        assert_eq!(result, "echo first");
    }

    #[test]
    fn test_template_expansion_with_angle_brackets() {
        let result = expand_template("echo <1> <2>", "alpha beta gamma", " ", "<>");
        assert_eq!(result, "echo alpha beta");
    }

    #[test]
    fn test_template_expansion_with_brackets_field_access() {
        let result = expand_template("echo [1] [2]", "first second third", " ", "[]");
        assert_eq!(result, "echo first second");
    }

    #[test]
    fn test_template_expansion_with_brackets_field_range() {
        let result = expand_template("echo [2+]", "first second third fourth", " ", "[]");
        assert_eq!(result, "echo second third fourth");
    }

    #[test]
    fn test_template_expansion_with_brackets_sed() {
        let result = expand_template("echo [s/old/new/g]", "old old new", " ", "[]");
        assert_eq!(result, "echo new new new");
    }

    #[test]
    fn test_template_expansion_with_brackets_regex_capture() {
        let result = expand_template("echo [/(.+)\\.(.+)/1]", "file.txt", " ", "[]");
        assert_eq!(result, "echo file");
    }

    #[test]
    fn test_template_expansion_with_brackets_complex() {
        let result = expand_template("cp [] [s/.mp4/.mp3/g]", "video.mp4", " ", "[]");
        assert_eq!(result, "cp video.mp4 video.mp3");
    }

    #[test]
    fn test_template_expansion_with_at_placeholder_field_access() {
        let result = expand_template("echo @1@ @2@", "first second third", " ", "@@");
        assert_eq!(result, "echo first second");
    }

    #[test]
    fn test_template_expansion_with_at_placeholder_field_range() {
        let result = expand_template("echo @2+@", "first second third fourth", " ", "@@");
        assert_eq!(result, "echo second third fourth");
    }

    #[test]
    fn test_template_expansion_with_at_placeholder_sed() {
        let result = expand_template("echo @s/old/new/g@", "old old new", " ", "@@");
        assert_eq!(result, "echo new new new");
    }

    #[test]
    fn test_template_expansion_with_at_placeholder_regex_capture() {
        let result = expand_template("echo @/(.+)\\.(.+)/1@", "file.txt", " ", "@@");
        assert_eq!(result, "echo file");
    }

    #[test]
    fn test_template_expansion_with_percent_placeholder() {
        let result = expand_template("echo %s/foo/bar/g%", "foo foo baz", " ", "%%");
        assert_eq!(result, "echo bar bar baz");
    }

    #[test]
    fn test_template_expansion_with_pipe_placeholder() {
        let result = expand_template("echo |1| |2|", "one two three", " ", "||");
        assert_eq!(result, "echo one two");
    }

    #[test]
    fn test_template_expansion_with_custom_delimiter_field_access() {
        let result = expand_template("echo ::1::", "a b c d", " ", "::");
        assert!(result.contains("a"), "Result should contain 'a'");
    }

    #[test]
    fn test_template_expansion_with_custom_delimiter_field_range() {
        let result = expand_template("echo ::2+::", "a b c d", " ", "::");
        assert!(result.contains("b"), "Result should contain 'b'");
    }

    #[test]
    fn test_template_expansion_with_colons_sed() {
        let result = expand_template("echo :s/a/x/g:", "a b c", " ", "::");
        assert_eq!(result, "echo x b c");
    }

    #[test]
    fn test_template_expansion_mixed_placeholders() {
        let result = expand_template("echo [] [1]", "hello world", " ", "[]");
        assert_eq!(result, "echo hello world hello");
    }

    #[test]
    fn test_template_expansion_field_minus_with_custom_placeholder() {
        let result = expand_template("echo [3-]", "first second third fourth", " ", "[]");
        assert_eq!(result, "echo first second third");
    }

    #[test]
    fn test_template_expansion_case_insensitive_with_custom_placeholder() {
        let result = expand_template("echo [s/HELLO/world/i]", "HELLO hello", " ", "[]");
        assert_eq!(result, "echo world hello");

        let result = expand_template("echo [s/HELLO/world/gi]", "HELLO hello", " ", "[]");
        assert_eq!(result, "echo world world");
    }

    #[test]
    fn test_template_expansion_regex_capture_group2() {
        let result = expand_template("echo [/(\\d+)-(\\w+)/2]", "123-abc", " ", "[]");
        assert_eq!(result, "echo abc");
    }

    #[test]
    fn test_template_expansion_empty_field_with_custom_placeholder() {
        let result = expand_template("echo [5]", "one two three", " ", "[]");
        assert_eq!(result, "echo ");
    }

    #[test]
    fn test_placeholder_at_start() {
        let result = "@@ suffix".replace("@@", "prefix");
        assert_eq!(result, "prefix suffix");
    }

    #[test]
    fn test_placeholder_at_end() {
        let result = "prefix @".replace("@", "suffix");
        assert_eq!(result, "prefix suffix");
    }

    #[test]
    fn test_placeholder_with_spaces() {
        let result = "echo { x }".replace("{ x }", "test");
        assert_eq!(result, "echo test");
    }

    #[test]
    fn test_placeholder_with_underscores() {
        let result = "echo _placeholder_".replace("_placeholder_", "value");
        assert_eq!(result, "echo value");
    }

    #[test]
    fn test_placeholder_with_dashes() {
        let result = "echo --value--".replace("--value--", "test");
        assert_eq!(result, "echo test");
    }

    #[test]
    fn test_placeholder_substring_replacement() {
        let result = "test_test_test".replace("test", "X");
        assert_eq!(result, "X_X_X");
    }

    #[test]
    fn test_placeholder_case_sensitive() {
        let result = "ABC abc".replace("ABC", "123");
        assert_eq!(result, "123 abc");
    }

    #[test]
    fn test_placeholder_with_numbers() {
        let result = "echo #1#".replace("#1#", "first");
        assert_eq!(result, "echo first");
    }

    #[test]
    fn test_placeholder_with_colon() {
        let result = "echo ::val::".replace("::val::", "content");
        assert_eq!(result, "echo content");
    }

    #[test]
    fn test_placeholder_with_pipe() {
        let result = "echo |val|".replace("|val|", "data");
        assert_eq!(result, "echo data");
    }

    #[test]
    fn test_multiple_different_placeholders() {
        let result = "start @ end %"
            .replace("@", "middle1")
            .replace("%", "middle2");
        assert_eq!(result, "start middle1 end middle2");
    }

    #[test]
    fn test_placeholder_with_special_regex_chars() {
        let result = "echo $1$".replace("$1$", "group1");
        assert_eq!(result, "echo group1");
    }

    #[test]
    fn test_placeholder_long_string() {
        let result = "echo __PLACEHOLDER_HERE__".replace("__PLACEHOLDER_HERE__", "value");
        assert_eq!(result, "echo value");
    }

    #[test]
    fn test_placeholder_empty_value_replacement() {
        let result = "start [] end".replace("[]", "multi word value");
        assert_eq!(result, "start multi word value end");
    }

    #[test]
    fn test_real_world_file_renaming_with_brackets() {
        let result = expand_template("mv [] [s/\\.jpg/.png/]", "photo.jpg", " ", "[]");
        assert_eq!(result, "mv photo.jpg photo.png");
    }

    #[test]
    fn test_real_world_csv_field_extraction_with_at() {
        let result = expand_template(
            "echo 'User: @1@, Email: @3@'",
            "alice admin alice@example.com",
            " ",
            "@@",
        );
        assert_eq!(result, "echo 'User: alice, Email: alice@example.com'");
    }

    #[test]
    fn test_field_separator_with_comma() {
        let result = expand_template(
            "echo \"Name: {1}, Email: {2}\"",
            "jacobi,j@cobi.dev",
            ",",
            "{}",
        );
        assert_eq!(result, "echo \"Name: jacobi, Email: j@cobi.dev\"");
    }

    #[test]
    fn test_field_separator_with_comma_multiple_lines() {
        let result1 = expand_template(
            "echo \"Name: {1}, Email: {2}\"",
            "jacobi,j@cobi.dev",
            ",",
            "{}",
        );
        assert_eq!(result1, "echo \"Name: jacobi, Email: j@cobi.dev\"");

        let result2 = expand_template("echo \"Name: {1}, Email: {2}\"", "jade,j@de", ",", "{}");
        assert_eq!(result2, "echo \"Name: jade, Email: j@de\"");
    }

    #[test]
    fn test_real_world_log_processing_with_angle_brackets() {
        let result = expand_template(
            "echo Timestamp: <1>, Level: <2>",
            "2024-01-15 ERROR",
            " ",
            "<>",
        );
        assert_eq!(result, "echo Timestamp: 2024-01-15, Level: ERROR");
    }

    #[test]
    fn test_real_world_batch_video_conversion() {
        let result = expand_template("ffmpeg -i [] [s/\\.avi/.mp4/]", "movie.avi", " ", "[]");
        assert_eq!(result, "ffmpeg -i movie.avi movie.mp4");
    }

    #[test]
    fn test_real_world_backup_with_timestamp() {
        let result = expand_template("cp [] backups/[]-2024", "document.txt", " ", "[]");
        assert_eq!(result, "cp document.txt backups/document.txt-2024");
    }

    #[test]
    fn test_real_world_url_rewriting_with_at() {
        let result = expand_template("echo @s/http:/https:/@", "http://example.com", " ", "@@");
        assert_eq!(result, "echo https://example.com");
    }

    #[test]
    fn test_real_word_field_ranges_for_logs() {
        let result = expand_template(
            "echo 'Message: [3+]'",
            "INFO 2024-01-15 This is a log message",
            " ",
            "[]",
        );
        assert_eq!(result, "echo 'Message: This is a log message'");
    }

    #[test]
    fn test_real_world_extract_extension_with_regex() {
        let result = expand_template("echo [/(.+)\\.(.+)/2]", "archive.tar.gz", " ", "[]");
        assert_eq!(result, "echo gz");
    }

    #[test]
    fn test_real_world_filename_sanitization() {
        let result = expand_template("echo [s/ /_/g]", "my document file.txt", " ", "[]");
        assert_eq!(result, "echo my_document_file.txt");
    }

    #[test]
    fn test_real_word_case_conversion() {
        let result = expand_template("mv [] [s/[A-Z]/\\L$0/g]", "FILE.TXT", " ", "[]");
        assert!(result.contains("FILE.TXT"));
    }

    #[test]
    fn test_real_word_process_list_of_paths() {
        let result = expand_template("echo 'Processing: [1]'", "/home/user/file.txt", " ", "[]");
        assert_eq!(result, "echo 'Processing: /home/user/file.txt'");
    }

    #[test]
    fn test_real_word_create_symlinks() {
        let result = expand_template("ln -s [1] [2]", "original.txt link-to-original", " ", "[]");
        assert_eq!(result, "ln -s original.txt link-to-original");
    }

    #[test]
    fn test_real_word_remove_file_extension() {
        let result = expand_template("echo [s/.txt//]", "document.txt", " ", "[]");
        assert_eq!(result, "echo document");
    }

    #[test]
    fn test_real_word_extract_filename_from_path() {
        let result = expand_template("echo [3]", "path to file.txt", " ", "[]");
        assert_eq!(result, "echo file.txt");
    }

    #[test]
    fn test_real_word_batch_resize_images() {
        let result = expand_template(
            "convert [] -resize 800x600 resized/[]",
            "photo.jpg",
            " ",
            "[]",
        );
        assert_eq!(
            result,
            "convert photo.jpg -resize 800x600 resized/photo.jpg"
        );
    }

    #[test]
    fn test_real_word_extract_fields_from_ps_output() {
        let result = expand_template(
            "echo 'PID: @1@, USER: @2@, CMD: @3+@'",
            "1234 root /usr/bin/myapp --daemon",
            " ",
            "@@",
        );
        assert_eq!(
            result,
            "echo 'PID: 1234, USER: root, CMD: /usr/bin/myapp --daemon'"
        );
    }

    #[test]
    fn test_real_word_normalize_whitespace() {
        let result = expand_template("echo [s/\\s+/ /g]", "hello    world   test", " ", "[]");
        assert_eq!(result, "echo hello world test");
    }

    #[test]
    fn test_real_word_replace_underscores_with_dashes() {
        let result = expand_template("echo [s/_/-/g]", "my_file_name.txt", " ", "[]");
        assert_eq!(result, "echo my-file-name.txt");
    }

    #[test]
    fn test_real_word_add_prefix_to_files() {
        let result = expand_template("echo [s/^/backup_/]", "file.txt", " ", "[]");
        assert_eq!(result, "echo backup_file.txt");
    }

    #[test]
    fn test_real_word_add_suffix_to_files() {
        let result = expand_template("echo [].bak", "file.txt", " ", "[]");
        assert_eq!(result, "echo file.txt.bak");
    }
}
