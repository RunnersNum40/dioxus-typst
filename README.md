# dioxus-typst

A Dioxus component for rendering Typst documents as HTML.

> **Note:** Typst's HTML export is experimental. Pin your `typst` version and test output carefully.

## Usage

```rust
use dioxus::prelude::*;
use dioxus_typst::Typst;

let content = r#"
= Header

Some *typst* content with `code` and math: $E = m c^2$
"#;

rsx! {
    article {
        Typst { source: content.to_string() }
    }
}
```

### With Files

Provide images, bibliographies, or other files via `CompileOptions`:

```rust
use dioxus_typst::{Typst, CompileOptions};

let bib_bytes = std::fs::read("path/to/refs.bib").unwrap();
let image_bytes = std::fs::read("path/to/figure.png").unwrap();
let content = r#"
#image(figure.png)
#bibliography(refs.bib)
"#;
let options = CompileOptions::new()
    .with_file("refs.bib", bib_bytes)
    .with_file("figure.png", image_bytes);

rsx! {
    Typst {
        source: content,
        options: options,
    }
}
```

### With Packages

```rust
use dioxus_typst::{Typst, CompileOptions, PackageSpec};

let content = r#"
#import "@preview/cetz:0.4.2"

#cetz.canvas({
  import cetz.draw: *
  // Your drawing code goes here
})
"#
let options = CompileOptions::new()
    .with_package(
        PackageSpec::from_str("@preview/cetz:0.4.2").unwrap(),
        package_files,
    );

rsx! {
    Typst {
        source: content,
        options: options,
    }
}
```
