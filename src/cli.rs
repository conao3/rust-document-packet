use std::{ffi::OsString, fs, path::PathBuf};

use clap::{ArgMatches, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::Value;

use crate::{
    Error, Result, apply_manifest, inspect_pdf, load_manifest, render_pdf, verify_manifest,
};

#[derive(Parser)]
#[command(
    name = "document-packet",
    version,
    about = "Build and verify PDF document packets",
    disable_help_subcommand = true,
    after_help = "Run `document-packet <command> help` or `document-packet <command> <subcommand> help` for details."
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Json, help = "Output format")]
    output: OutputFormat,
    #[arg(long, global = true, help = "Print debug logs to stderr")]
    debug: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Json,
    Text,
    Table,
}

#[derive(Subcommand)]
enum Commands {
    #[command(subcommand, about = "Inspect and render PDF files")]
    Pdf(PdfCommands),
    #[command(subcommand, about = "Apply and verify document-packet manifests")]
    Manifest(ManifestCommands),
}

#[derive(Subcommand)]
enum PdfCommands {
    #[command(about = "Report page count, MediaBox, rotation, size, and SHA-256 of a PDF")]
    DescribePdf(DescribePdfArgs),
    #[command(about = "Render every page of a PDF to PNG with Poppler (pdftoppm)")]
    RenderPages(RenderPagesArgs),
}

#[derive(Subcommand)]
enum ManifestCommands {
    #[command(about = "Write the output PDF described by a manifest and verify it")]
    ApplyManifest(ManifestArgs),
    #[command(about = "Verify an output PDF against its manifest")]
    VerifyManifest(ManifestArgs),
}

#[derive(Args)]
struct DescribePdfArgs {
    #[arg(long, value_name = "PATH", value_parser = parse_value::<PathBuf>, help = "PDF to inspect")]
    pdf_file: PathBuf,
}

#[derive(Args)]
struct RenderPagesArgs {
    #[arg(long, value_name = "PATH", value_parser = parse_value::<PathBuf>, help = "PDF to render")]
    pdf_file: PathBuf,
    #[arg(
        long,
        value_name = "PATH",
        value_parser = parse_value::<PathBuf>,
        help = "Directory that receives <prefix>-NNN.png"
    )]
    output_dir: PathBuf,
    #[arg(
        long,
        default_value = "page",
        value_parser = parse_prefix,
        help = "File name prefix (ASCII letters, digits, '-' and '_')"
    )]
    prefix: String,
    #[arg(long, default_value = "144", value_parser = parse_dpi, help = "Resolution, 72 to 600")]
    dpi: u32,
}

#[derive(Args)]
struct ManifestArgs {
    #[arg(
        long,
        value_name = "PATH",
        value_parser = parse_value::<PathBuf>,
        help = "Manifest JSON; relative paths inside it resolve from its directory"
    )]
    manifest_file: PathBuf,
}

fn read_value(raw: &str) -> std::result::Result<String, String> {
    match raw.strip_prefix("file://") {
        Some(path) => fs::read_to_string(path)
            .map(|content| content.strip_suffix('\n').unwrap_or(&content).to_string())
            .map_err(|error| format!("{path}: {error}")),
        None => Ok(raw.to_string()),
    }
}

fn parse_value<T>(raw: &str) -> std::result::Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    read_value(raw)?
        .parse()
        .map_err(|error: T::Err| error.to_string())
}

fn parse_dpi(raw: &str) -> std::result::Result<u32, String> {
    let dpi: u32 = parse_value(raw)?;
    if !(72..=600).contains(&dpi) {
        return Err("must be between 72 and 600".to_string());
    }
    Ok(dpi)
}

fn parse_prefix(raw: &str) -> std::result::Result<String, String> {
    let prefix = read_value(raw)?;
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("may contain only ASCII letters, digits, '-' and '_'".to_string());
    }
    Ok(prefix)
}

