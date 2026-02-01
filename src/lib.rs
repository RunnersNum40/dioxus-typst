//! A Dioxus component for rendering Typst documents as HTML.
//!
//! This crate provides a [`Typst`] component that compiles Typst markup to HTML
//! at runtime, allowing you to embed rich typeset content in Dioxus applications.
//!
//! # Example
//!
//! ```rust,ignore
//! use dioxus::prelude::*;
//! use dioxus_typst::Typst;
//!
//! #[component]
//! fn App() -> Element {
//!     let content = r#"
//! = Hello, Typst!
//!
//! Some *formatted* text with math: $E = m c^2$
//! "#;
//!
//!     rsx! {
//!         Typst { source: content.to_string() }
//!     }
//! }
//! ```
//!
//! # Feature Flags
//!
//! - **`fonts`** (default): Bundles fonts from `typst-assets` for consistent rendering.
//! - **`download-packages`**: Enables automatic downloading of Typst packages from
//!   the package registry.
//!
//! # Limitations
//!
//! Typst's HTML export is experimental and may change between versions. Pin your
//! `typst` dependency and test output carefully when upgrading.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{Datelike, Timelike};
use dioxus::prelude::*;
use typst::{
    Feature, Library, LibraryExt, World,
    diag::{FileError, FileResult, PackageError},
    foundations::{Bytes, Datetime},
    syntax::{FileId, Source, VirtualPath, package::PackageSpec},
    text::{Font, FontBook},
    utils::LazyHash,
};
use typst_html::HtmlDocument;

/// Normalizes a path to ensure it starts with a leading slash.
fn normalize_path(path: String) -> String {
    if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    }
}

/// Options for configuring Typst compilation.
///
/// Use this to provide additional files (images, bibliographies, data files) and
/// pre-loaded packages to the Typst compiler.
///
/// # Example
///
/// ```rust,ignore
/// use dioxus_typst::CompileOptions;
///
/// let options = CompileOptions::new()
///     .with_file("/data.csv", csv_bytes)
///     .with_file("/logo.png", image_bytes);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompileOptions {
    /// Files available to the Typst document, keyed by their virtual path.
    pub files: HashMap<String, Vec<u8>>,
    /// Pre-loaded packages, keyed by their package specification.
    pub packages: HashMap<PackageSpec, HashMap<String, Vec<u8>>>,
}

impl CompileOptions {
    /// Creates a new empty `CompileOptions`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file to the compilation environment.
    ///
    /// The path will be normalized to start with `/`. Files added here can be
    /// referenced in Typst source using their path (e.g., `#image("/logo.png")`).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let options = CompileOptions::new()
    ///     .with_file("/figure.png", png_bytes)
    ///     .with_file("data.csv", csv_bytes); // Also normalized to "/data.csv"
    /// ```
    #[must_use]
    pub fn with_file(mut self, path: impl Into<String>, content: Vec<u8>) -> Self {
        self.files.insert(normalize_path(path.into()), content);
        self
    }

    /// Adds a pre-loaded package to the compilation environment.
    ///
    /// Use this to provide package files without requiring network access. File
    /// paths within the package will be normalized to start with `/`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use typst::syntax::package::PackageSpec;
    /// use std::str::FromStr;
    ///
    /// let options = CompileOptions::new()
    ///     .with_package(
    ///         PackageSpec::from_str("@preview/cetz:0.2.2").unwrap(),
    ///         package_files,
    ///     );
    /// ```
    #[must_use]
    pub fn with_package(mut self, spec: PackageSpec, files: HashMap<String, Vec<u8>>) -> Self {
        let files = files
            .into_iter()
            .map(|(path, content)| (normalize_path(path), content))
            .collect();
        self.packages.insert(spec, files);
        self
    }
}

/// Errors that can occur during Typst compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// An error occurred during Typst compilation or HTML generation.
    ///
    /// The string contains one or more error messages joined by semicolons.
    Typst(String),
    /// An error occurred while loading a package.
    Package(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Typst(msg) => write!(f, "Typst compilation error: {msg}"),
            CompileError::Package(msg) => write!(f, "Package error: {msg}"),
        }
    }
}

impl std::error::Error for CompileError {}

#[cfg(feature = "download-packages")]
mod downloader {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;
    use std::path::PathBuf;
    use tar::Archive;
    use typst::diag::eco_format;

    fn cache_dir() -> Option<PathBuf> {
        dirs::cache_dir().map(|p| p.join("typst").join("packages"))
    }

    fn package_dir(spec: &PackageSpec) -> Option<PathBuf> {
        cache_dir().map(|p| {
            p.join(spec.namespace.as_str())
                .join(spec.name.as_str())
                .join(spec.version.to_string())
        })
    }

