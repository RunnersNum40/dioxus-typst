//! A Dioxus component for rendering Typst documents as HTML.
//!
//! This crate provides a [`Typst`] component that compiles Typst markup to HTML
//! at runtime, allowing you to embed rich typeset content in Dioxus applications.
//!
//! # Example
//!
//! ```rust
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

use std::collections::HashMap;

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
/// ```rust
/// use dioxus_typst::CompileOptions;
///
/// let options = CompileOptions::new()
///     .with_file("data.csv", csv_bytes)
///     .with_file("logo.png", image_bytes);
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
    /// # Example
    ///
    /// ```rust
    /// let options = CompileOptions::new()
    ///     .with_file("figure.png", png_bytes)
    ///     .with_file("data.csv", csv_bytes);
    /// ```
    #[must_use]
    pub fn with_file(mut self, path: impl Into<String>, content: Vec<u8>) -> Self {
        self.files.insert(normalize_path(path.into()), content);
        self
    }

    /// Adds a pre-loaded package to the compilation environment.
    ///
    /// # Example
    ///
    /// ```rust
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
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Typst(msg) => write!(f, "Typst compilation error: {msg}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// The compilation world that provides all resources to the Typst compiler.
struct CompileWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: Source,
    files: HashMap<String, Bytes>,
    packages: HashMap<PackageSpec, HashMap<String, Bytes>>,
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
            packages,
        }
    }

    /// Retrieves a file from a package.
    fn get_package_file(&self, package: &PackageSpec, path: &str) -> FileResult<Bytes> {
        if let Some(pkg_files) = self.packages.get(package)
            && let Some(content) = pkg_files.get(path)
        {
            return Ok(content.clone());
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
    Vec::new()
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
/// ```rust
/// use dioxus::prelude::*;
/// use dioxus_typst::Typst;
///
/// #[component]
/// fn App() -> Element {
///     let content = r#"
/// = Hello, Typst!
///
/// Some *formatted* text with math: $E = m c^2$
/// "#;
///
///     rsx! {
///         Typst { source: content.to_string() }
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
