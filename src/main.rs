use std::{collections::HashMap, fs, io, path};

use clap::Parser;

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

    fn instantiate(&self) -> html_parser::Node {
        // TODO:
        // - accept and substitute attributes
        // - accept and substitute children
        // - generate and return new VDomGuard (or just string?)

        self.node.clone()
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
        *node = element_defn.instantiate();

        assert!(
            if let html_parser::Node::Element(e) = node {
                e.name != element_name
            } else {
                true
            },
            "Node was not substituted"
        );
        true
    } else {
        false
    }
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

    // TODO: how to serialize back to html???
    println!("{}", dom.to_json_pretty().unwrap());

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

    println!("source = {}", args.source.display());
    println!("elements = {}", args.elements.display());
    println!("destination = {}", args.destination.display());

    let library = ElementLibrary::from_folder(&args.elements).expect("Failed to load elements");

    println!("Elements:");
    for element in library.elements().values() {
        println!("    {}", element.tag_name());
    }

    generate_folder(&args.source, &args.destination, &library).expect("Failed to generate");
}
