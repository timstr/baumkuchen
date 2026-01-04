use clap::Parser;
use regex::{Captures, Regex};
use std::{
    collections::HashMap,
    fmt::Debug,
    fs::{self, read_dir},
    io, panic,
    path::{self, Path, PathBuf},
};
use xot::Xot;

#[derive(Clone)]
struct Context {
    // path of the document currently being generated, relative
    // to the root of the source directory
    regex_dollar_expansion: Regex,
    regex_or_expr: Regex,
    regex_variable: Regex,
    regex_sort_key: Regex,
    source_root: PathBuf,
    variables: HashMap<String, String>,
}

impl Context {
    fn new(source_root: PathBuf) -> Context {
        let regex_dollar_expansion = Regex::new(r"\$\{([a-zA-Z0-9_\-\.\|]+)}").unwrap();
        let regex_or_expr = Regex::new(r"^([a-zA-Z0-9_\-\.]+)\|\|([a-zA-Z0-9_\-\.]+)$").unwrap();
        let regex_variable = Regex::new(r"^[a-zA-Z]+\.[a-zA-Z]+$").unwrap();
        let regex_sort_key =
            Regex::new(r"^(-?)([a-zA-Z][a-zA-Z0-9]*)\.([a-zA-Z][a-zA-Z0-9]*)$").unwrap();

        Context {
            regex_dollar_expansion,
            regex_or_expr,
            regex_variable,
            regex_sort_key,
            source_root,
            variables: HashMap::new(),
        }
    }

    fn define_variable(&mut self, name: String, value: String) {
        self.variables.insert(name, value);
    }

    fn get_variable(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(|s| s.as_ref())
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
        minify(xot, *child).expect("Failed to minify");
    }

    Ok(())
}

// Look for and replace single instances of a named tag with
// the given replacement
fn substitute_tag<F: FnMut(&mut Xot, xot::Node) -> Vec<xot::Node>>(
    xot: &mut Xot,
    node: xot::Node,
    tag_name: xot::NameId,
    f: &mut F,
    invocation: xot::Node,
    context: &Context,
) -> Result<(), xot::Error> {
    debug_assert!(!xot.is_removed(node));

    let xot::Value::Element(elem) = xot.value(node) else {
        return Ok(());
    };
    if elem.name() == tag_name {
        let replacement = f(xot, node);
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

        // NOTE: there seems to be a bug here where calling
        // xot.replace(node, r)?;
        // where 'r' is a text node and 'node' is only child
        // results in all attributes on the parent
        // node being cleared. Inserting and then detaching
        // circumvents that.
        for r in &replacement {
            xot.insert_after(node, *r)
                .expect("Failed to insert replacement node during substitution");
        }
        xot.detach(node)
            .expect("Failed to detach node being replaced");

        if let [replacement] = &replacement[..] {
            for (key, value) in orig_attrs {
                let key_id = xot.add_name(&key);
                xot.attributes_mut(*replacement).insert(key_id, value);
            }
        }
        return Ok(());
    }
    let children: Vec<xot::Node> = xot.children(node).collect();
    for child in children {
        substitute_tag(xot, child, tag_name, f, invocation, context)?;
    }
    Ok(())
}

fn substitute_foreachchild(
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

    assert!(xot.children(node).filter(|c| xot.is_element(*c)).count() == 1);

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

        xot.insert_before(node, ch)
            .expect("Failed to insert substituted node");

        substitute_tag(
            xot,
            ch,
            loop_var,
            &mut |xot, _| vec![xot.clone(inv_child)],
            invocation,
            context,
        )?;
    }
    // xot.remove(node)?;
    xot.detach(node)
        .expect("Failed to detach node after substituting");
    return Ok(());
}