    pub fn download_package(spec: &PackageSpec) -> Result<HashMap<String, Vec<u8>>, PackageError> {
        if let Some(dir) = package_dir(spec)
            && dir.exists()
        {
            return read_package_dir(&dir);
        }

        let url = format!(
            "https://packages.typst.org/preview/{}-{}.tar.gz",
            spec.name, spec.version
        );

        let compressed = ureq::get(&url)
            .call()
            .map_err(|e| PackageError::NetworkFailed(Some(eco_format!("{e}"))))?
            .into_body()
            .read_to_vec()
            .map_err(|e| PackageError::NetworkFailed(Some(eco_format!("{e}"))))?;

        let decoder = GzDecoder::new(&compressed[..]);
        let mut archive = Archive::new(decoder);
        let mut files = HashMap::new();

        let cache_path = package_dir(spec);
        if let Some(ref path) = cache_path {
            let _ = std::fs::create_dir_all(path);
        }

        for entry in archive
            .entries()
            .map_err(|e| PackageError::MalformedArchive(Some(eco_format!("{e}"))))?
        {
            let mut entry =
                entry.map_err(|e| PackageError::MalformedArchive(Some(eco_format!("{e}"))))?;

            let path = entry
                .path()
                .map_err(|e| PackageError::MalformedArchive(Some(eco_format!("{e}"))))?
                .into_owned();

            if entry.header().entry_type().is_file() {
                let path_str = format!("/{}", path.to_string_lossy());
                let mut content = Vec::new();
                entry
                    .read_to_end(&mut content)
                    .map_err(|e| PackageError::MalformedArchive(Some(eco_format!("{e}"))))?;

                if let Some(ref cache) = cache_path {
                    let file_path = cache.join(&path);
                    if let Some(parent) = file_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&file_path, &content);
                }

                files.insert(path_str, content);
            }
        }

        Ok(files)
    }

    fn read_package_dir(dir: &PathBuf) -> Result<HashMap<String, Vec<u8>>, PackageError> {
        let mut files = HashMap::new();
        read_dir_recursive(dir, dir, &mut files)?;
        Ok(files)
    }

    fn read_dir_recursive(
        base: &PathBuf,
        current: &PathBuf,
        files: &mut HashMap<String, Vec<u8>>,
    ) -> Result<(), PackageError> {
        for entry in
            std::fs::read_dir(current).map_err(|e| PackageError::Other(Some(eco_format!("{e}"))))?
        {
            let entry = entry.map_err(|e| PackageError::Other(Some(eco_format!("{e}"))))?;
            let path = entry.path();

            if path.is_dir() {
                read_dir_recursive(base, &path, files)?;
            } else {
                let relative = path.strip_prefix(base).unwrap();
                let key = format!("/{}", relative.to_string_lossy());
                let content = std::fs::read(&path)
                    .map_err(|e| PackageError::Other(Some(eco_format!("{e}"))))?;
                files.insert(key, content);
            }
        }
        Ok(())
    }
}

/// The compilation world that provides all resources to the Typst compiler.
struct CompileWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: Source,
    files: HashMap<String, Bytes>,
    packages: Arc<RwLock<HashMap<PackageSpec, HashMap<String, Bytes>>>>,
    #[cfg(feature = "download-packages")]
    allow_downloads: bool,
}

impl CompileWorld {
    /// Creates a new compilation world with the given source and options.
    fn new(source: &str, options: &CompileOptions) -> Self {
        let fonts = load_fonts();
        let book = FontBook::from_fonts(&fonts);
        let main_id = FileId::new(None, VirtualPath::new("/main.typ"));
        let main = Source::new(main_id, source.to_string());

        let files = options
            .files
            .iter()
            .map(|(path, content)| (path.clone(), Bytes::new(content.clone())))
            .collect();

        let packages: HashMap<PackageSpec, HashMap<String, Bytes>> = options
            .packages
            .iter()
            .map(
                |(spec, pkg_files): (&PackageSpec, &HashMap<String, Vec<u8>>)| {
                    let converted: HashMap<String, Bytes> = pkg_files
                        .iter()
                        .map(|(path, content)| (path.clone(), Bytes::new(content.clone())))
                        .collect();
                    (spec.clone(), converted)
                },
            )
            .collect();

        let library = Library::builder()
            .with_features([Feature::Html].into_iter().collect())
            .build();

        Self {
            library: LazyHash::new(library),
            book: LazyHash::new(book),
            fonts,
            main,
            files,
            packages: Arc::new(RwLock::new(packages)),
            #[cfg(feature = "download-packages")]
            allow_downloads: true,
        }
    }

    /// Configures whether automatic package downloads are allowed.
    #[cfg(feature = "download-packages")]
    #[allow(dead_code)]
    fn with_downloads(mut self, allow: bool) -> Self {
        self.allow_downloads = allow;
        self
    }

