//! Validation logic for KDL Schema v2.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use miette::Severity;
#[cfg(not(feature = "span"))]
use miette::SourceSpan;

use super::types::Validations;
use super::KdlSchemaV2;
use crate::{KdlDiagnostic, KdlDocument, KdlNode, KdlValue};

/// Validate a KDL document against a v2 schema.
pub(super) fn validate_document(schema: &KdlSchemaV2, target: &KdlDocument) -> Vec<KdlDiagnostic> {
    let mut errors = Vec::new();

    let Some(doc_node) = schema.document_node() else {
        return errors;
    };

    let Some(schema_children) = doc_node.children() else {
        return errors;
    };

    // Get target input for diagnostics
    let target_input = Arc::new(target.to_string());

    // Look for the children block in document
    if let Some(children_block) = schema_children.get("children") {
        validate_children_block(schema, target, children_block, &target_input, &mut errors);
    } else {
        // If no explicit children block, treat document children as node definitions directly
        let mut node_defs: Vec<&KdlNode> = schema_children
            .nodes()
            .iter()
            .filter(|n| n.name().value() == "node")
            .collect();

        // Process undefine blocks
        for undef in schema_children
            .nodes()
            .iter()
            .filter(|n| n.name().value() == "undefine")
        {
            if let Some(undef_children) = undef.children() {
                for undef_node in undef_children
                    .nodes()
                    .iter()
                    .filter(|n| n.name().value() == "node")
                {
                    let name_to_remove = undef_node.get(0).and_then(|v| v.as_string());
                    node_defs.retain(|d| d.get(0).and_then(|v| v.as_string()) != name_to_remove);
                }
            }
        }

        let disallow_others = schema_children
            .get("disallow-others")
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Validate each node in the target document
        for target_node in target.nodes() {
            validate_node(
                schema,
                target_node,
                &node_defs,
                disallow_others,
                &target_input,
                &mut errors,
            );
        }

        // Check required nodes and cardinality
        check_node_cardinality(target, &node_defs, &target_input, &mut errors);
    }

    errors
}

/// Validate a children block against target children.
fn validate_children_block(
    schema: &KdlSchemaV2,
    target: &KdlDocument,
    children_block: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let Some(block_children) = children_block.children() else {
        return;
    };

    // Collect node definitions
    let mut node_defs: Vec<&KdlNode> = block_children
        .nodes()
        .iter()
        .filter(|n| n.name().value() == "node")
        .collect();

    // Process undefine blocks - remove any node definitions that are undefined
    for undef in block_children
        .nodes()
        .iter()
        .filter(|n| n.name().value() == "undefine")
    {
        if let Some(undef_children) = undef.children() {
            for undef_node in undef_children
                .nodes()
                .iter()
                .filter(|n| n.name().value() == "node")
            {
                let name_to_remove = undef_node.get(0).and_then(|v| v.as_string());
                node_defs.retain(|d| d.get(0).and_then(|v| v.as_string()) != name_to_remove);
            }
        }
    }

    // Check disallow-others (v2: default is false, meaning others are allowed)
    // If node exists without a value, it defaults to true
    let disallow_others = block_children
        .get("disallow-others")
        .is_some_and(|n| n.get(0).and_then(|v| v.as_bool()).unwrap_or(true));

    // Validate names if specified
    if let Some(names_node) = block_children.get("names") {
        let validations = build_validations(schema, names_node);
        for node in target.nodes() {
            let name_value = KdlValue::String(node.name().value().to_string());
            #[cfg(feature = "span")]
            let span = node.name().span();
            #[cfg(not(feature = "span"))]
            let span = SourceSpan::new(0.into(), 0);
            errors.extend(validations.check(&name_value, span, input));
        }
    }

    // Check min/max children count
    if let Some(min_node) = block_children.get("min") {
        if let Some(min) = min_node.get(0).and_then(|v| v.as_integer()) {
            if (target.nodes().len() as i128) < min {
                #[cfg(feature = "span")]
                let span = target.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);
                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Too few children: expected at least {}, found {}",
                        min,
                        target.nodes().len()
                    )),
                    label: Some("not enough children".into()),
                    help: None,
                    severity: Severity::Error,
                });
            }
        }
    }

    if let Some(max_node) = block_children.get("max") {
        if let Some(max) = max_node.get(0).and_then(|v| v.as_integer()) {
            if (target.nodes().len() as i128) > max {
                #[cfg(feature = "span")]
                let span = target.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);
                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Too many children: expected at most {}, found {}",
                        max,
                        target.nodes().len()
                    )),
                    label: Some("too many children".into()),
                    help: None,
                    severity: Severity::Error,
                });
            }
        }
    }

    // Check for disallow block
    if let Some(disallow_node) = block_children.get("disallow") {
        check_disallow(target, disallow_node, input, errors);
    }

    // Check for one-of block (alternative validation)
    if let Some(one_of_node) = block_children.get("one-of") {
        validate_one_of(schema, target, one_of_node, input, errors);
        // If one-of exists, skip regular node validation
        return;
    }

    // Validate each node
    for target_node in target.nodes() {
        validate_node(
            schema,
            target_node,
            &node_defs,
            disallow_others,
            input,
            errors,
        );
    }

    // Check required nodes and cardinality
    check_node_cardinality(target, &node_defs, input, errors);
}

