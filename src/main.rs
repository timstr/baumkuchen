use clap::Parser;
use regex::{Captures, Regex};
use std::{collections::HashMap, fs, io, path};
use xot::Xot;

struct Context {
    // path of the document currently being generated, relative
    // to the root of the source directory
    file_path: String,
    regex_dollar_expansion: Regex,
    regex_or_expr: Regex,
}

impl Context {
    fn new(file_path: String) -> Context {
        let regex_dollar_expansion = Regex::new(r"\$\{([a-zA-Z0-9_\-\.\|]+)}").unwrap();
        let regex_or_expr = Regex::new(r"^([a-zA-Z0-9_\-\.]+)\|\|([a-zA-Z0-9_\-\.]+)$").unwrap();

        Context {
            file_path,
            regex_dollar_expansion,
            regex_or_expr,
        }
    }
}

// Remove comments and outer whitespace from an existing node
fn minify(xot: &mut Xot, node: xot::Node) -> Result<(), xot::Error> {
    if xot.is_comment(node) {
        return xot.remove(node);
    }

    if let Some(text) = xot.text(node) {
        let orig_text = text.get();

        // Replace all runs of whitespace with just a single space
        let mut trimmed = {
            let mut s = String::new();
            let mut words = orig_text.split_whitespace();
            if let Some(w) = words.next() {
                s = w.to_string();
            }
            while let Some(w) = words.next() {
                s += " ";
                s += w;
            }
            s
        };

        // Add backing a leading space if it was removed and there is a previous node
        {
            if xot.previous_sibling(node).is_some() && orig_text.starts_with(char::is_whitespace) {
                trimmed.insert(0, ' ');
            }
        }

        // Add backing a trailing space if it was removed and there is a next node
        {
            if xot.next_sibling(node).is_some() && orig_text.ends_with(char::is_whitespace) {
                trimmed.push(' ');
            }
        }

        // Remove the node outright if it is empty or all white space
        // NOTE: this implicitly assumes that both adjacent siblings are not inline elements
        if trimmed.chars().all(char::is_whitespace) {
            return xot.remove(node);
        }

        if trimmed != orig_text {
            xot.text_mut(node).unwrap().set(trimmed);
        }
    }

    let children: Vec<xot::Node> = xot.children(node).collect();
    for child in &children {
        minify(xot, *child)?;
    }

    Ok(())
}

// Look for and replace single instances of a named tag with
// the given replacement
fn substitute_tag(
    xot: &mut Xot,
    node: xot::Node,
    tag_name: xot::NameId,
    replacement: xot::Node,
    invocation: xot::Node,
    context: &Context,
) -> Result<(), xot::Error> {
    debug_assert!(!xot.is_removed(node));
    debug_assert!(!xot.is_removed(replacement));
    let xot::Value::Element(elem) = xot.value(node) else {
        return Ok(());
    };
    if elem.name() == tag_name {
        let r = xot.clone(replacement);
        // expand and propagate any attributes
        let orig_attrs: Vec<(String, String)> = xot
            .attributes(node)
            .iter()
            .map(|(key, value)| {
                let key = xot.name_ns_str(key).0.to_string();
                let value = expand_string(xot, value, invocation, context);
                (key, value)
            })
            .collect();
        xot.replace(node, r)?;
        for (key, value) in orig_attrs {
            let key_id = xot.add_name(&key);
            xot.attributes_mut(r).insert(key_id, value);
        }
        return Ok(());
    }
    let children: Vec<xot::Node> = xot.children(node).collect();
    for child in children {
        substitute_tag(xot, child, tag_name, replacement, invocation, context)?;
    }
    Ok(())
}

fn substitute_foreach(
    xot: &mut Xot,
    node: xot::Node,
    invocation: xot::Node,
    context: &Context,
) -> Result<(), xot::Error> {
    let loop_var_str = xot
        .name_ns_str(xot.node_name(node).unwrap())
        .0
        .strip_prefix("foreachchild.")
        .unwrap();

    debug_assert!(xot.children(node).filter(|c| xot.is_element(*c)).count() == 1);

    let Some(loop_var) = xot.name(&loop_var_str) else {
        println!(
            "Warning: found tag \"<foreachchild.{}>\" but there is nothing named \"{}\"",
            loop_var_str, loop_var_str
        );
        return Ok(());
    };

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

        substitute_tag(xot, ch, loop_var, inv_child, invocation, context)?;
    }
    // xot.remove(node)?;
    xot.detach(node)?;
    return Ok(());
}