    /// Retrieves a file from a package, downloading if necessary and allowed.
    fn get_package_file(&self, package: &PackageSpec, path: &str) -> FileResult<Bytes> {
        {
            let packages = self.packages.read().unwrap();
            if let Some(pkg_files) = packages.get(package)
                && let Some(content) = pkg_files.get(path)
            {
                return Ok(content.clone());
            }
        }

        #[cfg(feature = "download-packages")]
        if self.allow_downloads {
            let downloaded = downloader::download_package(package).map_err(FileError::Package)?;

            let result = downloaded
                .get(path)
                .map(|c| Bytes::new(c.clone()))
                .ok_or_else(|| FileError::NotFound(path.into()));

            let mut packages = self.packages.write().unwrap();
            let converted: HashMap<String, Bytes> = downloaded
                .into_iter()
                .map(|(p, c)| (p, Bytes::new(c)))
                .collect();
            packages.insert(package.clone(), converted);

            return result;
        }

        Err(FileError::Package(PackageError::NotFound(package.clone())))
    }
}

impl World for CompileWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            return Ok(self.main.clone());
        }

        if let Some(package) = id.package() {
            let path = id.vpath().as_rooted_path().to_string_lossy();
            let content = self.get_package_file(package, &path)?;
            let text = String::from_utf8(content.to_vec()).map_err(|_| FileError::InvalidUtf8)?;
            return Ok(Source::new(id, text));
        }

        Err(FileError::NotFound(id.vpath().as_rooted_path().into()))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if let Some(package) = id.package() {
            let path = id.vpath().as_rooted_path().to_string_lossy();
            return self.get_package_file(package, &path);
        }

        let path = id.vpath().as_rooted_path().to_string_lossy();
        self.files
            .get(path.as_ref())
            .cloned()
            .ok_or_else(|| FileError::NotFound(id.vpath().as_rooted_path().into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, offset: Option<i64>) -> Option<Datetime> {
        let now = chrono::Local::now();
        let now = match offset {
            Some(hours) => {
                let offset = chrono::FixedOffset::east_opt((hours * 3600) as i32)?;
                now.with_timezone(&offset).naive_local()
            }
            None => now.naive_local(),
        };
        Datetime::from_ymd_hms(
            now.year(),
            now.month().try_into().ok()?,
            now.day().try_into().ok()?,
            now.hour().try_into().ok()?,
            now.minute().try_into().ok()?,
            now.second().try_into().ok()?,
        )
    }
}

/// Loads all available fonts.
fn load_fonts() -> Vec<Font> {
    let mut fonts = Vec::new();
    #[cfg(feature = "fonts")]
    for data in typst_assets::fonts() {
        for font in Font::iter(Bytes::new(data)) {
            fonts.push(font);
        }
    }
    fonts
}

/// Compiles Typst source to HTML.
fn compile(source: &str, options: &CompileOptions) -> Result<String, CompileError> {
    let world = CompileWorld::new(source, options);
    let warned = typst::compile::<HtmlDocument>(&world);
    let document = warned.output.map_err(|errors| {
        let messages: Vec<String> = errors.iter().map(|e| e.message.to_string()).collect();
        CompileError::Typst(messages.join("; "))
    })?;
    typst_html::html(&document).map_err(|errors| {
        let messages: Vec<String> = errors.iter().map(|e| e.message.to_string()).collect();
        CompileError::Typst(messages.join("; "))
    })
}

/// A Dioxus component that renders Typst markup as HTML.
///
/// This component compiles the provided Typst source at runtime and renders the
/// resulting HTML. Compilation errors are displayed inline.
///
/// # Props
///
/// - `source`: The Typst source code to compile.
/// - `options`: Optional [`CompileOptions`] providing additional files and packages.
/// - `class`: CSS class for the wrapper div (defaults to `"typst-content"`).
///
/// # Example
///
/// ```rust,ignore
/// use dioxus::prelude::*;
/// use dioxus_typst::{Typst, CompileOptions};
///
/// #[component]
/// fn Document() -> Element {
///     let source = r#"
/// = Introduction
///
/// This is a *Typst* document with inline math: $integral_0^1 x^2 dif x$
/// "#;
///
///     rsx! {
///         Typst {
///             source: source.to_string(),
///             class: "prose".to_string(),
///         }
///     }
/// }
/// ```
///
/// # Styling
///
/// The component outputs semantic HTML without styling. Apply CSS to the wrapper
/// class to style headings, paragraphs, code blocks, and other elements.
///
/// # Errors
///
/// Compilation errors are rendered as a `<div class="typst-error">` containing
/// the error message. Style this class to make errors visible during development.
#[component]
pub fn Typst(
    source: String,
    #[props(default)] options: CompileOptions,
    #[props(default = "typst-content".to_string())] class: String,
) -> Element {
    match compile(&source, &options) {
        Ok(html) => rsx! {
            div { class, dangerous_inner_html: "{html}" }
        },
        Err(e) => rsx! {
            div { class: "typst-error", "Error compiling Typst: {e}" }
        },
    }
}

