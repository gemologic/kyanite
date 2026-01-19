# kyanite

A high-performance parallel command execution tool for Rust, inspired by GNU parallel and xargs. `kyanite` reads input lines from stdin and executes commands in parallel, with sophisticated template expansion capabilities and some other useful options.

## Features

- **Parallel Execution**: Leverages all CPU cores by default (configurable with `-j`)
- **Custom Placeholders**: Define your own placeholder (default: `{}`) for template expansion
- **Template Expansion**: Powerful substitution system with sed-like patterns, field access, and regex captures
- **Order Preservation**: Optional output ordering with `-k` flag
- **Graceful Shutdown**: Handles Ctrl+C gracefully and finishes running jobs
- **Dry Run Mode**: Preview commands with `-n` flag
- **Rich Error Handling**: Detailed error reporting for failed commands

## Quick Start

```bash
# Install from source
cargo build --release

# Basic usage with default placeholder {}
echo -e "file1.mp4\nfile2.mp4" | kyanite 'ffmpeg -i {} {s/.mp4/.mp3/g}'
echo -e "file1.mp4\nfile2.mp4" | kyanite 'ffmpeg -i {} {/(.*)\./1}.mp3'

# Parallel processing with custom worker count
ls *.txt | kyanite -j 4 'echo "processing: {}"'

# Keep original output order
ps aux | kyanite -k 'echo "PID: {2} CMD: {11+}"'

# Use custom placeholder []
ls *.mp4 | kyanite -I [] 'ffmpeg -i [] [s/.mp4/.mp3/g]'

# Use custom placeholder @
ps aux | kyanite -I @ 'echo "PID: @2@ CMD: @11+@"'
```

## Template System

Kyanite provides sophisticated template expansion that works with **any custom placeholder**:

| Template Pattern            | Description                                         | Example (with `{}`)        |
| --------------------------- | --------------------------------------------------- | -------------------------- |
| `{}`                        | Full input line                                     | `echo {}`                  |
| `{1}`, `{2}`, `{3}`         | Individual fields (space-delimited)                 | `echo "Field 1: {1}"`      |
| `{3+}`                      | Field 3 and all following                           | `echo "Args: {3+}"`        |
| `{3-}`                      | Fields 1 through 3                                  | `echo "First three: {3-}"` |
| `{s/p/r/f}`                 | Sed-like substitution (`g`=global, `i`=ignore case) | `{s/.mp4/.mp3/gi}`         |
| `{/regex/group}`            | Regex capture group                                 | `{/(.+)\\.(.+)/1}`         |

**Note:** Replace `PLACEHOLDER` with your custom placeholder string (default: `{}`).

### Custom Placeholders

You can define any placeholder using `-I` or `--input`:

| Placeholder | Example Usage                                    |
| ----------- | ------------------------------------------------ |
| `{}`        | `kyanite 'echo {1} {2}'` (default)             |
| `[]`        | `kyanite -I [] 'echo [1] [2]'`                  |
| `@`         | `kyanite -I @ 'echo @1@ @2@'`                   |
| `<>`        | `kyanite -I '<>' 'echo <1> <2>'`                |
| Custom      | `kyanite -I 'XXX' 'echo XXX1 XXX2'`             |

All template expansions work identically with any placeholder type!

## Configuration

- `-j, --jobs <N>`: Number of parallel workers (default: CPU count)
- `-k, --keep-order`: Preserve input order in output
- `-n, --dry-run`: Show commands without executing
- `-v, --verbose`: Detailed progress information
- `--max-jobs <N>`: Limit total jobs processed (0 = unlimited)
- `-I, --input <placeholder>`: Custom placeholder for template expansion (default: `{}`)
- `--field-separator <sep>`: Separator for field range operations (default: space)

## Examples

### Media Conversion

```bash
# Using default {}
ls *.mp4 | kyanite 'ffmpeg -i {} {s/.mp4/.mp3/g}'

# Using custom []
ls *.mp4 | kyanite -I [] 'ffmpeg -i [] [s/.mp4/.mp3/g]'
```

### Process Information

```bash
# Using default {}
ps aux | kyanite 'echo "PID: {2} User: {1} CPU: {3}"'

# Using custom @
ps aux | kyanite -I @ 'echo "PID: @2@ User: @1@ CPU: @3@"'
```

### File Processing with Regex

```bash
# Using default {}
ls | kyanite 'echo "{/(.+)\\.(.+)/1} has extension {/(.+)\\.(.+)/2}"'

# Using custom <>
ls | kyanite -I '<>' 'echo "< /(.+)\\.(.+)/1> has extension < /(.+)\\.(.+)/2>"'
```

### Batch Downloads

```bash
cat urls.txt | kyanite -j 8 'wget -O {/(.*)\\/.*/1} {}'
```

### CSV Field Extraction

```bash
# Using custom placeholder @ with comma separator
cat data.csv | kyanite -I @ --field-separator , 'echo "Name: @1@, Email: @2@"'
```

### Log Processing with Field Ranges

```bash
# Using [] to process log fields
cat access.log | kyanite -I [] 'echo "IP: [1] - Timestamp: [4]"'
```

## Performance

Built with Rust's performance guarantees:

- Static linking with LTO optimization
- Minimal runtime overhead
- Efficient thread pool management
- Zero-copy string operations where possible

## Development

```bash
# Build
cargo build --release

# Run tests
cargo test

# Development build
cargo build
```
