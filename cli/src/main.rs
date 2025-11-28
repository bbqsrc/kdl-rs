use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use kdl::schema_v2::validate_with_directives;
use kdl::{KdlDiagnostic, KdlDocument, KdlError};
use miette::{Diagnostic, NamedSource, Report, Severity};

#[derive(Parser)]
#[command(name = "kdl", version, about = "KDL document utilities")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate KDL document(s)
    Check {
        /// Files to check (use '-' for stdin)
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Format KDL document(s)
    Fmt {
        /// File to format (default: stdin)
        #[arg(default_value = "-")]
        file: PathBuf,

        /// Output file (use '-' for stdout). If not specified, replaces the input file.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Check { files } => run_check(&files),
        Command::Fmt { file, output } => run_fmt(&file, output.as_deref()),
    };

    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run_check(files: &[PathBuf]) -> Result<(), ()> {
    let mut has_errors = false;

    for file in files {
        let (content, source_name, file_dir) = match read_input(file) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Error reading {}: {}", file.display(), e);
                has_errors = true;
                continue;
            }
        };

        match content.parse::<KdlDocument>() {
            Ok(doc) => {
                // Validate against @ksl:schema directives if present
                if let Some(dir) = file_dir.as_ref() {
                    let result = validate_with_directives(&doc, dir);
                    for diag in result.diagnostics {
                        print_diagnostic(&diag, &source_name, &content);
                        if diag.severity == Severity::Error {
                            has_errors = true;
                        }
                    }
                }
            }
            Err(e) => {
                print_kdl_error(&e, &source_name);
                has_errors = true;
            }
        }
    }

    if has_errors {
        Err(())
    } else {
        Ok(())
    }
}

fn run_fmt(file: &Path, output: Option<&Path>) -> Result<(), ()> {
    let is_stdin = file.as_os_str() == "-";

    let (content, source_name, _file_dir) = match read_input(file) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", file.display(), e);
            return Err(());
        }
    };

    let mut doc: KdlDocument = match content.parse() {
        Ok(doc) => doc,
        Err(e) => {
            print_kdl_error(&e, &source_name);
            return Err(());
        }
    };

    doc.autoformat();
    let formatted = doc.to_string();

    // Determine output destination
    let output_path = match output {
        Some(p) if p.as_os_str() == "-" => None, // stdout
        Some(p) => Some(p),                      // explicit output file
        None if is_stdin => None,                // stdin without -o -> stdout
        None => Some(file),                      // default: replace input file
    };

    match output_path {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &formatted) {
                eprintln!("Error writing {}: {}", path.display(), e);
                return Err(());
            }
        }
        None => {
            print!("{}", formatted);
        }
    }

    Ok(())
}

fn read_input(path: &Path) -> io::Result<(String, String, Option<PathBuf>)> {
    if path.as_os_str() == "-" {
        let mut content = String::new();
        io::stdin().read_to_string(&mut content)?;
        Ok((content, "<stdin>".to_string(), None))
    } else {
        let content = std::fs::read_to_string(path)?;
        let source_name = path.display().to_string();
        let file_dir = path.parent().map(|p| p.to_path_buf());
        Ok((content, source_name, file_dir))
    }
}

fn print_kdl_error(error: &KdlError, source_name: &str) {
    let report = Report::new(error.clone())
        .with_source_code(NamedSource::new(source_name, (*error.input).clone()));
    eprintln!("{:?}", report);
}

fn print_diagnostic(diag: &KdlDiagnostic, source_name: &str, content: &str) {
    let msg = diag.message.as_deref().unwrap_or("validation error");
    let label = diag.label.as_deref();
    let help = diag.help.as_deref();

    #[derive(Debug)]
    struct ValidationDiag {
        message: String,
        severity: Severity,
        span: miette::SourceSpan,
        label: Option<String>,
        help: Option<String>,
    }

    impl std::fmt::Display for ValidationDiag {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl std::error::Error for ValidationDiag {}

    impl Diagnostic for ValidationDiag {
        fn severity(&self) -> Option<Severity> {
            Some(self.severity)
        }

        fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
            self.label.as_ref().map(|l| {
                Box::new(std::iter::once(miette::LabeledSpan::new_with_span(
                    Some(l.clone()),
                    self.span,
                ))) as Box<dyn Iterator<Item = miette::LabeledSpan>>
            })
        }

        fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
            self.help
                .as_ref()
                .map(|h| Box::new(h.as_str()) as Box<dyn std::fmt::Display>)
        }
    }

    let validation_diag = ValidationDiag {
        message: msg.to_string(),
        severity: diag.severity,
        span: diag.span,
        label: label.map(|s| s.to_string()),
        help: help.map(|s| s.to_string()),
    };

    let report = Report::new(validation_diag)
        .with_source_code(NamedSource::new(source_name, content.to_string()));
    eprintln!("{:?}", report);
}