/// Resolve a `ref` property by looking up the definition by id or KQL path.
fn resolve_ref<'a>(schema: &'a KdlSchemaV2, node: &KdlNode) -> Option<&'a KdlNode> {
    // Check for ref child node (v2 uses ref as a child, not property)
    let ref_child = node.children()?.get("ref")?;

    // Get the path from the ref's first argument
    let ref_path = ref_child.get(0)?.as_string()?;

    let definitions = schema.definitions()?;
    let def_children = definitions.children()?;

    // Try KQL path resolution first
    if let Ok(query) = ref_path.parse::<crate::query::KdlQuery>() {
        if let Ok(Some(result)) = def_children.query(query) {
            return Some(result);
        }
    }

    // Fallback: id-based lookup (existing behavior)
    def_children
        .nodes()
        .iter()
        .find(|n| n.entry("id").and_then(|e| e.value().as_string()) == Some(ref_path))
}

/// Build validations by merging the node's constraints with any referenced definition.
fn build_validations(schema: &KdlSchemaV2, node: &KdlNode) -> Validations {
    let mut validations = Validations::from_node(node);

    // If there's a ref, merge constraints from the referenced definition
    if let Some(ref_node) = resolve_ref(schema, node) {
        validations.merge(&Validations::from_node(ref_node));
    }

    validations
}

/// Find a matching node definition for a given node name.
fn find_matching_node_def<'a>(node_defs: &[&'a KdlNode], node_name: &str) -> Option<&'a KdlNode> {
    // First try to find an exact match
    node_defs
        .iter()
        .find(|def| {
            def.get(0)
                .and_then(|v| v.as_string())
                .map(|s| s == node_name)
                .unwrap_or(false)
        })
        .copied()
        // Fall back to wildcard (no name argument)
        .or_else(|| node_defs.iter().find(|def| def.get(0).is_none()).copied())
}

/// Validate a single node against schema definitions.
fn validate_node(
    schema: &KdlSchemaV2,
    node: &KdlNode,
    node_defs: &[&KdlNode],
    disallow_others: bool,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let node_name = node.name().value();

    // Find matching schema definition
    let schema_def = find_matching_node_def(node_defs, node_name);

    match schema_def {
        Some(def) => {
            // Check for deprecated node
            check_deprecated(node, def, input, errors);

            // Validate node's type annotation if schema specifies one
            validate_node_annotation(schema, node, def, input, errors);

            // Validate node's entries (args and props)
            validate_node_entries(schema, node, def, input, errors);

            // Validate children recursively
            validate_node_children(schema, node, def, input, errors);
        }
        None if disallow_others => {
            #[cfg(feature = "span")]
            let span = node.span();
            #[cfg(not(feature = "span"))]
            let span = SourceSpan::new(0.into(), 0);

            errors.push(KdlDiagnostic {
                input: input.clone(),
                span,
                message: Some(format!("Unknown node: '{}'", node_name)),
                label: Some("not defined in schema".into()),
                help: Some("Add this node to the schema or remove 'disallow-others'".into()),
                severity: Severity::Error,
            });
        }
        None => {}
    }
}

