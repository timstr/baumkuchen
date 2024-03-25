use clap::Parser;
use std::{collections::HashMap, fs, io, path};
use xot::Xot;

// Look for and replace single instances of a named tag with
// the given replacement
fn substitute_tag(
    xot: &mut Xot,
    node: xot::Node,
    tag_name: xot::NameId,
    replacement: xot::Node,
) -> Result<(), xot::Error> {
    debug_assert!(!xot.is_removed(node));
    debug_assert!(!xot.is_removed(replacement));
    let xot::Value::Element(elem) = xot.value(node) else {
        return Ok(());
    };
    if elem.name() == tag_name {
        let r = xot.clone(replacement);
        xot.replace(node, r)?;
        return Ok(());
    }
    let children: Vec<xot::Node> = xot.children(node).collect();
    for child in children {
        substitute_tag(xot, child, tag_name, replacement)?;
    }
    Ok(())
}

// Process a node, recursively substituting and applying rules, and inserting
// any resulting nodes in its place
fn substitute_invocation(
    xot: &mut Xot,
    node: xot::Node,
    invocation: xot::Node,
) -> Result<(), xot::Error> {
    debug_assert!(!xot.is_removed(node));
    // comments and text get passed through unmodified
    let elem_name: String = if let xot::Value::Element(elem) = xot.value(node) {
        xot.name_ns_str(elem.name()).0.to_string()
    } else {
        return Ok(());
    };

    // substitute innermost elements
    {
        let children: Vec<xot::Node> = xot.children(node).collect();
        for child in children {
            substitute_invocation(xot, child, invocation)?;
        }
    }

    // substitute foreach tags
    if elem_name == "foreachchild" {
        let attributes = xot.attributes(node);
        assert!(attributes.len() == 1);
        let (loop_var, val) = attributes.iter().next().unwrap().clone();

        assert!(val.is_empty());

        debug_assert!(xot.children(node).filter(|c| xot.is_element(*c)).count() == 1);

        let node_child = xot
            .children(node)
            .filter(|c| xot.is_element(*c))
            .next()
            .unwrap();

        let children: Vec<xot::Node> = xot.children(invocation).collect();
        for inv_child in children {
            // don't replace outer white space, text, or comments
            if !xot.is_element(inv_child) {
                continue;
            }
            let ch = xot.clone(node_child);
            xot.insert_before(node, ch)?;
            substitute_tag(xot, ch, loop_var, inv_child)?;
        }
        // xot.remove(node)?;
        xot.detach(node)?;
        return Ok(());
    }

    // Look for tags of the form <self.xyz>
    let Some(attr_name) = elem_name.strip_prefix("self.") else {
        // Pass the node through unmodified otherwise
        return Ok(());
    };

    if attr_name == "inner" {
        // replace tags <self.inner> with the node's children
        let children: Vec<xot::Node> = xot.children(invocation).collect();
        for ch in children {
            let r = xot.clone(ch);
            xot.insert_before(node, r)?;
        }
        // xot.remove(node)?;
        xot.detach(node)?;
        return Ok(());
    }

    let Some(attr_id) = xot.name(attr_name) else {
        println!(
            "Warning: undefined attribute self.{} on node <{}>",
            attr_name, elem_name
        );
        return Ok(());
    };

    if let Some(attr_val) = xot.attributes(invocation).get(attr_id).cloned() {
        // replace tags <self.xyz> with attribute value xyz if defined
        if !attr_val.is_empty() {
            let r = xot.new_text(&attr_val);
            xot.insert_before(node, r)?;
        }
        // xot.remove(node)?;
        xot.detach(node)?;
    }

    Ok(())
}

struct ElementDefinition {
    tag_name: xot::NameId,
    node: xot::Node,
}

impl ElementDefinition {
    fn from_file(xot: &mut Xot, path: &std::path::Path) -> Result<ElementDefinition, io::Error> {
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let mut source_text = fs::read_to_string(path)?;

        // https://github.com/faassen/xot/issues/22
        source_text.insert_str(0, "<throwaway>");
        source_text.push_str("</throwaway>");

        let document = xot
            .parse(&source_text)
            .expect("Failed to parse element definition");

        Ok(ElementDefinition {
            tag_name: xot.add_name(&name),
            node: document,
        })
    }

    fn tag_name(&self) -> xot::NameId {
        self.tag_name
    }

