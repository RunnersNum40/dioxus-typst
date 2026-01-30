use std::collections::HashMap;

use chrono::{Datelike, Timelike};
use dioxus::prelude::*;
use typst::{
    Feature, Library, LibraryExt, World,
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime},
    syntax::{FileId, Source, VirtualPath},
    text::{Font, FontBook},
    utils::LazyHash,
};
use typst_html::HtmlDocument;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompileOptions {
    pub files: HashMap<String, Vec<u8>>,
}

impl CompileOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_file(mut self, path: impl Into<String>, content: Vec<u8>) -> Self {
        self.files.insert(path.into(), content);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
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

struct CompileWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: Source,
    files: HashMap<String, Bytes>,
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
        let library = Library::builder()
            .with_features([Feature::Html].into_iter().collect())
            .build();

        Self {
            library: LazyHash::new(library),
            book: LazyHash::new(book),
            fonts,
            main,
            files,
        }
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
            Ok(self.main.clone())
        } else {
            Err(FileError::NotFound(id.vpath().as_rooted_path().into()))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
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