fn substitute_foreachfile(
    xot: &mut Xot,
    node: xot::Node,
    invocation: xot::Node,
    context: &Context,
) -> Result<(), xot::Error> {
    // <foreachfile.f dir="/blog/" sortby="-blogpost.date" max="3" exclude="hidden.html">
    //     ...
    // <foreachfile.f>

    let loop_var_str = xot
        .name_ns_str(xot.node_name(node).unwrap())
        .0
        .strip_prefix("foreachfile.")
        .unwrap()
        .to_string();

    let mut dir_attr = xot
        .attributes(node)
        .get(
            xot.name("dir")
                .expect("foreachfile is missing dir attribute"),
        )
        .expect("foreachfile is missing dir attribute")
        .clone();

    let mut sortyby_tag_attr = None;
    let mut reverse = false;

    if let Some(sortby_id) = xot.name("sortby") {
        if let Some(sortby_val) = xot.attributes(node).get(sortby_id) {
            let Some(captures) = context.regex_sort_key.captures(sortby_val) else {
                panic!(
                    "<foreachfile.*> 'sortby' attribute must be of the form 'tagname.attributename'"
                );
            };
            reverse = &captures[1] == "-";
            sortyby_tag_attr = Some((captures[2].to_string(), captures[3].to_string()));
        }
    }

    let mut max_count = None;

    if let Some(max_id) = xot.name("max") {
        if let Some(max_val) = xot.attributes(node).get(max_id) {
            let Ok(n) = max_val.parse::<usize>() else {
                panic!("<foreachfile.*> 'max' attribute must be a positive integer");
            };

            max_count = Some(n);
        }
    }

    let mut exclusions = Vec::new();

    if let Some(exclude_id) = xot.name("exclude") {
        if let Some(exclude_val) = xot.attributes(node).get(exclude_id) {
            exclusions = exclude_val.split(",").map(str::to_string).collect();
        }
    }

    // TODO: prevent '..' escapes?
    if let Some(stripped) = dir_attr.strip_prefix("/") {
        dir_attr = stripped.to_string();
    }

    let mut file_paths = Vec::<PathBuf>::new();

    let dir_path = context.source_root.join(dir_attr);
    for dir_ent in read_dir(dir_path).unwrap() {
        let dir_ent = dir_ent.unwrap();
        let file_name = dir_ent.file_name();
        let file_name = file_name.to_str().unwrap();
        if dir_ent.file_type().unwrap().is_file() && file_name.ends_with(".html") {
            if exclusions.iter().any(|s| s == file_name) {
                continue;
            }
            file_paths.push(dir_ent.path());
        }
    }

    if let Some((sortby_tag, sortby_attr)) = sortyby_tag_attr {
        if let (Some(tag_id), Some(attr_id)) = (xot.name(&sortby_tag), xot.name(&sortby_attr)) {
            file_paths.sort_by_cached_key(|filepath| -> String {
                let doc = DocumentFragment::from_file(xot, filepath).unwrap();
                let contents = doc.get_contents_inside_node(xot);
                for d in xot.all_descendants(contents) {
                    if xot.node_name(d) == Some(tag_id) {
                        if let Some(val) = xot.attributes(d).get(attr_id) {
                            return val.clone();
                        }
                    }
                }

                "".to_string()
            });
        }

        if reverse {
            file_paths.reverse();
        }
    }

    if let Some(n) = max_count {
        if n < file_paths.len() {
            file_paths.drain(n..);
        }
    }

    let node_children: Vec<xot::Node> = xot.children(node).filter(|c| xot.is_element(*c)).collect();

    for file_path in file_paths {
        for node_child in &node_children {
            let ch = xot.clone(*node_child);

            let path_var_name = format!("{}.path", loop_var_str);
            let mut path_var_value = file_path
                .strip_prefix(&context.source_root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            path_var_value.insert(0, '/');

            let mut inner_context = context.clone();
            inner_context.define_variable(path_var_name.clone(), path_var_value.to_string());

            if let Some(path_name_id) = xot.name(&path_var_name) {
                substitute_tag(
                    xot,
                    ch,
                    path_name_id,
                    &mut |xot, _| vec![xot.new_text(&path_var_value)],
                    invocation,
                    context,
                )?;
            }

            if let Some(loop_var_id) = xot.name(&loop_var_str) {
                let file_contents = DocumentFragment::from_file(xot, &file_path).unwrap();

                substitute_tag(
                    xot,
                    ch,
                    loop_var_id,
                    &mut |xot, elem| {
                        let contents = file_contents.get_contents_inside_node(xot);

                        if let Some(excerpttag_id) = xot.name("excerpttag") {
                            if let Some(excerpttag_value) = xot.attributes(elem).get(excerpttag_id)
                            {
                                if let Some(excerpttag_name_id) = xot.name(excerpttag_value) {
                                    for d in xot.all_descendants(contents) {
                                        if xot.node_name(d) == Some(excerpttag_name_id) {
                                            return vec![d];
                                        }
                                    }
                                }

                                return vec![];
                            }
                        }

                        xot.children(contents).collect()
                    },
                    invocation,
                    context,
                )?;
            }

            expand_all_attr_strings(xot, ch, invocation, &inner_context)?;

            xot.insert_before(node, ch)
                .expect("Failed to insert node during substitution");
        }
    }
    // xot.remove(node)?;
    xot.detach(node)
        .expect("Failed to detach node after substitution");
    Ok(())
}

fn evaluate_expression(xot: &Xot, expr: &str, invocation: xot::Node, context: &Context) -> String {
    if let Some(value) = context.get_variable(expr) {
        return value.to_string();
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

    // remove unexpanded variables.
    if context.regex_variable.is_match(expr) {
        return "".to_string();
    }

    // If nothing else matches, leave the expression as-is as a literal
    expr.to_string()
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
        let (attr_name_id, pattern) = attrs_iter
            .next()
            .expect("<if> tag must contain an attribute");
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
                xot.insert_before(node, ch).expect("Failed to insert node");
            }
        }
        xot.remove(node)
    } else {
        // otherwise, replace with contents of 'else'
        if let Some(node_else) = node_else {
            let children: Vec<xot::Node> = xot.children(node_else).collect();
            for ch in children {
                let ch = xot.clone(ch);
                xot.insert_before(node, ch).expect("Failed to insert node");
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
            xot.insert_before(node, r).expect("Failed to insert node");
        }
        xot.remove(node).expect("Failed to remove node");

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
            xot.insert_before(node, r).expect("Failed to insert node");
        }
        // xot.remove(node)?;
        xot.detach(node).expect("Failed to detach node");
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
        return substitute_foreachchild(xot, node, invocation, context);
    }

    // substitute <foreachfile.*> tags
    if elem_name.starts_with("foreachfile.") {
        return substitute_foreachfile(xot, node, invocation, context);
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

struct DocumentFragment {
    root_node: xot::Node,
}

impl DocumentFragment {
    fn from_file(xot: &mut Xot, path: &Path) -> Result<DocumentFragment, io::Error> {
        let mut source_text = fs::read_to_string(path)?;

        // Wrap the document root in a throwaway node because document roots
        // currently cannot be moved.
        // See https://github.com/faassen/xot/issues/22
        source_text.insert_str(0, "<throwaway>");
        source_text.push_str("</throwaway>");

        let root_node = xot.parse(&source_text).unwrap_or_else(|err| {
            panic!(
                "Failed to parse element definition at {}: {}",
                path.display(),
                err
            )
        });

        Ok(DocumentFragment { root_node })
    }

    fn get_contents_inside_node(&self, xot: &mut Xot) -> xot::Node {
        // unwrap anonymous outer node
        let node = xot.children(self.root_node).next().unwrap();

        // clone throwaway node
        let node = xot.clone(node);

        // Return the throwaway node
        // This is mainly so that the outer throwaway node can be used
        // for performing substitutions on the children without
        // losing track of what those children are.
        node
    }
}

struct ElementDefinition {
    tag_name: xot::NameId,
    document: DocumentFragment,
}

impl ElementDefinition {
    fn from_file(xot: &mut Xot, path: &Path) -> Result<ElementDefinition, io::Error> {
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();

        let document = DocumentFragment::from_file(xot, path)?;

        Ok(ElementDefinition {
            tag_name: xot.add_name(&name),
            document,
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
        // create nested context with variables defined by attributes on the invocation
        let mut inner_context = context.clone();

        for (attr_name_id, attr_value) in xot.attributes(invocation).iter() {
            let attr_name = xot.name_ns_str(attr_name_id).0;
            inner_context.define_variable(format!("self.{}", attr_name), attr_value.clone());
        }

        let outer_node = self.document.get_contents_inside_node(xot);

        substitute_invocation(xot, outer_node, invocation, &inner_context)?;
        expand_all_attr_strings(xot, outer_node, invocation, &inner_context)?;

        Ok(xot.children(outer_node).collect())
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

    // TODO: this fails when an output file's root element is being
    // substituted. Add a workaround for that.

    if let Some(element_defn) = library.elements().get(&element_name) {
        let instantiation = element_defn
            .instantiate(xot, node, context)
            .expect("Failed to instantiate node");
        for inst_node in instantiation {
            debug_assert!(!xot.is_removed(node));
            debug_assert!(!xot.is_removed(inst_node));
            xot.insert_before(node, inst_node)
                .expect("Failed to insert node");
        }
        // xot.remove(node)?;
        xot.detach(node).expect("Failed to detach node");
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

    let mut context = Context::new(source_root.to_path_buf());
    context.define_variable("self.filepath".to_string(), file_path);

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

/// Static html site generator with basic procedural html fragment substitution
#[derive(Parser, Debug)]
#[command(about)]
struct Args {
    /// Path to a directory of files to copy and expand.
    /// HTML files are parsed and any tags with names
    /// matching files in the elements directory are expanded
    /// according to those definitions. Other files are copied
    /// without modifications.
    source: std::path::PathBuf,

    /// Path to a directory of html element files. These
    /// resemble HTML fragments but with additional expressions
    /// that are unique to baumkuchen, such as <if>. The
    /// name of each html file without the suffix is used
    /// to expand instances of tags with the same name.
    /// For example, if the elements directory contains a
    /// file called widget.html, and a html file in the source
    /// directory includes a <widget> tag, that tag will be
    /// replaced with the expanded contents of widget.html
    elements: std::path::PathBuf,

    /// Path to a directory where expanded/copied files from
    /// the source directory should be written to
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
