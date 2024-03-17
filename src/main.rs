use std::{collections::HashMap, fs, io, path};

use clap::Parser;

// Look for and replace single instances of a named tag with
// the given replacement
fn substitute_tag(
    node: html_parser::Node,
    tag_name: &str,
    replacement: &html_parser::Node,
) -> html_parser::Node {
    let html_parser::Node::Element(mut elem) = node else {
        return node;
    };
    if elem.name == tag_name {
        return replacement.clone();
    }
    for child in &mut elem.children {
        // TODO: avoid this clone
        *child = substitute_tag(child.clone(), tag_name, replacement);
    }
    html_parser::Node::Element(elem)
}

// Process a list of nodes, recursively substituting and applying rules,
// and return the resulting list of nodes, which may have shrunk or grown.
fn substitute_invocation(
    nodes: Vec<html_parser::Node>,
    invocation: &html_parser::Element,
) -> Vec<html_parser::Node> {
    nodes
        .into_iter()
        .map(|node| -> Vec<html_parser::Node> {
            // comments and text get passed through unmodified
            let html_parser::Node::Element(mut elem) = node else {
                return vec![node];
            };

            // substitute innermost elements
            let children = std::mem::replace(&mut elem.children, vec![]);
            elem.children = substitute_invocation(children, invocation);

            // substitute foreach tags
            if elem.name == "foreachchild" {
                assert!(elem.attributes.len() == 1);
                let (loop_var, val) = elem.attributes.iter().next().unwrap().clone();
                assert!(val.is_none());
                assert!(elem.children.len() == 1);
                return invocation
                    .children
                    .iter()
                    .map(|inv_child| {
                        let n = substitute_tag(elem.children[0].clone(), &loop_var, &inv_child);
                        n
                    })
                    .collect();
            }

            // Look for tags of the form <self.xyz>
            let Some(attr_name) = elem.name.strip_prefix("self.") else {
                // Pass the node through unmodified otherwise
                return vec![html_parser::Node::Element(elem)];
            };
            if attr_name == "inner" {
                // replace tags <self.inner> with the node's children
                invocation.children.clone()
            } else if let Some(attr_val) = invocation.attributes.get(attr_name) {
                // replace tags <self.xyz> with attribute value xyz if defined
                if let Some(attr_val) = attr_val {
                    vec![html_parser::Node::Text(attr_val.to_string())]
                } else {
                    vec![]
                }
            } else {
                println!("Warning: undefined attribute <self.{}>", attr_name);
                vec![]
            }
        })
        .flatten()
        .collect()
}

struct ElementDefinition {
    tag_name: String,
    node: html_parser::Node,
}

impl ElementDefinition {
    fn from_file(path: &std::path::Path) -> Result<ElementDefinition, io::Error> {
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let source_text = fs::read_to_string(path)?;
        let dom = html_parser::Dom::parse(&source_text).unwrap();
        assert!(dom.children.len() == 1);
        let node = dom.children.into_iter().next().unwrap();

        Ok(ElementDefinition {
            tag_name: name,
            node,
        })
    }

    fn tag_name(&self) -> &str {
        &self.tag_name
    }

    fn instantiate(&self, invocation: &html_parser::Element) -> html_parser::Node {
        let mut node = self.node.clone();

        if let html_parser::Node::Element(ref mut node) = &mut node {
            let children = std::mem::replace(&mut node.children, vec![]);
            node.children = substitute_invocation(children, invocation);
        }

        node
    }
}

struct ElementLibrary {
    elements: HashMap<String, ElementDefinition>,
}

impl ElementLibrary {
    fn from_folder(path: &std::path::Path) -> Result<ElementLibrary, io::Error> {
        let mut elements = HashMap::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();
            if let Some(ext) = entry_path.extension() {
                if ext == "html" {
                    let element_defn = ElementDefinition::from_file(&entry_path)?;
                    let prev = elements.insert(element_defn.tag_name().to_string(), element_defn);
                    assert!(prev.is_none());
                }
            }
        }
        Ok(ElementLibrary { elements })
    }

    fn elements(&self) -> &HashMap<String, ElementDefinition> {
        &self.elements
    }
}

fn substitute(node: &mut html_parser::Node, library: &ElementLibrary) -> bool {
    let html_parser::Node::Element(element) = node else {
        return false;
    };

    let mut any_substitutions = false;

    loop {
        let mut did_anything = false;
        for child in &mut element.children {
            if substitute(child, library) {
                did_anything = true;
            }
        }
        if !did_anything {
            break;
        }
    }

    let element_name = element.name.clone();

    if let Some(element_defn) = library.elements().get(&element_name) {
        // TODO: do the substitution
        *node = element_defn.instantiate(&element);

        assert!(
            if let html_parser::Node::Element(e) = node {
                e.name != element_name
            } else {
                true
            },
            "Node was not substituted"
        );

        any_substitutions = true;
    }

    if let html_parser::Node::Element(element) = node {
        loop {
            let mut did_anything = false;
            for child in &mut element.children {
                if substitute(child, library) {
                    did_anything = true;
                    any_substitutions = true;
                }
            }
            if !did_anything {
                break;
            }
        }
    }

    any_substitutions
}

fn generate_file(
    source_path: &path::Path,
    dst_path: &path::Path,
    library: &ElementLibrary,
) -> Result<(), io::Error> {
    if !source_path.is_file() {
        panic!("Source path must be a file: {}", source_path.display());
    }

    if dst_path.exists() {
        panic!("Output file already exists: {}", dst_path.display());
    }

    let source_text = fs::read_to_string(source_path)?;
    let mut dom = html_parser::Dom::parse(&source_text).unwrap();

    for node in &mut dom.children {
        substitute(node, library);
    }

    let generated_html = dom.to_html();

    println!("{}", generated_html);

    fs::write(dst_path, generated_html)?;

    Ok(())
}

fn generate_folder(
    source_path: &std::path::Path,
    dst_path: &std::path::Path,
    library: &ElementLibrary,
) -> Result<(), io::Error> {
    if !source_path.is_dir() {
        panic!("Source path must be a directory: {}", source_path.display());
    }

    if dst_path.exists() {
        panic!("Output directory already exists: {}", dst_path.display());
    }

    fs::create_dir(dst_path)?;

    for entry in fs::read_dir(source_path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let entry_type = entry.file_type()?;
        if entry_type.is_dir() {
            generate_folder(
                &entry_path,
                &dst_path.join(entry_path.file_name().unwrap()),
                library,
            )?;
        } else if entry_type.is_file() {
            if let Some(ext) = entry_path.extension() {
                if ext == "html" {
                    generate_file(
                        &entry_path,
                        &dst_path.join(entry_path.file_name().unwrap()),
                        library,
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Parser, Debug)]
#[command(about)]
struct Args {
    source: std::path::PathBuf,
    elements: std::path::PathBuf,
    destination: std::path::PathBuf,
}

fn main() {
    let args = Args::parse();

    let library = ElementLibrary::from_folder(&args.elements).expect("Failed to load elements");

    generate_folder(&args.source, &args.destination, &library).expect("Failed to generate");
}