pub(crate) fn main() -> i32 {
    match run(std::env::args_os().collect()) {
        Ok(()) => 0,
        Err(error) => {
            match &error {
                Error::Clap(_) => eprint!("{error}"),
                _ => eprintln!("error: {error}"),
            }
            error.exit_code()
        }
    }
}

fn run(argv: Vec<OsString>) -> Result<()> {
    let mut argv = argv
        .into_iter()
        .map(|argument| {
            argument
                .into_string()
                .map_err(|argument| Error::Usage(format!("argument is not UTF-8: {argument:?}")))
        })
        .collect::<Result<Vec<_>>>()?;
    rewrite_help(&mut argv);
    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(error) => return clap_exit(error),
    };
    if cli.debug {
        eprintln!("debug: argv {argv:?}");
    }
    let value = execute(cli.command)?;
    match cli.output {
        OutputFormat::Json => print_json(&value),
        OutputFormat::Text => {
            let mut lines = Vec::new();
            text_lines(&value, None, &mut lines);
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        OutputFormat::Table => {
            print!("{}", table(&value, &table_title(&argv)));
            Ok(())
        }
    }
}

fn rewrite_help(argv: &mut [String]) {
    let length = argv.len();
    if length >= 2 && argv[length - 1] == "help" && !argv[length - 2].starts_with('-') {
        argv[length - 1] = "--help".to_string();
    }
}

fn clap_exit(error: clap::Error) -> Result<()> {
    if error.exit_code() == 0 {
        print!("{error}");
        return Ok(());
    }
    Err(Error::Clap(error))
}

fn leaf_matches(matches: &ArgMatches) -> Option<(&str, &str, &ArgMatches)> {
    let (command, matches) = matches.subcommand()?;
    let (leaf, matches) = matches.subcommand()?;
    Some((command, leaf, matches))
}

fn pascal_case(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn execute(command: Commands) -> Result<Value> {
    match command {
        Commands::Pdf(PdfCommands::DescribePdf(args)) => to_value(&inspect_pdf(&args.pdf_file)?),
        Commands::Pdf(PdfCommands::RenderPages(args)) => to_value(&render_pdf(
            &args.pdf_file,
            &args.output_dir,
            &args.prefix,
            args.dpi,
        )?),
        Commands::Manifest(ManifestCommands::ApplyManifest(args)) => {
            to_value(&apply_manifest(&load_manifest(&args.manifest_file)?)?)
        }
        Commands::Manifest(ManifestCommands::VerifyManifest(args)) => {
            to_value(&verify_manifest(&load_manifest(&args.manifest_file)?)?)
        }
    }
}

fn to_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| Error::Other(error.to_string()))
}

fn print_json(value: &Value) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| Error::Other(error.to_string()))?
    );
    Ok(())
}

