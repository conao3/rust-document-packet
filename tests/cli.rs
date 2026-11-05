use std::{fs, path::Path, process::Command};

use lopdf::{Dictionary, Document, Object, Stream, dictionary};
use serde_json::{Value, json};

fn create_pdf(path: &Path) {
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let page_id = document.new_object_id();
    let content_id = document.add_object(Stream::new(Dictionary::new(), Vec::new()));
    document.objects.insert(
        page_id,
        Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => Dictionary::new(),
            "Contents" => content_id,
        }),
    );
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    document.save(path).unwrap();
}

fn run(directory: &Path, arguments: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_document-packet"))
        .current_dir(directory)
        .args(arguments)
        .output()
        .unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn run_json(directory: &Path, arguments: &[&str]) -> Value {
    let (code, stdout, stderr) = run(directory, arguments);
    assert_eq!(code, 0, "{stderr}");
    serde_json::from_str(&stdout).unwrap()
}

fn packet(directory: &Path) -> String {
    create_pdf(&directory.join("source.pdf"));
    image::RgbaImage::from_pixel(20, 10, image::Rgba([10, 20, 30, 200]))
        .save(directory.join("signature.png"))
        .unwrap();
    let described = run_json(
        directory,
        &["pdf", "describe-pdf", "--pdf-file", "source.pdf"],
    );
    let sha256 = described["Sha256"].as_str().unwrap().to_string();
    let manifest = json!({
        "schema": 1,
        "source": "source.pdf",
        "output": "out/signed.pdf",
        "expected_source_sha256": sha256,
        "expected_page_count": 1,
        "min_output_bytes": 100,
        "operations": [
            {"type": "image", "page": 1, "path": "signature.png", "x": 100, "y": 200, "width": 120, "height": 60},
            {"type": "text", "page": 1, "text": "Date: 2026/08/28", "x": 100, "y": 180, "size": 9}
        ]
    });
    fs::write(directory.join("manifest.json"), manifest.to_string()).unwrap();
    sha256
}

#[test]
fn describe_pdf_prints_pascal_case_json() {
    let directory = tempfile::tempdir().unwrap();
    create_pdf(&directory.path().join("source.pdf"));
    let described = run_json(
        directory.path(),
        &["pdf", "describe-pdf", "--pdf-file", "source.pdf"],
    );
    assert_eq!(described["PdfFile"], "source.pdf");
    assert_eq!(described["PageCount"], 1);
    assert_eq!(described["Pages"][0]["WidthPoints"], 612.0);
    assert_eq!(described["Pages"][0]["Rotation"], 0);
}

#[test]
fn apply_and_verify_manifest_with_text_output() {
    let directory = tempfile::tempdir().unwrap();
    let sha256 = packet(directory.path());
    let applied = run_json(
        directory.path(),
        &[
            "manifest",
            "apply-manifest",
            "--manifest-file",
            "manifest.json",
        ],
    );
    assert_eq!(applied["SourceSha256"], sha256.as_str());
    assert_eq!(applied["OperationCount"], 2);
    assert!(directory.path().join("out/signed.pdf").exists());

    let (code, stdout, _) = run(
        directory.path(),
        &[
            "--output",
            "text",
            "manifest",
            "verify-manifest",
            "--manifest-file",
            "manifest.json",
        ],
    );
    assert_eq!(code, 0);
    assert!(
        stdout.starts_with(&format!("source.pdf\t{sha256}\tout/signed.pdf\t")),
        "{stdout}"
    );
    assert!(stdout.ends_with("\t1\t2\n"), "{stdout}");
}

#[test]
fn file_prefix_reads_parameter_values() {
    let directory = tempfile::tempdir().unwrap();
    create_pdf(&directory.path().join("source.pdf"));
    fs::write(directory.path().join("pdf-file.txt"), "source.pdf\n").unwrap();
    let described = run_json(
        directory.path(),
        &["pdf", "describe-pdf", "--pdf-file", "file://pdf-file.txt"],
    );
    assert_eq!(described["PdfFile"], "source.pdf");
}

#[test]
fn help_is_available_at_every_level() {
    let directory = tempfile::tempdir().unwrap();
    for arguments in [
        vec!["help"],
        vec!["pdf", "help"],
        vec!["manifest", "apply-manifest", "help"],
    ] {
        let (code, stdout, _) = run(directory.path(), &arguments);
        assert_eq!(code, 0);
        assert!(stdout.contains("Usage: document-packet"));
    }
}

#[test]
fn exit_codes_separate_syntax_and_target_errors() {
    let directory = tempfile::tempdir().unwrap();
    packet(directory.path());
    for arguments in [
        vec!["inspect", "--pdf", "source.pdf"],
        vec!["pdf", "describe-pdf"],
        vec![
            "pdf",
            "render-pages",
            "--pdf-file",
            "source.pdf",
            "--output-dir",
            "pages",
            "--dpi",
            "10",
        ],
    ] {
        let (code, _, stderr) = run(directory.path(), &arguments);
        assert_eq!(code, 252, "{arguments:?}: {stderr}");
    }
    let (code, _, _) = run(
        directory.path(),
        &[
            "manifest",
            "verify-manifest",
            "--manifest-file",
            "manifest.json",
        ],
    );
    assert_eq!(code, 254);
    let (code, _, _) = run(
        directory.path(),
        &["pdf", "describe-pdf", "--pdf-file", "missing.pdf"],
    );
    assert_eq!(code, 254);
}
