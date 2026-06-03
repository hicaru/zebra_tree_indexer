use std::collections::HashMap;

use tree_sitter::Node;
use zti_ts_core::config::{JAVASCRIPT_CONFIG, LangConfig};
use zti_ts_core::walker::LanguageFrontend;

pub struct JavaScriptFrontend;

impl LanguageFrontend for JavaScriptFrontend {
    fn language(&self) -> tree_sitter::Language {
        tree_sitter_javascript::LANGUAGE.into()
    }

    fn config(&self) -> &'static LangConfig {
        &JAVASCRIPT_CONFIG
    }

    fn extract_imports(&self, root: Node, source: &str) -> HashMap<String, String> {
        let mut imports = HashMap::with_capacity(8);
        collect_js_imports(root, source, &mut imports);
        imports
    }
}

fn collect_js_imports(node: Node, source: &str, imports: &mut HashMap<String, String>) {
    if node.kind() == "import_statement" {
        let text = node.utf8_text(source.as_bytes()).unwrap_or("");
        for child in node.children(&mut node.walk()) {
            if child.kind() == "import_clause" {
                for cc in child.children(&mut child.walk()) {
                    match cc.kind() {
                        "identifier" => {
                            if let Ok(name) = cc.utf8_text(source.as_bytes()) {
                                imports.entry(name.to_string()).or_insert(text.to_string());
                            }
                        }
                        "named_imports" => {
                            for spec in cc.children(&mut cc.walk()) {
                                if spec.kind() == "import_specifier" {
                                    let key = spec
                                        .child_by_field_name("alias")
                                        .or_else(|| spec.child_by_field_name("name"));
                                    if let Some(kn) = key
                                        && let Ok(name) = kn.utf8_text(source.as_bytes())
                                    {
                                        imports.entry(name.to_string()).or_insert(text.to_string());
                                    }
                                }
                            }
                        }
                        "namespace_import" => {
                            let ns_name = cc.child_by_field_name("name").or_else(|| {
                                let mut c = cc.walk();
                                cc.children(&mut c).find(|ch| ch.kind() == "identifier")
                            });
                            if let Some(id) = ns_name
                                && let Ok(name) = id.utf8_text(source.as_bytes())
                            {
                                imports.entry(name.to_string()).or_insert(text.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_js_imports(child, source, imports);
    }
}

#[cfg(test)]
mod tests {
    use zti_ts_core::types::{Edge, Kind, Symbol, Target};

    use super::*;

    fn parse_js(source: &str) -> (Vec<Symbol>, Vec<Edge>, HashMap<String, String>) {
        JavaScriptFrontend.parse(source, 0, 0).unwrap()
    }

    #[test]
    fn javascript_function_captured() {
        let source = "function greet(name) { return 'hi'; }";
        let (symbols, _, _) = parse_js(source);
        let greet = symbols.iter().find(|s| s.name == "greet").unwrap();
        assert_eq!(greet.kind, Kind::Function);
    }

    #[test]
    fn javascript_class_method_is_method() {
        let source = indoc::indoc! {"
            class Greeter {
                hello() { return 1; }
                bye() { return 2; }
            }
        "};
        let (symbols, _, _) = parse_js(source);
        let cls = symbols.iter().find(|s| s.kind == Kind::Class).unwrap();
        assert_eq!(cls.name, "Greeter");
        let methods: Vec<&str> = symbols
            .iter()
            .filter(|s| s.kind == Kind::Method && s.parent == Some(cls.id))
            .map(|s| s.name.as_str())
            .collect();
        assert!(methods.contains(&"hello"));
        assert!(methods.contains(&"bye"));
    }

    #[test]
    fn javascript_const_captured() {
        let source = "const MAX = 100;";
        let (symbols, _, _) = parse_js(source);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"MAX"));
    }

    #[test]
    fn javascript_local_var_not_captured() {
        let source = indoc::indoc! {"
            function foo() {
                const x = 42;
                return x;
            }
        "};
        let (symbols, _, _) = parse_js(source);
        let locals: Vec<&str> = symbols
            .iter()
            .filter(|s| s.name == "x")
            .map(|s| s.name.as_str())
            .collect();
        assert!(locals.is_empty());
    }

    #[test]
    fn javascript_call_edge_captured() {
        let source = indoc::indoc! {"
            function helper() { return 1; }
            function caller() { return helper(); }
        "};
        let (symbols, edges, _) = parse_js(source);
        let caller = symbols.iter().find(|s| s.name == "caller").unwrap();
        let calls = edges.iter().any(|e| {
            e.from == caller.id && matches!(&e.to, Target::Unresolved(name) if name == "helper")
        });
        assert!(calls);
    }

    #[test]
    fn javascript_doc_comment_extracted() {
        let source = indoc::indoc! {"
            /** JSDoc */
            function foo() {}
        "};
        let (symbols, _, _) = parse_js(source);
        let foo = symbols.iter().find(|s| s.name == "foo").unwrap();
        assert!(foo.doc.as_ref().is_some_and(|d| d.contains("JSDoc")));
    }

    #[test]
    fn javascript_import_default() {
        let source = "import React from 'react';";
        let (_, _, imports) = parse_js(source);
        assert_eq!(imports.get("React"), Some(&source.to_string()));
    }

    #[test]
    fn javascript_import_named() {
        let source = "import { Foo, Bar } from './mod';";
        let (_, _, imports) = parse_js(source);
        assert!(imports.contains_key("Foo"));
        assert!(imports.contains_key("Bar"));
    }

    #[test]
    fn javascript_import_namespace() {
        let source = "import * as fs from 'fs';";
        let (_, _, imports) = parse_js(source);
        assert_eq!(imports.get("fs"), Some(&source.to_string()));
    }

    #[test]
    fn javascript_import_aliased() {
        let source = "import { Foo as Bar } from './mod';";
        let (_, _, imports) = parse_js(source);
        assert_eq!(imports.get("Bar"), Some(&source.to_string()));
    }
}