/// Validate a node's type annotation against schema constraints.
fn validate_node_annotation(
    schema: &KdlSchemaV2,
    node: &KdlNode,
    schema_def: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let Some(def_children) = schema_def.children() else {
        return;
    };

    // Check for annotations node in schema
    let annotations_node = def_children.get("annotations");

    if let Some(ann_node) = annotations_node {
        let annotations_validations = build_validations(schema, ann_node);

        // Get node's type annotation (the (type) prefix)
        if let Some(node_ty) = node.ty() {
            let ty_value = KdlValue::String(node_ty.value().to_string());
            #[cfg(feature = "span")]
            let span = node_ty.span();
            #[cfg(not(feature = "span"))]
            let span = SourceSpan::new(0.into(), 0);
            errors.extend(annotations_validations.check(&ty_value, span, input));
        } else if annotations_validations.ty.is_some()
            || !annotations_validations.enum_values.is_empty()
        {
            // Annotation required but missing (schema specifies type or enum)
            #[cfg(feature = "span")]
            let span = node.span();
            #[cfg(not(feature = "span"))]
            let span = SourceSpan::new(0.into(), 0);

            errors.push(KdlDiagnostic {
                input: input.clone(),
                span,
                message: Some("Missing required type annotation".into()),
                label: Some("type annotation expected".into()),
                help: Some("Add a type annotation like (type)nodename".into()),
                severity: Severity::Error,
            });
        }
    }
}

/// Check if a node is deprecated and emit a warning.
fn check_deprecated(
    node: &KdlNode,
    schema_def: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let deprecated_node = schema_def.children().and_then(|c| c.get("deprecated"));

    if let Some(dep) = deprecated_node {
        let message = dep
            .entry("message")
            .and_then(|e| e.value().as_string())
            .map(|s| s.to_string());

        let replacement = dep
            .entry("by")
            .and_then(|e| e.value().as_string())
            .map(|s| s.to_string());

        #[cfg(feature = "span")]
        let span = node.span();
        #[cfg(not(feature = "span"))]
        let span = SourceSpan::new(0.into(), 0);

        let help = replacement.map(|r| format!("Use '{}' instead", r));
        let msg =
            message.unwrap_or_else(|| format!("Node '{}' is deprecated", node.name().value()));

        errors.push(KdlDiagnostic {
            input: input.clone(),
            span,
            message: Some(msg),
            label: Some("deprecated".into()),
            help,
            severity: Severity::Warning,
        });
    }
}

