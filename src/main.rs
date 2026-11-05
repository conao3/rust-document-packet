mod cli;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use image::GenericImageView;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug)]
enum Error {
    Clap(clap::Error),
    Usage(String),
    Target(String),
    Other(String),
}

impl Error {
    fn exit_code(&self) -> i32 {
        match self {
            Error::Clap(_) | Error::Usage(_) => 252,
            Error::Target(_) => 254,
            Error::Other(_) => 255,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Clap(error) => write!(formatter, "{}", error.render()),
            Error::Usage(message) | Error::Target(message) | Error::Other(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Error::Target(message)
    }
}

fn other(error: impl fmt::Display) -> Error {
    Error::Other(error.to_string())
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Deserialize, Serialize)]
struct Manifest {
    schema: u32,
    source: PathBuf,
    output: PathBuf,
    #[serde(default)]
    expected_source_sha256: Option<String>,
    #[serde(default)]
    expected_page_count: Option<usize>,
    #[serde(default)]
    min_output_bytes: Option<u64>,
    #[serde(default)]
    max_output_bytes: Option<u64>,
    operations: Vec<Operation>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Operation {
    Image {
        page: u32,
        path: PathBuf,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    Text {
        page: u32,
        text: String,
        x: f64,
        y: f64,
        #[serde(default = "default_font_size")]
        size: f64,
    },
}

fn default_font_size() -> f64 {
    10.0
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
struct PdfInspection {
    pdf_file: PathBuf,
    sha256: String,
    bytes: u64,
    page_count: usize,
    pages: Vec<PageInspection>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
struct PageInspection {
    page: u32,
    width_points: f64,
    height_points: f64,
    rotation: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct RenderReport {
    output_dir: PathBuf,
    page_count: usize,
    pages: Vec<RenderedPage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct RenderedPage {
    page: u32,
    image_file: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct PacketReport {
    source_file: PathBuf,
    source_sha256: String,
    output_file: PathBuf,
    output_sha256: String,
    output_bytes: u64,
    page_count: usize,
    operation_count: usize,
}

#[derive(Debug)]
struct ResolvedManifest {
    manifest: Manifest,
    source: PathBuf,
    output: PathBuf,
    operation_images: BTreeMap<usize, PathBuf>,
}

fn main() {
    std::process::exit(cli::main());
}

fn load_manifest(path: &Path) -> Result<ResolvedManifest> {
    let raw = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let manifest: Manifest =
        serde_json::from_str(&raw).map_err(|error| format!("{}: {error}", path.display()))?;
    if manifest.schema != 1 {
        return Err(format!(
            "unsupported manifest schema {}; expected 1",
            manifest.schema
        )
        .into());
    }
    if manifest.operations.is_empty() {
        return Err("manifest must contain at least one operation"
            .to_string()
            .into());
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let source = resolve_path(base, &manifest.source);
    let output = resolve_path(base, &manifest.output);
    if source == output {
        return Err("source and output must be different files"
            .to_string()
            .into());
    }
    let mut operation_images = BTreeMap::new();
    for (index, operation) in manifest.operations.iter().enumerate() {
        match operation {
            Operation::Image {
                path,
                page,
                width,
                height,
                ..
            } => {
                if *page == 0 || *width <= 0.0 || *height <= 0.0 {
                    return Err(
                        format!("operation {} has invalid page or dimensions", index + 1).into(),
                    );
                }
                operation_images.insert(index, resolve_path(base, path));
            }
            Operation::Text {
                page, text, size, ..
            } => {
                if *page == 0 || *size <= 0.0 || text.is_empty() {
                    return Err(
                        format!("operation {} has invalid page, text, or size", index + 1).into(),
                    );
                }
                if !text.is_ascii() {
                    return Err(format!(
                        "operation {} text must be ASCII when using the built-in Helvetica font",
                        index + 1
                    )
                    .into());
                }
            }
        }
    }
    Ok(ResolvedManifest {
        manifest,
        source,
        output,
        operation_images,
    })
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn inspect_pdf(path: &Path) -> Result<PdfInspection> {
    let document = Document::load(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let pages = document.get_pages();
    let mut inspected = Vec::with_capacity(pages.len());
    for (page, id) in pages {
        let media_box = inherited_array(&document, id, b"MediaBox")?;
        if media_box.len() != 4 {
            return Err(format!("page {page} MediaBox must contain four numbers").into());
        }
        let x0 = number(&document, &media_box[0])?;
        let y0 = number(&document, &media_box[1])?;
        let x1 = number(&document, &media_box[2])?;
        let y1 = number(&document, &media_box[3])?;
        let rotation = inherited_optional(&document, id, b"Rotate")
            .and_then(|object| object.as_i64().ok())
            .unwrap_or(0);
        inspected.push(PageInspection {
            page,
            width_points: (x1 - x0).abs(),
            height_points: (y1 - y0).abs(),
            rotation,
        });
    }
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(PdfInspection {
        pdf_file: path.to_path_buf(),
        sha256: sha256(path)?,
        bytes: metadata.len(),
        page_count: inspected.len(),
        pages: inspected,
    })
}

fn inherited_optional(document: &Document, mut id: ObjectId, key: &[u8]) -> Option<Object> {
    loop {
        let dictionary = document.get_object(id).ok()?.as_dict().ok()?;
        if let Ok(value) = dictionary.get(key) {
            return resolve_object(document, value).ok();
        }
        id = dictionary.get(b"Parent").ok()?.as_reference().ok()?;
    }
}

fn inherited_array(document: &Document, id: ObjectId, key: &[u8]) -> Result<Vec<Object>> {
    inherited_optional(document, id, key)
        .ok_or_else(|| {
            format!(
                "page object {id:?} has no inherited {}",
                String::from_utf8_lossy(key)
            )
        })?
        .as_array()
        .cloned()
        .map_err(|_| format!("{} is not an array", String::from_utf8_lossy(key)).into())
}

fn resolve_object(document: &Document, object: &Object) -> Result<Object> {
    match object {
        Object::Reference(id) => document
            .get_object(*id)
            .cloned()
            .map_err(|error| error.to_string().into()),
        other => Ok(other.clone()),
    }
}

fn number(document: &Document, object: &Object) -> Result<f64> {
    match resolve_object(document, object)? {
        Object::Integer(value) => Ok(value as f64),
        Object::Real(value) => Ok(value as f64),
        other => Err(format!("expected PDF number, got {other:?}").into()),
    }
}

fn page_id(document: &Document, page_number: u32) -> Result<ObjectId> {
    document
        .get_pages()
        .get(&page_number)
        .copied()
        .ok_or_else(|| format!("page {page_number} does not exist").into())
}

fn cloned_page_resources(document: &Document, page: ObjectId) -> Result<Dictionary> {
    match inherited_optional(document, page, b"Resources") {
        Some(Object::Dictionary(dictionary)) => Ok(dictionary),
        Some(other) => Err(format!("page Resources is not a dictionary: {other:?}").into()),
        None => Ok(Dictionary::new()),
    }
}

fn nested_dictionary(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Dictionary> {
    match dictionary.get(key) {
        Ok(object) => match resolve_object(document, object)? {
            Object::Dictionary(dictionary) => Ok(dictionary),
            other => Err(format!(
                "{} resource is not a dictionary: {other:?}",
                String::from_utf8_lossy(key)
            )
            .into()),
        },
        Err(_) => Ok(Dictionary::new()),
    }
}

fn append_page_content(document: &mut Document, page: ObjectId, data: Vec<u8>) -> Result<()> {
    let stream_id = document.add_object(Stream::new(Dictionary::new(), data));
    let page_dictionary = document
        .get_object_mut(page)
        .map_err(|error| error.to_string())?
        .as_dict_mut()
        .map_err(|error| error.to_string())?;
    let existing = page_dictionary.get(b"Contents").ok().cloned();
    let contents = match existing {
        None => Object::Reference(stream_id),
        Some(Object::Array(mut items)) => {
            items.push(Object::Reference(stream_id));
            Object::Array(items)
        }
        Some(object) => Object::Array(vec![object, Object::Reference(stream_id)]),
    };
    page_dictionary.set("Contents", contents);
    Ok(())
}

fn isolate_page_content(document: &mut Document, page: ObjectId) -> Result<()> {
    let prefix = document.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let suffix = document.add_object(Stream::new(Dictionary::new(), b"Q\n".to_vec()));
    let page_dictionary = document
        .get_object_mut(page)
        .map_err(|error| error.to_string())?
        .as_dict_mut()
        .map_err(|error| error.to_string())?;
    let existing = page_dictionary.get(b"Contents").ok().cloned();
    let mut contents = vec![Object::Reference(prefix)];
    match existing {
        None => {}
        Some(Object::Array(items)) => contents.extend(items),
        Some(object) => contents.push(object),
    }
    contents.push(Object::Reference(suffix));
    page_dictionary.set("Contents", Object::Array(contents));
    Ok(())
}

fn install_image(
    document: &mut Document,
    page: ObjectId,
    resource_name: &str,
    image_path: &Path,
) -> Result<()> {
    let image =
        image::open(image_path).map_err(|error| format!("{}: {error}", image_path.display()))?;
    let (width, height) = image.dimensions();
    let rgba = image.to_rgba8();
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    let mut alpha = Vec::with_capacity((width * height) as usize);
    let mut has_alpha = false;
    for pixel in rgba.pixels() {
        rgb.extend_from_slice(&pixel.0[..3]);
        alpha.push(pixel.0[3]);
        has_alpha |= pixel.0[3] != 255;
    }

    let smask = if has_alpha {
        let mut stream = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => width as i64,
                "Height" => height as i64,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            alpha,
        );
        stream.compress().map_err(|error| error.to_string())?;
        Some(document.add_object(stream))
    } else {
        None
    };

    let mut image_dictionary = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => width as i64,
        "Height" => height as i64,
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
    };
    if let Some(smask) = smask {
        image_dictionary.set("SMask", Object::Reference(smask));
    }
    let mut stream = Stream::new(image_dictionary, rgb);
    stream.compress().map_err(|error| error.to_string())?;
    let image_id = document.add_object(stream);

    let mut resources = cloned_page_resources(document, page)?;
    let mut xobjects = nested_dictionary(document, &resources, b"XObject")?;
    xobjects.set(resource_name.as_bytes(), Object::Reference(image_id));
    resources.set("XObject", Object::Dictionary(xobjects));
    document
        .get_object_mut(page)
        .map_err(|error| error.to_string())?
        .as_dict_mut()
        .map_err(|error| error.to_string())?
        .set("Resources", Object::Dictionary(resources));
    Ok(())
}

fn install_helvetica(document: &mut Document, page: ObjectId) -> Result<()> {
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let mut resources = cloned_page_resources(document, page)?;
    let mut fonts = nested_dictionary(document, &resources, b"Font")?;
    fonts.set("DPFont", Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));
    document
        .get_object_mut(page)
        .map_err(|error| error.to_string())?
        .as_dict_mut()
        .map_err(|error| error.to_string())?
        .set("Resources", Object::Dictionary(resources));
    Ok(())
}

fn escape_pdf_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn apply_manifest(resolved: &ResolvedManifest) -> Result<PacketReport> {
    let source_before = sha256(&resolved.source)?;
    if let Some(expected) = &resolved.manifest.expected_source_sha256 {
        if &source_before != expected {
            return Err(format!(
                "source sha256 mismatch: expected {expected}, got {source_before}"
            )
            .into());
        }
    }
    let inspection = inspect_pdf(&resolved.source)?;
    if let Some(expected) = resolved.manifest.expected_page_count {
        if inspection.page_count != expected {
            return Err(format!(
                "source page count mismatch: expected {expected}, got {}",
                inspection.page_count
            )
            .into());
        }
    }
    if let Some(parent) = resolved.output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| other(format!("{}: {error}", parent.display())))?;
    }

    let mut document = Document::load(&resolved.source)
        .map_err(|error| format!("{}: {error}", resolved.source.display()))?;
    let mut isolated_pages = BTreeSet::new();
    for (index, operation) in resolved.manifest.operations.iter().enumerate() {
        let marker = format!("% document-packet:op-{}\n", index + 1);
        match operation {
            Operation::Image {
                page,
                x,
                y,
                width,
                height,
                ..
            } => {
                let page_id = page_id(&document, *page)?;
                if isolated_pages.insert(page_id) {
                    isolate_page_content(&mut document, page_id)?;
                }
                let resource = format!("DPImg{}", index + 1);
                let image_path = resolved.operation_images.get(&index).unwrap();
                install_image(&mut document, page_id, &resource, image_path)?;
                append_page_content(
                    &mut document,
                    page_id,
                    format!(
                        "{marker}q\n{width:.4} 0 0 {height:.4} {x:.4} {y:.4} cm\n/{resource} Do\nQ\n"
                    )
                    .into_bytes(),
                )?;
            }
            Operation::Text {
                page,
                text,
                x,
                y,
                size,
            } => {
                let page_id = page_id(&document, *page)?;
                if isolated_pages.insert(page_id) {
                    isolate_page_content(&mut document, page_id)?;
                }
                install_helvetica(&mut document, page_id)?;
                append_page_content(
                    &mut document,
                    page_id,
                    format!(
                        "{marker}BT\n/DPFont {size:.4} Tf\n{x:.4} {y:.4} Td\n({}) Tj\nET\n",
                        escape_pdf_text(text)
                    )
                    .into_bytes(),
                )?;
            }
        }
    }
    let parent = resolved
        .output
        .parent()
        .ok_or_else(|| "output has no parent directory".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(other)?;
    document.save_to(temporary.as_file_mut()).map_err(other)?;
    temporary.as_file_mut().flush().map_err(other)?;
    temporary.as_file().sync_all().map_err(other)?;
    temporary
        .persist(&resolved.output)
        .map_err(|error| other(error.error))?;
    if sha256(&resolved.source)? != source_before {
        return Err("source PDF changed while applying manifest"
            .to_string()
            .into());
    }
    verify_manifest(resolved)
}

fn verify_manifest(resolved: &ResolvedManifest) -> Result<PacketReport> {
    let source = inspect_pdf(&resolved.source)?;
    let output = inspect_pdf(&resolved.output)?;
    if let Some(expected) = &resolved.manifest.expected_source_sha256 {
        if &source.sha256 != expected {
            return Err(format!(
                "source sha256 mismatch: expected {expected}, got {}",
                source.sha256
            )
            .into());
        }
    }
    if let Some(expected) = resolved.manifest.expected_page_count {
        if output.page_count != expected {
            return Err(format!(
                "output page count mismatch: expected {expected}, got {}",
                output.page_count
            )
            .into());
        }
    }
    if source.page_count != output.page_count {
        return Err(format!(
            "page count changed: source {}, output {}",
            source.page_count, output.page_count
        )
        .into());
    }
    for (source_page, output_page) in source.pages.iter().zip(&output.pages) {
        if (source_page.width_points - output_page.width_points).abs() > 0.01
            || (source_page.height_points - output_page.height_points).abs() > 0.01
            || source_page.rotation != output_page.rotation
        {
            return Err(format!("page {} dimensions or rotation changed", source_page.page).into());
        }
    }
    if let Some(minimum) = resolved.manifest.min_output_bytes {
        if output.bytes < minimum {
            return Err(format!("output is too small: {} < {minimum} bytes", output.bytes).into());
        }
    }
    if let Some(maximum) = resolved.manifest.max_output_bytes {
        if output.bytes > maximum {
            return Err(format!("output is too large: {} > {maximum} bytes", output.bytes).into());
        }
    }

    let document = Document::load(&resolved.output)
        .map_err(|error| format!("{}: {error}", resolved.output.display()))?;
    for (index, operation) in resolved.manifest.operations.iter().enumerate() {
        let target_page = match operation {
            Operation::Image { page, .. } | Operation::Text { page, .. } => *page,
        };
        let page = page_id(&document, target_page)?;
        let contents = document.get_page_content(page);
        let marker = format!("document-packet:op-{}", index + 1);
        if !contents
            .windows(marker.len())
            .any(|window| window == marker.as_bytes())
        {
            return Err(format!(
                "operation {} marker is missing on page {target_page}",
                index + 1
            )
            .into());
        }
        if matches!(operation, Operation::Image { .. }) {
            let resource_name = format!("DPImg{}", index + 1);
            let resources = cloned_page_resources(&document, page)?;
            let xobjects = nested_dictionary(&document, &resources, b"XObject")?;
            if xobjects.get(resource_name.as_bytes()).is_err() {
                return Err(format!("operation {} image resource is missing", index + 1).into());
            }
        }
    }
    Ok(PacketReport {
        source_file: resolved.source.clone(),
        source_sha256: source.sha256,
        output_file: resolved.output.clone(),
        output_sha256: output.sha256,
        output_bytes: output.bytes,
        page_count: output.page_count,
        operation_count: resolved.manifest.operations.len(),
    })
}

fn render_pdf(pdf: &Path, output_dir: &Path, prefix: &str, dpi: u32) -> Result<RenderReport> {
    inspect_pdf(pdf)?;
    fs::create_dir_all(output_dir)
        .map_err(|error| other(format!("{}: {error}", output_dir.display())))?;
    for entry in fs::read_dir(output_dir).map_err(other)? {
        let entry = entry.map_err(other)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if rendered_page_number(&name, prefix).is_some() {
            fs::remove_file(entry.path()).map_err(other)?;
        }
    }
    let output_prefix = output_dir.join(prefix);
    let status = Command::new("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg(dpi.to_string())
        .arg(pdf)
        .arg(&output_prefix)
        .status()
        .map_err(|error| other(format!("failed to run pdftoppm: {error}")))?;
    if !status.success() {
        return Err(format!("pdftoppm exited with {status}").into());
    }
    let mut rendered = fs::read_dir(output_dir)
        .map_err(other)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let page = rendered_page_number(&name.to_string_lossy(), prefix)?;
            Some((page, entry.path()))
        })
        .collect::<Vec<_>>();
    rendered.sort_by_key(|(page, _)| *page);
    if rendered.is_empty() {
        return Err("pdftoppm produced no PNG pages".to_string().into());
    }
    let mut pages = Vec::with_capacity(rendered.len());
    for (page, source) in &rendered {
        let destination = output_dir.join(format!("{prefix}-{page:03}.png"));
        if source != &destination {
            fs::rename(source, &destination).map_err(other)?;
        }
        pages.push(RenderedPage {
            page: *page,
            image_file: destination,
        });
    }
    Ok(RenderReport {
        output_dir: output_dir.to_path_buf(),
        page_count: pages.len(),
        pages,
    })
}

fn rendered_page_number(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(&format!("{prefix}-"))?
        .strip_suffix(".png")?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        document.compress();
        document.save(path).unwrap();
    }

    #[test]
    fn apply_and_verify_preserve_source_and_page_geometry() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.pdf");
        let output = directory.path().join("output.pdf");
        let signature = directory.path().join("signature.png");
        create_pdf(&source);
        image::RgbaImage::from_pixel(20, 10, image::Rgba([10, 20, 30, 200]))
            .save(&signature)
            .unwrap();
        let source_before = fs::read(&source).unwrap();
        let resolved = ResolvedManifest {
            manifest: Manifest {
                schema: 1,
                source: source.clone(),
                output: output.clone(),
                expected_source_sha256: Some(sha256(&source).unwrap()),
                expected_page_count: Some(1),
                min_output_bytes: Some(100),
                max_output_bytes: None,
                operations: vec![
                    Operation::Image {
                        page: 1,
                        path: signature.clone(),
                        x: 100.0,
                        y: 200.0,
                        width: 120.0,
                        height: 60.0,
                    },
                    Operation::Text {
                        page: 1,
                        text: "Date: 2026/08/28".to_string(),
                        x: 100.0,
                        y: 180.0,
                        size: 9.0,
                    },
                ],
            },
            source,
            output,
            operation_images: BTreeMap::from([(0, signature)]),
        };
        let report = apply_manifest(&resolved).unwrap();
        assert_eq!(report.page_count, 1);
        assert_eq!(report.operation_count, 2);
        assert_eq!(fs::read(&resolved.source).unwrap(), source_before);
        verify_manifest(&resolved).unwrap();

        let output_document = Document::load(&resolved.output).unwrap();
        let output_page = page_id(&output_document, 1).unwrap();
        let contents = output_document
            .get_object(output_page)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_array()
            .unwrap();
        let stream_data = |object: &Object| {
            let id = object.as_reference().unwrap();
            output_document
                .get_object(id)
                .unwrap()
                .as_stream()
                .unwrap()
                .content
                .clone()
        };
        assert_eq!(stream_data(&contents[0]), b"q\n");
        assert_eq!(stream_data(&contents[2]), b"Q\n");
        assert!(
            String::from_utf8_lossy(&stream_data(&contents[3])).contains("document-packet:op-1")
        );
        assert!(
            String::from_utf8_lossy(&stream_data(&contents[4])).contains("document-packet:op-2")
        );
    }

    #[test]
    fn rejects_source_output_alias_and_non_ascii_text() {
        let directory = tempfile::tempdir().unwrap();
        let manifest_path = directory.path().join("manifest.json");
        fs::write(
            &manifest_path,
            r#"{"schema":1,"source":"same.pdf","output":"same.pdf","operations":[{"type":"text","page":1,"text":"署名","x":1,"y":1}]}"#,
        )
        .unwrap();
        assert!(
            load_manifest(&manifest_path)
                .unwrap_err()
                .to_string()
                .contains("source and output")
        );
    }
}
