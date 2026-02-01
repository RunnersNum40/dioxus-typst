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

fn normalize_path(path: String) -> String {
    if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompileOptions {
    pub files: HashMap<String, Vec<u8>>,
    pub packages: HashMap<PackageSpec, HashMap<String, Vec<u8>>>,
}

impl CompileOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_file(mut self, path: impl Into<String>, content: Vec<u8>) -> Self {
        self.files.insert(normalize_path(path.into()), content);
        self
    }

    pub fn with_package(mut self, spec: PackageSpec, files: HashMap<String, Vec<u8>>) -> Self {
        let files = files
            .into_iter()
            .map(|(path, content)| (normalize_path(path), content))
            .collect();
        self.packages.insert(spec, files);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    Typst(String),
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

    #[cfg(feature = "download-packages")]
    #[allow(dead_code)]
    fn with_downloads(mut self, allow: bool) -> Self {
        self.allow_downloads = allow;
        self
    }

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