/// Validate node entries (args and props) against schema.
fn validate_node_entries(
    schema: &KdlSchemaV2,
    node: &KdlNode,
    schema_def: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let Some(def_children) = schema_def.children() else {
        return;
    };

    // Resolve ref to get inherited definitions
    let ref_node = resolve_ref(schema, schema_def);
    let ref_children = ref_node.and_then(|n| n.children());

    // Collect arg definitions (ordered) - from both current def and ref
    let mut arg_defs: Vec<&KdlNode> = def_children
        .nodes()
        .iter()
        .filter(|n| n.name().value() == "arg")
        .collect();

    // Add arg defs from referenced definition if present
    if let Some(rc) = ref_children {
        for arg_node in rc.nodes().iter().filter(|n| n.name().value() == "arg") {
            // Only add if we don't already have one at this position
            if !arg_defs.iter().any(|_| false) {
                // For now, just append ref args to the list
                arg_defs.push(arg_node);
            }
        }
    }

    // Get args definition (for variadic args) - prefer local, fallback to ref
    let args_def = def_children
        .get("args")
        .or_else(|| ref_children.and_then(|rc| rc.get("args")));

    // Collect prop definitions - from both current def and ref
    let mut prop_defs: Vec<&KdlNode> = def_children
        .nodes()
        .iter()
        .filter(|n| n.name().value() == "prop")
        .collect();

    // Add prop defs from referenced definition if present
    if let Some(rc) = ref_children {
        for prop_node in rc.nodes().iter().filter(|n| n.name().value() == "prop") {
            let ref_prop_name = prop_node.get(0).and_then(|v| v.as_string());
            // Only add if we don't already have a prop with the same name
            if !prop_defs.iter().any(|p| {
                p.get(0).and_then(|v| v.as_string()) == ref_prop_name && ref_prop_name.is_some()
            }) {
                prop_defs.push(prop_node);
            }
        }
    }

    // Get props aggregate (for general property validations) - prefer local, fallback to ref
    let props_def = def_children
        .get("props")
        .or_else(|| ref_children.and_then(|rc| rc.get("props")));

    // Check if other props are disallowed
    let disallow_other_props = props_def
        .and_then(|p| p.children())
        .and_then(|c| c.get("disallow-others"))
        .and_then(|n| n.get(0))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Validate props:names if specified
    if let Some(props) = props_def {
        if let Some(names_node) = props.children().and_then(|c| c.get("names")) {
            let validations = build_validations(schema, names_node);
            for entry in node.entries() {
                if let Some(name) = entry.name() {
                    let name_value = KdlValue::String(name.value().to_string());
                    #[cfg(feature = "span")]
                    let span = name.span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);
                    errors.extend(validations.check(&name_value, span, input));
                }
            }
        }
    }

    // Track found props and args
    let mut found_props: HashMap<&str, usize> = HashMap::new();
    let mut arg_idx: usize = 0;

    for entry in node.entries() {
        if let Some(name) = entry.name() {
            // This is a property
            let prop_name = name.value();
            *found_props.entry(prop_name).or_insert(0) += 1;

            // Find matching prop definition
            let prop_def = prop_defs
                .iter()
                .find(|def| {
                    def.get(0)
                        .and_then(|v| v.as_string())
                        .map(|s| s == prop_name)
                        .unwrap_or(false)
                })
                .or_else(|| prop_defs.iter().find(|def| def.get(0).is_none()));

            match prop_def {
                Some(def) => {
                    let validations = build_validations(schema, def);
                    #[cfg(feature = "span")]
                    let span = entry.span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);
                    errors.extend(validations.check(entry.value(), span, input));
                }
                None if disallow_other_props => {
                    #[cfg(feature = "span")]
                    let span = entry.span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);

                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!("Unknown property: '{}'", prop_name)),
                        label: Some("not defined in schema".into()),
                        help: Some(
                            "Add this property to the schema or allow other properties".into(),
                        ),
                        severity: Severity::Error,
                    });
                }
                None => {}
            }
        } else {
            // This is a positional argument
            // In v2, args are required by default unless marked optional
            if let Some(arg_def) = arg_defs.get(arg_idx) {
                let validations = build_validations(schema, arg_def);
                #[cfg(feature = "span")]
                let span = entry.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);
                errors.extend(validations.check(entry.value(), span, input));
            } else if let Some(args) = args_def {
                // Variadic args
                let validations = build_validations(schema, args);
                #[cfg(feature = "span")]
                let span = entry.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);
                errors.extend(validations.check(entry.value(), span, input));
            }

            arg_idx += 1;
        }
    }

    // Check required args - in v2, args are required by default
    for (idx, arg_def) in arg_defs.iter().enumerate() {
        // If optional node exists without a value, it defaults to true
        let is_optional = arg_def.children().is_some_and(|c| {
            c.get("optional")
                .is_some_and(|n| n.get(0).and_then(|v| v.as_bool()).unwrap_or(true))
        });

        // Check if arg has a default value - if so, it's not required
        let has_default = arg_def
            .children()
            .is_some_and(|c| c.get("default").is_some());

        if !is_optional && !has_default && idx >= arg_idx {
            #[cfg(feature = "span")]
            let span = node.span();
            #[cfg(not(feature = "span"))]
            let span = SourceSpan::new(0.into(), 0);

            errors.push(KdlDiagnostic {
                input: input.clone(),
                span,
                message: Some(format!("Missing required argument at position {}", idx + 1)),
                label: Some("missing argument".into()),
                help: Some("Add the required argument or mark it as optional in the schema".into()),
                severity: Severity::Error,
            });
        }
    }

    // Check args min/max and distinct
    if let Some(args) = args_def {
        if let Some(min) = args
            .children()
            .and_then(|c| c.get("min"))
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_integer())
        {
            let total_args = arg_idx;
            if (total_args as i128) < min {
                #[cfg(feature = "span")]
                let span = node.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);

                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Too few arguments: expected at least {}, found {}",
                        min, total_args
                    )),
                    label: Some("not enough arguments".into()),
                    help: None,
                    severity: Severity::Error,
                });
            }
        }

        if let Some(max) = args
            .children()
            .and_then(|c| c.get("max"))
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_integer())
        {
            let total_args = arg_idx;
            if (total_args as i128) > max {
                #[cfg(feature = "span")]
                let span = node.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);

                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Too many arguments: expected at most {}, found {}",
                        max, total_args
                    )),
                    label: Some("too many arguments".into()),
                    help: None,
                    severity: Severity::Error,
                });
            }
        }

        // Check distinct - require unique argument values
        let is_distinct = args.children().is_some_and(|c| c.get("distinct").is_some());

        if is_distinct {
            check_distinct_args(node, input, errors);
        }
    }

    // Check required props
    // First from individual prop definitions
    for prop_def in &prop_defs {
        let prop_name = prop_def.get(0).and_then(|v| v.as_string());
        // If required node exists without a value, it defaults to true
        let is_required = prop_def.children().is_some_and(|c| {
            c.get("required")
                .is_some_and(|n| n.get(0).and_then(|v| v.as_bool()).unwrap_or(true))
        });

        // Check if prop has a default value - if so, it's not required
        let has_default = prop_def
            .children()
            .is_some_and(|c| c.get("default").is_some());

        if is_required && !has_default {
            if let Some(name) = prop_name {
                if !found_props.contains_key(name) {
                    #[cfg(feature = "span")]
                    let span = node.span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);

                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!("Missing required property: '{}'", name)),
                        label: Some("required property not found".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }
        }
    }

    // Then from props:required list
    if let Some(props) = props_def {
        if let Some(required_node) = props.children().and_then(|c| c.get("required")) {
            for entry in required_node.entries() {
                if entry.name().is_none() {
                    if let Some(name) = entry.value().as_string() {
                        if !found_props.contains_key(name) {
                            #[cfg(feature = "span")]
                            let span = node.span();
                            #[cfg(not(feature = "span"))]
                            let span = SourceSpan::new(0.into(), 0);

                            errors.push(KdlDiagnostic {
                                input: input.clone(),
                                span,
                                message: Some(format!("Missing required property: '{}'", name)),
                                label: Some("required property not found".into()),
                                help: None,
                                severity: Severity::Error,
                            });
                        }
                    }
                }
            }
        }
    }
}