    fn instantiate(
        &self,
        xot: &mut Xot,
        invocation: xot::Node,
    ) -> Result<Vec<xot::Node>, xot::Error> {
        // unwrap <throwaway> node
        let node = xot.children(self.node).next().unwrap();
        // let node = xot.children(node).next().unwrap();

        let node = xot.clone(node);

        let children: Vec<xot::Node> = xot.children(node).collect();
        for child in &children {
            substitute_invocation(xot, *child, invocation)?;
        }

        Ok(children)
    }
}

struct ElementLibrary {
    elements: HashMap<xot::NameId, ElementDefinition>,
}

impl ElementLibrary {
    fn from_folder(xot: &mut Xot, path: &std::path::Path) -> Result<ElementLibrary, io::Error> {
        let mut elements = HashMap::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();
            if let Some(ext) = entry_path.extension() {
                if ext == "html" {
                    let element_defn = ElementDefinition::from_file(xot, &entry_path)?;
                    let prev = elements.insert(element_defn.tag_name(), element_defn);
                    assert!(prev.is_none());
                }
            }
        }
        Ok(ElementLibrary { elements })
    }

    fn elements(&self) -> &HashMap<xot::NameId, ElementDefinition> {
        &self.elements
    }
}

fn substitute(
    xot: &mut Xot,
    node: xot::Node,
    library: &ElementLibrary,
) -> Result<bool, xot::Error> {
    let Some(element) = xot.element(node) else {
        return Ok(false);
    };
    let element_name = element.name();

    let mut did_anything = false;

    // TODO: does this need to be done both before and after?
    loop {
        let mut did_anything_inner = false;
        let children: Vec<xot::Node> = xot.children(node).collect();
        for child in children {
            if substitute(xot, child, library)? {
                did_anything_inner = true;
                did_anything = true;
                break;
            }
        }
        if !did_anything_inner {
            break;
        }
    }

    if let Some(element_defn) = library.elements().get(&element_name) {
        let instantiation = element_defn
            .instantiate(xot, node)
            .expect("Failed to instantiate node");
        for inst_node in instantiation {
            xot.insert_before(node, inst_node)?;
        }
        // xot.remove(node)?;
        xot.detach(node)?;
        did_anything = true;
    }

    // TODO: see above

    Ok(did_anything)
}

fn generate_file(
    xot: &mut Xot,
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
    let document = xot.parse(&source_text).expect("Failed to parse html file");

    let children: Vec<xot::Node> = xot.children(document).collect();

    for node in children {
        substitute(xot, node, library).expect("Failed to substitute document");
    }

    // let mut generated_html = xot.to_string(document).expect("Failed to serialize html");
    let generated_html = xot
        .html5()
        .serialize_string(
            xot::output::html5::Parameters {
                indentation: None,
                cdata_section_elements: vec![],
            },
            document,
        )
        .expect("Failed to serialize html");

    fs::write(dst_path, generated_html)?;

    // remove document node to free memory (hopefully?)
    // xot.remove(document).expect("Failed to remove document");

    Ok(())
}

fn clean_folder(path: &std::path::Path) -> Result<(), io::Error> {
    if !path.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name().to_str().unwrap().starts_with(".") {
            continue;
        }
        let entry_type = entry.file_type()?;
        if entry_type.is_file() {
            fs::remove_file(entry.path())?;
        } else if entry_type.is_dir() {
            fs::remove_dir_all(entry.path())?;
        }
    }

    Ok(())
}

fn generate_folder(
    xot: &mut Xot,
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
        let entry_name = entry_path.file_name().unwrap();
        if entry_type.is_dir() {
            generate_folder(xot, &entry_path, &dst_path.join(entry_name), library)?;
        } else if entry_type.is_file() {
            if let Some(ext) = entry_path.extension() {
                if ext == "html" {
                    generate_file(xot, &entry_path, &dst_path.join(entry_name), library)?;
                    continue;
                }
            }

            fs::copy(&entry_path, dst_path.join(entry_name))?;
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

    let mut xot = Xot::new();

    let library =
        ElementLibrary::from_folder(&mut xot, &args.elements).expect("Failed to load elements");

    clean_folder(&args.destination).expect("Failed to clean output directory");

    generate_folder(&mut xot, &args.source, &args.destination, &library)
        .expect("Failed to generate");
}
