use clap::Parser;
use regex::Regex;
use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader};
use std::process::{Command};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use tokio::signal;

#[derive(Parser)]
#[command(name = "kyanite")]
#[command(about = "Execute commands in parallel for each input line")]
struct Config {
    /// Number of parallel jobs
    #[arg(short = 'j', long = "jobs", default_value_t = num_cpus::get())]
    workers: usize,

    /// Keep output order
    #[arg(short = 'k', long = "keep-order")]
    keep_order: bool,

    /// Dry run - show commands without executing
    #[arg(short = 'n', long = "dry-run")]
    dry_run: bool,

    /// Verbose output
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Maximum number of jobs to process (0 = unlimited)
    #[arg(long = "max-jobs", default_value_t = 0)]
    max_jobs: usize,

    /// Placeholder for input line
    #[arg(short = 'I', long = "input", default_value = "{}")]
    placeholder: String,

    /// Field separator for output
    #[arg(long = "field-separator", default_value = " ")]
    field_separator: String,

    /// Command template to execute
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

    if config.command.is_empty() {
        print_usage();
        std::process::exit(1);
    }

    let config = Arc::new(config);
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let (result_tx, result_rx) = mpsc::channel::<JobResult>();

    // Spawn worker threads
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

    // Spawn result collector
    let config_clone = Arc::clone(&config);
    let collector_handle = thread::spawn(move || {
        result_collector(result_rx, config_clone);
    });

    // Read input and send jobs
    let stdin = io::stdin();
    let reader = BufReader::new(stdin);
    let mut job_id = 0;
    let mut job_count = 0;

    // Set up signal handling
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
                        eprintln!("Queued job {}: {}", job.id, job.line);
                    }

                    if job_tx.send(job).is_err() {
                        break; // Channel closed
                    }

                    job_id += 1;
                    job_count += 1;
                }
                Ok(_) => continue, // Skip empty lines
                Err(e) => {
                    eprintln!("Error reading input: {}", e);
                    std::process::exit(1);
                }
            }
        }
    };

    // Wait for either input completion or Ctrl+C
    tokio::select! {
        _ = input_task => {
            if config.verbose {
                eprintln!("Input finished, processed {} jobs", job_count);
            }
        }
        _ = ctrl_c => {
            if config.verbose {
                eprintln!("\nReceived interrupt signal, shutting down gracefully...");
            }
        }
    }

    // Signal workers to stop by dropping the sender
    drop(job_tx);

    // Wait for all workers to finish
    for handle in handles {
        let _ = handle.join();
    }

    // Signal result collector to stop
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
                Err(_) => break, // Channel closed
            }
        };

        if config.verbose {
            eprintln!("Worker {} processing job {}", worker_id, job.id);
        }

        let cmd_str = expand_template(&config.command, &job.line, &config.field_separator);

        let result = if config.dry_run {
            JobResult {
                id: job.id,
                output: format!("[+] {}", cmd_str),
                error: None,
            }
        } else {
            match Command::new("sh")
                .arg("-c")
                .arg(&cmd_str)
                .output()
            {
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
                        error: if output.status.success() { None } else { Some(format!("Command failed with exit code: {}", output.status)) },
                    }
                }
                Err(e) => JobResult {
                    id: job.id,
                    output: String::new(),
                    error: Some(format!("Failed to execute command: {}", e)),
                },
            }
        };

        if result_tx.send(result).is_err() {
            break; // Channel closed
        }
    }

    if config.verbose {
        eprintln!("Worker {} finished", worker_id);
    }
}

fn result_collector(result_rx: mpsc::Receiver<JobResult>, config: Arc<Config>) {
    if config.keep_order {
        let mut results = BTreeMap::new();
        let mut next_id = 0;

        for result in result_rx {
            results.insert(result.id, result);

            // Print all consecutive results starting from next_id
            while let Some(result) = results.remove(&next_id) {
                print_result(&result, &config);
                next_id += 1;
            }
        }

        // Print any remaining results (shouldn't happen with proper ordering)
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
        eprintln!("Error in job {}: {}", result.id, error);
        if !result.output.is_empty() {
            eprintln!("Output: {}", result.output);
        }
    } else if !result.output.is_empty() {
        if config.verbose {
            println!("[Job {}] {}", result.id, result.output);
        } else {
            println!("{}", result.output);
        }
    }
}

fn expand_template(template: &str, line: &str, field_separator: &str) -> String {
    let mut result = template.replace("{}", line);

    // Handle sed-like substitutions: {s/pattern/replacement/flags}
    let sed_re = Regex::new(r"\{s/([^/]+)/([^/]*)/([^}]*)\}").unwrap();
    result = sed_re.replace_all(&result, |caps: &regex::Captures| {
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
    }).to_string();

    // Handle field access: {1}, {2}, {3+}, {3-}
    let field_re = Regex::new(r"\{(\d+)([\+\-]?)\}").unwrap();
    result = field_re.replace_all(&result, |caps: &regex::Captures| {
        let field_num: usize = caps[1].parse().unwrap_or(0);
        let modifier = &caps[2];

        let fields: Vec<&str> = line.split_whitespace().collect();
        
        if field_num == 0 || field_num > fields.len() {
            return String::new();
        }

        match modifier {
            "+" => fields[(field_num - 1)..].join(field_separator),
            "-" => fields[0..field_num].join(field_separator),
            _ => fields[field_num - 1].to_string(),
        }
    }).to_string();

    // Handle regex captures: {/pattern/1}, {/pattern/2}
    let capture_re = Regex::new(r"\{/([^/]+)/(\d+)\}").unwrap();
    result = capture_re.replace_all(&result, |caps: &regex::Captures| {
        let pattern = &caps[1];
        let group_num: usize = caps[2].parse().unwrap_or(0);

        match Regex::new(pattern) {
            Ok(re) => {
                if let Some(captures) = re.captures(line) {
                    if let Some(group) = captures.get(group_num) {
                        return group.as_str().to_string();
                    }
                }
                String::new()
            }
            Err(_) => String::new(),
        }
    }).to_string();

    result
}

fn print_usage() {
    let bin_name = "kyanite";

    eprintln!("Usage: {} [options] 'command'", bin_name);
    eprintln!();
    eprintln!("Template expansions:");
    eprintln!("  {{}}              - Full line");
    eprintln!("  {{1}} {{2}} {{3}}     - Individual fields (space-delimited)");
    eprintln!("  {{3+}}            - Field 3 and all following");
    eprintln!("  {{s/pat/repl/g}}  - Sed-like substitution (g=global, i=ignore case)");
    eprintln!("  {{/regex/1}}      - Regex capture group 1");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  ls *.mp4 | {} 'ffmpeg -i {{}} {{s/.mp4/.mp3/g}}'", bin_name);
    eprintln!("  ps aux | {} 'echo \"PID: {{2}} CMD: {{11+}}\"'", bin_name);
    eprintln!("  ls | {} 'echo {{/(.+)\\.(.+)/1}} has extension {{/(.+)\\.(.+)/2}}'", bin_name);
}