/// Validate node children recursively.
fn validate_node_children(
    schema: &KdlSchemaV2,
    node: &KdlNode,
    schema_def: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let target_children = node.children();
    let schema_children_def = schema_def.children().and_then(|c| c.get("children"));

    match (target_children, schema_children_def) {
        (Some(target_doc), Some(children_def)) => {
            validate_children_block(schema, target_doc, children_def, input, errors);
        }
        (Some(_target_doc), None) => {
            // Target has children but schema doesn't define any
            // In v2, this might be an error depending on strictness
        }
        (None, Some(_children_def)) => {
            // Schema expects children but target has none
            // Check if any children are required
        }
        (None, None) => {}
    }
}

/// Check node cardinality (required/repeatable).
fn check_node_cardinality(
    target: &KdlDocument,
    node_defs: &[&KdlNode],
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    // Count occurrences of each node type
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for node in target.nodes() {
        *counts.entry(node.name().value()).or_insert(0) += 1;
    }

    // Check each node definition
    for def in node_defs {
        let node_name = def.get(0).and_then(|v| v.as_string());

        // Handle nodes with or without children blocks
        let def_children = def.children();

        // Check required (v2) - if required node exists without value, defaults to true
        let is_required = def_children.is_some_and(|c| {
            c.get("required")
                .is_some_and(|n| n.get(0).and_then(|v| v.as_bool()).unwrap_or(true))
        });

        // Check repeatable (v2) - if repeatable node exists, node can appear multiple times
        let is_repeatable = def_children.is_some_and(|c| c.get("repeatable").is_some());

        let repeatable_min = def_children
            .and_then(|c| c.get("repeatable"))
            .and_then(|n| n.entry("min"))
            .and_then(|e| e.value().as_integer())
            .map(|v| v as usize);

        let repeatable_max = def_children
            .and_then(|c| c.get("repeatable"))
            .and_then(|n| n.entry("max"))
            .and_then(|e| e.value().as_integer())
            .map(|v| v as usize);

        if let Some(name) = node_name {
            let count = counts.get(name).copied().unwrap_or(0);

            // Check required
            if is_required && count == 0 {
                #[cfg(feature = "span")]
                let span = target.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);

                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!("Missing required node: '{}'", name)),
                    label: Some("required node not found".into()),
                    help: None,
                    severity: Severity::Error,
                });
            }

            // Check repeatable constraints
            if !is_repeatable && count > 1 {
                #[cfg(feature = "span")]
                let span = target.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);

                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Node '{}' is not repeatable but appears {} times",
                        name, count
                    )),
                    label: Some("duplicate node".into()),
                    help: Some("Mark the node as 'repeatable' or remove duplicates".into()),
                    severity: Severity::Error,
                });
            }

            if let Some(min) = repeatable_min {
                if count < min {
                    #[cfg(feature = "span")]
                    let span = target.span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);

                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!(
                            "Too few '{}' nodes: expected at least {}, found {}",
                            name, min, count
                        )),
                        label: Some("not enough nodes".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }

            if let Some(max) = repeatable_max {
                if count > max {
                    #[cfg(feature = "span")]
                    let span = target.span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);

                    errors.push(KdlDiagnostic {
                        input: input.clone(),
                        span,
                        message: Some(format!(
                            "Too many '{}' nodes: expected at most {}, found {}",
                            name, max, count
                        )),
                        label: Some("too many nodes".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }
        }
    }
}

