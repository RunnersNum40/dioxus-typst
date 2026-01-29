# dioxus-typst

A Dioxus component for rendering Typst documents as HTML.

> **Note:** Typst's HTML export is experimental. Pin your `typst` version and test output carefully.

## Usage

```rust
use dioxus::prelude::*;
use dioxus_typst::Typst;

#[component]
fn BlogPost() -> Element {
    let content = r#"
= My Post

Some *typst* content with `code` and math: $E = m c^2$
"#;

    rsx! {
        article {
            Typst { source: content.to_string() }
        }
    }
}
```

### With Files

Provide images, bibliographies, or other files via `CompileOptions`:

```rust
use dioxus_typst::{Typst, CompileOptions};

let options = CompileOptions::new()
    .with_file("/refs.bib", bib_bytes)
    .with_file("/figure.png", image_bytes);

rsx! {
    Typst {
        source: content,
        options: options,
    }
}
```

### Custom Class

The component wraps output in `<div class="typst-content">` by default:

```rust
rsx! {
    Typst {
        source: content,
        class: "my-custom-class".to_string(),
    }
}
```

## Styling

Typst outputs semantic HTML without CSS. Style with your own:

```css
.typst-content h1 {
  font-size: 1.75rem;
}
.typst-content h2 {
  font-size: 1.5rem;
}
.typst-content p {
  margin: 0.75rem 0;
}
.typst-content pre {
  padding: 1rem;
  background: var(--surface);
  border-radius: 4px;
  overflow-x: auto;
}
```

## Feature Flags

| Feature | Default | Description                      |
| ------- | ------- | -------------------------------- |
| `fonts` | ✓       | Bundle fonts from `typst-assets` |

## Limitations

- Typst HTML export is experimental and may change
- No automatic CSS generation from Typst styles
- External files must be provided via `CompileOptions`