fn is_scalar(value: &Value) -> bool {
    !matches!(value, Value::Array(_) | Value::Object(_))
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn text_lines(value: &Value, label: Option<&str>, lines: &mut Vec<String>) {
    let prefixed = |fields: Vec<String>| match label {
        Some(label) => std::iter::once(label.to_uppercase())
            .chain(fields)
            .collect::<Vec<_>>()
            .join("\t"),
        None => fields.join("\t"),
    };
    match value {
        Value::Object(map) => {
            let fields = map
                .values()
                .filter(|value| is_scalar(value))
                .map(scalar_text)
                .collect::<Vec<_>>();
            if !fields.is_empty() {
                lines.push(prefixed(fields));
            }
            for (key, value) in map.iter().filter(|(_, value)| !is_scalar(value)) {
                text_lines(value, Some(key), lines);
            }
        }
        Value::Array(items) if items.iter().all(is_scalar) => {
            lines.push(prefixed(items.iter().map(scalar_text).collect()));
        }
        Value::Array(items) => {
            for item in items {
                text_lines(item, label, lines);
            }
        }
        scalar => lines.push(prefixed(vec![scalar_text(scalar)])),
    }
}

fn table_title(argv: &[String]) -> String {
    let matches = Cli::command().ignore_errors(true).get_matches_from(argv);
    leaf_matches(&matches)
        .map(|(_, leaf, _)| pascal_case(leaf))
        .unwrap_or_default()
}

fn table(value: &Value, title: &str) -> String {
    let mut output = String::new();
    table_section(value, title, &mut output);
    output
}

fn table_section(value: &Value, title: &str, output: &mut String) {
    match value {
        Value::Object(map) => {
            let rows = map
                .iter()
                .filter(|(_, value)| is_scalar(value))
                .map(|(key, value)| vec![key.clone(), scalar_text(value)])
                .collect::<Vec<_>>();
            if !rows.is_empty() {
                render_box(title, None, &rows, output);
            }
            for (key, value) in map.iter().filter(|(_, value)| !is_scalar(value)) {
                table_section(value, key, output);
            }
        }
        Value::Array(items) if items.iter().all(|item| matches!(item, Value::Object(_))) => {
            let mut headers: Vec<String> = Vec::new();
            for item in items {
                if let Value::Object(map) = item {
                    for (key, value) in map {
                        if is_scalar(value) && !headers.contains(key) {
                            headers.push(key.clone());
                        }
                    }
                }
            }
            let rows = items
                .iter()
                .map(|item| {
                    headers
                        .iter()
                        .map(|header| item.get(header).map(scalar_text).unwrap_or_default())
                        .collect()
                })
                .collect::<Vec<_>>();
            if !headers.is_empty() {
                render_box(title, Some(&headers), &rows, output);
            }
            for item in items {
                if let Value::Object(map) = item {
                    for (key, value) in map.iter().filter(|(_, value)| !is_scalar(value)) {
                        table_section(value, key, output);
                    }
                }
            }
        }
        Value::Array(items) => {
            let rows = items
                .iter()
                .map(|item| vec![scalar_text(item)])
                .collect::<Vec<_>>();
            render_box(title, None, &rows, output);
        }
        scalar => render_box(title, None, &[vec![scalar_text(scalar)]], output),
    }
}

fn render_box(title: &str, headers: Option<&[String]>, rows: &[Vec<String>], output: &mut String) {
    let columns = headers
        .map(|headers| headers.len())
        .unwrap_or_else(|| rows.iter().map(Vec::len).max().unwrap_or(1));
    let mut widths = vec![0; columns];
    for row in headers
        .into_iter()
        .map(|headers| headers.to_vec())
        .chain(rows.iter().cloned())
    {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let inner = widths.iter().map(|width| width + 2).sum::<usize>() + columns - 1;
    let inner = inner.max(title.chars().count() + 2);
    let extra = inner + 1 - (widths.iter().map(|width| width + 2).sum::<usize>() + columns);
    if let Some(last) = widths.last_mut() {
        *last += extra;
    }
    let border = |corner: char| {
        let mut line = String::from(corner);
        for width in &widths {
            line.push_str(&"-".repeat(width + 2));
            line.push(corner);
        }
        line.push('\n');
        line
    };
    let row_line = |cells: &[String]| {
        let mut line = String::from("|");
        for (index, width) in widths.iter().enumerate() {
            let cell = cells.get(index).map(String::as_str).unwrap_or("");
            line.push_str(&format!(" {cell:<width$} |"));
        }
        line.push('\n');
        line
    };
    output.push_str(&format!("{}\n", "-".repeat(inner + 2)));
    output.push_str(&format!("|{title:^inner$}|\n"));
    output.push_str(&border('+'));
    if let Some(headers) = headers {
        output.push_str(&row_line(headers));
        output.push_str(&border('+'));
    }
    for row in rows {
        output.push_str(&row_line(row));
    }
    output.push_str(&border('+'));
}
