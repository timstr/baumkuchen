use clap::Parser;
use kuchikiki::{traits::*, NodeRef};
use std::{cell::RefCell, collections::HashMap, fs, io, path};

// Look for and replace single instances of a named tag with
// the given replacement
fn substitute_tag(node: kuchikiki::NodeRef, tag_name: &str, replacement: &kuchikiki::NodeRef) {
    let kuchikiki::NodeData::Element(elem) = node.data() else {
        return;
    };
    if &*elem.name.local == tag_name {
        node.insert_before(replacement.deep_clone());
        node.detach();
        return;
    }
    for child in node.children() {
        substitute_tag(child, tag_name, replacement);
    }
}

// Process a node, recursively substituting and applying rules, and inserting
// any resulting nodes in its place
fn substitute_invocation(node: kuchikiki::NodeRef, invocation: kuchikiki::NodeRef) {
    // comments and text get passed through unmodified
    let elem_name: String = if let Some(elem) = node.as_element() {
        elem.name.local.to_string()
    } else {
        return;
    };

    // substitute innermost elements
    for child in node.children() {
        substitute_invocation(child, invocation.clone());
    }

    // substitute foreach tags
    if elem_name == "foreachchild" {
        let attributes = node.as_element().unwrap().attributes.borrow();
        assert!(attributes.map.len() == 1);
        let (loop_var, val) = attributes.map.iter().next().unwrap().clone();

        assert!(val.value.is_empty());

        debug_assert!(node.children().filter(|c| c.as_element().is_some()).count() == 1);

        let node_child = node
            .children()
            .filter(|c| c.as_element().is_some())
            .next()
            .unwrap();

        for inv_child in invocation.children() {
            if inv_child.as_element().is_none() {
                continue;
            }
            let ch = node_child.deep_clone();
            node.insert_before(ch.clone());
            substitute_tag(ch, &loop_var.local, &inv_child);
        }
        node.detach();
        return;
    }

    // Look for tags of the form <self.xyz>
    let Some(attr_name) = elem_name.strip_prefix("self.") else {
        // Pass the node through unmodified otherwise
        return;
    };

    if attr_name == "inner" {
        // replace tags <self.inner> with the node's children
        for ch in invocation.children() {
            node.insert_before(ch.deep_clone());
        }
        node.detach();
        return;
    } else if let Some(attr_val) = invocation
        .as_element()
        .unwrap()
        .attributes
        .borrow()
        .get(attr_name)
    {
        // replace tags <self.xyz> with attribute value xyz if defined
        if !attr_val.is_empty() {
            node.insert_before(NodeRef::new(kuchikiki::NodeData::Text(RefCell::new(
                attr_val.to_string(),
            ))));
        }
        node.detach();
    } else {
        println!(
            "Warning: undefined attribute self.{} on node <{}>",
            attr_name, elem_name
        );
        println!("Valid attributes are:");
        for attr in invocation
            .as_element()
            .unwrap()
            .attributes
            .borrow()
            .map
            .iter()
        {
            println!("    {:?}", attr);
        }
    }
}

struct ElementDefinition {
    tag_name: String,
    node: kuchikiki::NodeRef,
}

impl ElementDefinition {
    fn from_file(path: &std::path::Path) -> Result<ElementDefinition, io::Error> {
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let source_text = fs::read_to_string(path)?;

        let document = kuchikiki::parse_fragment(
            kuchikiki::QualName {
                prefix: None,
                ns: kuchikiki::Namespace::from("html"),
                local: kuchikiki::LocalName::from(""),
            },
            Default::default(),
        )
        .one(source_text);

        // outer document
        let document = document.children().next().unwrap();
        // outer html element
        let document = document.children().next().unwrap();

        Ok(ElementDefinition {
            tag_name: name,
            node: document,
        })
    }

    fn tag_name(&self) -> &str {
        &self.tag_name
    }

    fn instantiate(&self, invocation: kuchikiki::NodeRef) -> kuchikiki::NodeRef {
        let node = self.node.deep_clone();

        for child in node.children() {
            substitute_invocation(child, invocation.clone());
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

fn substitute(node: kuchikiki::NodeRef, library: &ElementLibrary) -> bool {
    let Some(element) = node.as_element() else {
        return false;
    };
    let element_name = element.name.local.to_string();

    let mut any_substitutions = false;

    // TODO: does this need to be done both before and after?
    loop {
        let mut did_anything = false;
        for child in node.children() {
            if substitute(child, library) {
                did_anything = true;
            }
        }
        if !did_anything {
            break;
        }
    }

    if let Some(element_defn) = library.elements().get(&element_name) {
        let instatiation = element_defn.instantiate(node.clone());
        node.insert_before(instatiation);
        node.detach();

        any_substitutions = true;
    }

    // TODO: see above
    loop {
        let mut did_anything = false;
        for child in node.children() {
            if substitute(child, library) {
                did_anything = true;
            }
        }
        if !did_anything {
            break;
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

    // if dst_path.exists() {
    //     panic!("Output file already exists: {}", dst_path.display());
    // }

    let source_text = fs::read_to_string(source_path)?;
    let document = kuchikiki::parse_html().one(source_text);

    for node in document.children() {
        substitute(node, library);
    }

    let mut generated_html = Vec::<u8>::new();
    document.serialize(&mut generated_html)?;
    let generated_html = String::from_utf8(generated_html).expect("Generated html is not UTF-8");

    // println!("{}", generated_html);

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

    // if dst_path.exists() {
    //     panic!("Output directory already exists: {}", dst_path.display());
    // }

    if !dst_path.exists() {
        fs::create_dir(dst_path)?;
    }

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