/// Check for distinct argument values (no duplicates).
fn check_distinct_args(node: &KdlNode, input: &Arc<String>, errors: &mut Vec<KdlDiagnostic>) {
    let mut seen: HashSet<String> = HashSet::new();

    for entry in node.entries() {
        if entry.name().is_none() {
            // This is a positional argument
            let value_str = format!("{}", entry.value());
            if !seen.insert(value_str.clone()) {
                #[cfg(feature = "span")]
                let span = entry.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);

                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!("Duplicate argument value: {}", value_str)),
                    label: Some("duplicate value".into()),
                    help: Some("Arguments must be distinct".into()),
                    severity: Severity::Error,
                });
            }
        }
    }
}

/// Check for disallowed nodes.
fn check_disallow(
    target: &KdlDocument,
    disallow_node: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let Some(disallow_children) = disallow_node.children() else {
        return;
    };

    // Collect node definitions inside disallow
    for def in disallow_children.nodes() {
        if def.name().value() == "node" {
            if let Some(disallowed_name) = def.get(0).and_then(|v| v.as_string()) {
                // Check if any target nodes match the disallowed name
                for target_node in target.nodes() {
                    if target_node.name().value() == disallowed_name {
                        #[cfg(feature = "span")]
                        let span = target_node.span();
                        #[cfg(not(feature = "span"))]
                        let span = SourceSpan::new(0.into(), 0);

                        let message = def
                            .children()
                            .and_then(|c| c.get("message"))
                            .and_then(|n| n.get(0))
                            .and_then(|v| v.as_string())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| {
                                format!("Node '{}' is not allowed here", disallowed_name)
                            });

                        errors.push(KdlDiagnostic {
                            input: input.clone(),
                            span,
                            message: Some(message),
                            label: Some("disallowed node".into()),
                            help: Some("Remove this node".into()),
                            severity: Severity::Error,
                        });
                    }
                }
            }
        }
    }
}

/// Validate one-of (alternative validation).
fn validate_one_of(
    schema: &KdlSchemaV2,
    target: &KdlDocument,
    one_of_node: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let Some(alternatives) = one_of_node.children() else {
        return;
    };

    // Try each alternative - at least one must pass
    let mut any_passed = false;
    let mut all_errors: Vec<Vec<KdlDiagnostic>> = Vec::new();

    for alt in alternatives.nodes() {
        let mut alt_errors = Vec::new();

        // Each alternative is a choice block containing node definitions
        if let Some(alt_children) = alt.children() {
            let node_defs: Vec<&KdlNode> = alt_children
                .nodes()
                .iter()
                .filter(|n| n.name().value() == "node")
                .collect();

            // Try validating against this alternative
            for target_node in target.nodes() {
                validate_node(
                    schema,
                    target_node,
                    &node_defs,
                    false, // don't disallow others within alternatives
                    input,
                    &mut alt_errors,
                );
            }

            // Check cardinality
            check_node_cardinality(target, &node_defs, input, &mut alt_errors);
        }

        if alt_errors.is_empty() {
            any_passed = true;
            break;
        }

        all_errors.push(alt_errors);
    }

    if !any_passed && !all_errors.is_empty() {
        #[cfg(feature = "span")]
        let span = target.span();
        #[cfg(not(feature = "span"))]
        let span = SourceSpan::new(0.into(), 0);

        errors.push(KdlDiagnostic {
            input: input.clone(),
            span,
            message: Some("None of the alternatives matched".into()),
            label: Some("no valid alternative".into()),
            help: Some("Document must match one of the defined alternatives".into()),
            severity: Severity::Error,
        });
    }
}