fn evaluate_expression(xot: &Xot, expr: &str, invocation: xot::Node, context: &Context) -> String {
    // 'self.filepath' evaluates to context's filepath
    if expr == "self.filepath" {
        return context.file_path.to_string();
    }

    // "A||B" evaluates expression A and returns it if defined and non-empty,
    // otherwise evaluates and returns expression B
    // TODO: if more general context-free expressions are needed,
    // implement a proper parser
    if let Some(captures) = context.regex_or_expr.captures(expr) {
        let a = &captures[1];
        let b = &captures[2];
        let a_val = evaluate_expression(xot, a, invocation, context);
        if !a_val.is_empty() {
            return a_val;
        }
        return evaluate_expression(xot, b, invocation, context);
    }

    // 'self.xyz' evaluates to contents of 'xyz' attribute of invocation element
    if let Some(attr_name) = expr.strip_prefix("self.") {
        let Some(attr_value) = xot
            .name(attr_name)
            .map(|id| xot.attributes(invocation).get(id))
            .flatten()
        else {
            // println!("Warning: reference to missing attribute \"{}\"", attr_name);
            return "".to_string();
        };

        debug_assert!(!attr_value.contains('$'));
        return attr_value.to_string();
    }

    println!("Warning: unrecognized expression: \"{}\"", expr);
    "".to_string()
}

fn expand_string(xot: &Xot, expr_string: &str, invocation: xot::Node, context: &Context) -> String {
    context
        .regex_dollar_expansion
        .replace_all(expr_string, |captures: &Captures| -> String {
            let s = evaluate_expression(xot, &captures[1], invocation, context);
            // println!("Expanding \"{}\" into \"{}\"", &captures[0], s);
            s
        })
        .to_string()
}

fn expression_matches_pattern(
    xot: &Xot,
    expr_string: &str,
    pattern_string: &str,
    invocation: xot::Node,
    context: &Context,
) -> bool {
    // println!(
    //     "Testing whether expression \"{}\" == \"{}\"",
    //     expr_string, pattern_string
    // );

    // Expand any expressions
    let expr_value = evaluate_expression(xot, expr_string, invocation, context);
    let pattern_value = expand_string(xot, pattern_string, invocation, context);

    // println!(" -> \"{}\" == \"{}\"", expr_value, pattern_value);

    // Wrap pattern in '^' and '$' to force matching the entire string
    let pattern = format!("^{}$", pattern_value);
    let re = Regex::new(&pattern).expect("Invalid regex");
    re.is_match(&expr_value)
}

fn substitute_if(
    xot: &mut Xot,
    node: xot::Node,
    invocation: xot::Node,
    context: &Context,
) -> Result<(), xot::Error> {
    // expect a single attribute of the form `expression="value-pattern"` and evaluate it
    let condition = {
        let attrs = xot.attributes(node);
        let mut attrs_iter = attrs.iter();
        let (attr_name_id, pattern) = attrs_iter.next().expect("msg");
        assert!(attrs_iter.next().is_none());
        let expr = xot.name_ns_str(attr_name_id).0;
        expression_matches_pattern(xot, expr, pattern, invocation, context)
    };

    // look for a 'then' child node
    let node_then = xot
        .name("then")
        .map(|id| {
            for child in xot.children(node) {
                if xot.node_name(child) == Some(id) {
                    return Some(child);
                }
            }
            None
        })
        .flatten();

    // look for an 'else' child node
    let node_else = xot
        .name("else")
        .map(|id| {
            for child in xot.children(node) {
                if xot.node_name(child) == Some(id) {
                    return Some(child);
                }
            }
            None
        })
        .flatten();

    if node_then.is_none() && node_else.is_none() {
        println!("Warning: <if> element without a nested <then> or <else> element");
    }

    if condition {
        // if match, replace with contents of 'then'
        if let Some(node_then) = node_then {
            let children: Vec<xot::Node> = xot.children(node_then).collect();
            for ch in children {
                let ch = xot.clone(ch);
                xot.insert_before(node, ch)?;
            }
        }
        xot.remove(node)
    } else {
        // otherwise, replace with contents of 'else'
        if let Some(node_else) = node_else {
            let children: Vec<xot::Node> = xot.children(node_else).collect();
            for ch in children {
                let ch = xot.clone(ch);
                xot.insert_before(node, ch)?;
            }
        }
        xot.remove(node)
    }
}

