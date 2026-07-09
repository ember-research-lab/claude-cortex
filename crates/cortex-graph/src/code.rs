//! Feature-gated Rust source → `ember_graph::Graph` projection.
//!
//! One node per `.rs` file, one node per top-level item (fn / struct / enum /
//! trait / mod), and a `defined_in` edge from each item to its file. Enabled by
//! the `code-extract` feature so the default (core) build stays lean.

use ember_graph::{Confidence, Edge, Graph, Node, NodeId};
use std::fs;
use std::path::Path;
use syn::Item;
use walkdir::WalkDir;

/// A top-level code item discovered in a Rust source tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeItem {
    pub file: String,
    pub name: String,
    pub kind: String,
}

/// Walk `root` recursively, parse each `.rs` file (skipping any path with a
/// `target/` component), and project files + top-level items into a graph.
///
/// Parse failures for individual files are skipped — the rest of the tree is
/// still extracted.
pub fn extract_rust_dir(root: &Path) -> std::io::Result<Graph> {
    let mut g = Graph::new();

    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        if path.components().any(|c| c.as_os_str() == "target") {
            continue;
        }

        let rel = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().into_owned(),
            Err(_) => continue,
        };

        let file_id = NodeId::canonical("code", &rel);
        g.add_node(Node {
            id: file_id.clone(),
            label: rel.clone(),
            kind: "file".to_string(),
            source: Some(rel.clone()),
        });

        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let file = match syn::parse_file(&text) {
            Ok(f) => f,
            Err(_) => continue,
        };

        for item in file.items {
            let (kind, name) = match &item {
                Item::Fn(i) => ("fn", i.sig.ident.to_string()),
                Item::Struct(i) => ("struct", i.ident.to_string()),
                Item::Enum(i) => ("enum", i.ident.to_string()),
                Item::Trait(i) => ("trait", i.ident.to_string()),
                Item::Mod(i) => ("mod", i.ident.to_string()),
                _ => continue,
            };

            let item_key = format!("{rel}::{name}");
            let item_id = NodeId::canonical("code", &item_key);
            g.add_node(Node {
                id: item_id.clone(),
                label: name,
                kind: kind.to_string(),
                source: Some(rel.clone()),
            });
            g.add_edge(Edge {
                source: item_id,
                target: file_id.clone(),
                relation: "defined_in".to_string(),
                confidence: Confidence::Extracted,
            });
        }
    }

    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn extracts_fn_and_struct_with_defined_in() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let src = dir.path().join("lib.rs");
        fs::write(&src, "pub fn foo() {}\nstruct Bar;\n").expect("write");

        let g = extract_rust_dir(dir.path()).expect("extract");

        let file_id = NodeId::canonical("code", "lib.rs");
        let foo_id = NodeId::canonical("code", "lib.rs::foo");
        let bar_id = NodeId::canonical("code", "lib.rs::Bar");

        let file = g.node(&file_id).expect("file node");
        assert_eq!(file.kind, "file");
        assert_eq!(file.label, "lib.rs");

        let foo = g.node(&foo_id).expect("foo node");
        assert_eq!(foo.kind, "fn");
        assert_eq!(foo.label, "foo");

        let bar = g.node(&bar_id).expect("Bar node");
        assert_eq!(bar.kind, "struct");
        assert_eq!(bar.label, "Bar");

        let foo_linked = g
            .neighbors(&foo_id)
            .iter()
            .any(|e| e.target == file_id && e.relation == "defined_in");
        assert!(foo_linked, "foo should have defined_in → file");

        let bar_linked = g
            .neighbors(&bar_id)
            .iter()
            .any(|e| e.target == file_id && e.relation == "defined_in");
        assert!(bar_linked, "Bar should have defined_in → file");
    }
}