fn substitute_attr(
    xot: &mut Xot,
    node: xot::Node,
    invocation: xot::Node,
) -> Result<(), xot::Error> {
    let attr_name = xot
        .name_ns_str(xot.node_name(node).unwrap())
        .0
        .strip_prefix("self.")
        .unwrap();

    if attr_name == "inner" {
        // replace tags <self.inner> with the node's children
        let children: Vec<xot::Node> = xot.children(invocation).collect();
        for ch in children {
            let r = xot.clone(ch);
            xot.insert_before(node, r)?;
        }
        xot.remove(node)?;

        return Ok(());
    }

    let Some(attr_id) = xot.name(attr_name) else {
        println!(
            "Warning: undefined attribute \"{}\" referenced in node <self.{}>",
            attr_name, attr_name
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

// Recursively visit all string attributes of all descendants of a node
// and expand expressions
fn expand_all_attr_strings(
    xot: &mut Xot,
    node: xot::Node,
    invocation: xot::Node,
    context: &Context,
) -> Result<(), xot::Error> {
    // Visit all attributes
    {
        let keys: Vec<xot::NameId> = xot.attributes(node).keys().collect();
        for key in keys {
            let Some(value) = xot.attributes(node).get(key) else {
                continue;
            };
            let new_value = expand_string(xot, &value, invocation, context);
            *xot.attributes_mut(node).get_mut(key).unwrap() = new_value;
        }
    }

    let children: Vec<xot::Node> = xot.children(node).collect();
    for child in children {
        expand_all_attr_strings(xot, child, invocation, context)?;
    }

    Ok(())
}

// Process a node, recursively substituting and applying rules, and inserting
// any resulting nodes in its place
fn substitute_invocation(
    xot: &mut Xot,
    node: xot::Node,
    invocation: xot::Node,
    context: &Context,
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
            substitute_invocation(xot, child, invocation, context)?;
        }
    }

    // substitute <foreachchild.*> tags
    if elem_name.starts_with("foreachchild.") {
        return substitute_foreach(xot, node, invocation, context);
    }

    // substitute <if> tags
    if elem_name == "if" {
        return substitute_if(xot, node, invocation, context);
    }

    // Look for tags of the form <self.xyz>
    if elem_name.starts_with("self.") {
        return substitute_attr(xot, node, invocation);
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

        // Wrap the document root in a throwaway node because document roots
        // currently cannot be moved.
        // See https://github.com/faassen/xot/issues/22
        source_text.insert_str(0, "<throwaway>");
        source_text.push_str("</throwaway>");

        let document = xot.parse(&source_text).unwrap_or_else(|err| {
            panic!(
                "Failed to parse element definition at {}: {}",
                path.display(),
                err
            )
        });

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
        context: &Context,
    ) -> Result<Vec<xot::Node>, xot::Error> {
        // unwrap <throwaway> node
        let node = xot.children(self.node).next().unwrap();

        let node = xot.clone(node);

        expand_all_attr_strings(xot, node, invocation, context)?;
        substitute_invocation(xot, node, invocation, context)?;

        Ok(xot.children(node).collect())
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
    context: &Context,
) -> Result<bool, xot::Error> {
    let Some(element) = xot.element(node) else {
        return Ok(false);
    };
    let element_name = element.name();

    let mut did_anything = false;

    if let Some(element_defn) = library.elements().get(&element_name) {
        let instantiation = element_defn
            .instantiate(xot, node, context)
            .expect("Failed to instantiate node");
        for inst_node in instantiation {
            debug_assert!(!xot.is_removed(node));
            debug_assert!(!xot.is_removed(inst_node));
            xot.insert_before(node, inst_node)?;
        }
        // xot.remove(node)?;
        xot.detach(node)?;
        did_anything = true;
    }

    loop {
        let mut did_anything_inner = false;
        let children: Vec<xot::Node> = xot.children(node).collect();
        for child in children {
            if substitute(xot, child, library, context)? {
                did_anything_inner = true;
                did_anything = true;
                break;
            }
        }
        if !did_anything_inner {
            break;
        }
    }

    Ok(did_anything)
}

fn generate_file(
    xot: &mut Xot,
    source_root: &path::Path,
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
    let document = xot.parse(&source_text).unwrap_or_else(|err| {
        panic!(
            "Failed to parse html file at {}: {}",
            source_path.display(),
            err
        )
    });

    let file_path = "/".to_string()
        + &source_path
            .strip_prefix(source_root)
            .unwrap()
            .to_string_lossy()
            .to_string();

    let context = Context::new(file_path);

    let children: Vec<xot::Node> = xot.children(document).collect();
    for node in children {
        substitute(xot, node, library, &context).expect("Failed to substitute document");
    }

    minify(xot, document).expect("Failed to minify document");

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
    xot.remove(document).expect("Failed to remove document");

    Ok(())
}

fn clean_folder(path: &std::path::Path) -> Result<(), io::Error> {
    if !path.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name().to_str().unwrap().starts_with(".") {
            println!(
                "Not deleting \"{}\" at \"{}\"",
                entry.file_name().to_str().unwrap(),
                path.display()
            );
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
    source_root: &path::Path,
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
            generate_folder(
                xot,
                source_root,
                &entry_path,
                &dst_path.join(entry_name),
                library,
            )?;
        } else if entry_type.is_file() {
            if let Some(ext) = entry_path.extension() {
                if ext == "html" {
                    generate_file(
                        xot,
                        source_root,
                        &entry_path,
                        &dst_path.join(entry_name),
                        library,
                    )?;
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

    // Disable text consolidation (merging of text nodes while modifying)
    // because it wreacks havoc when modifying nodes while iterating.
    // See https://github.com/faassen/xot/issues/25
    xot.set_text_consolidation(false);

    let library =
        ElementLibrary::from_folder(&mut xot, &args.elements).expect("Failed to load elements");

    clean_folder(&args.destination).expect("Failed to clean output directory");

    generate_folder(
        &mut xot,
        &args.source,
        &args.source,
        &args.destination,
        &library,
    )
    .expect("Failed to generate");
}
