//! Validation logic for KDL Schema.

use std::collections::HashMap;
use std::sync::Arc;

use miette::Severity;
#[cfg(not(feature = "span"))]
use miette::SourceSpan;

use super::types::Validations;
use super::KdlSchema;
use crate::{KdlDiagnostic, KdlDocument, KdlNode, KdlValue};

/// Resolve a `ref` property by looking up the definition by id.
fn resolve_ref<'a>(schema: &'a KdlSchema, node: &KdlNode) -> Option<&'a KdlNode> {
    let ref_id = node.entry("ref")?.value().as_string()?;
    schema
        .definitions()?
        .children()?
        .nodes()
        .iter()
        .find(|n| n.entry("id").and_then(|e| e.value().as_string()) == Some(ref_id))
}

/// Build validations by merging the node's constraints with any referenced definition.
fn build_validations(schema: &KdlSchema, node: &KdlNode) -> Validations {
    let mut validations = Validations::from_node(node);

    // If there's a ref, merge constraints from the referenced definition
    if let Some(ref_node) = resolve_ref(schema, node) {
        validations.merge(&Validations::from_node(ref_node));
    }

    validations
}

/// Validate a KDL document against a schema.
pub(super) fn validate_document(schema: &KdlSchema, target: &KdlDocument) -> Vec<KdlDiagnostic> {
    let mut errors = Vec::new();

    let Some(doc_node) = schema.document_node() else {
        return errors;
    };

    let Some(schema_children) = doc_node.children() else {
        return errors;
    };

    // Collect node definitions from schema
    let node_defs: Vec<&KdlNode> = schema_children
        .nodes()
        .iter()
        .filter(|n| n.name().value() == "node")
        .collect();

    // Collect tag definitions from schema
    let tag_defs: Vec<&KdlNode> = schema_children
        .nodes()
        .iter()
        .filter(|n| n.name().value() == "tag")
        .collect();

    // Check if other nodes are allowed
    let other_nodes_allowed = schema_children
        .get("other-nodes-allowed")
        .and_then(|n| n.get(0))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Check if other tags are allowed
    let other_tags_allowed = schema_children
        .get("other-tags-allowed")
        .and_then(|n| n.get(0))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Get target input for diagnostics
    let target_input = Arc::new(target.to_string());

    // Validate node-names if specified
    if let Some(node_names) = schema_children.get("node-names") {
        let validations = build_validations(schema, node_names);
        for node in target.nodes() {
            let name_value = KdlValue::String(node.name().value().to_string());
            #[cfg(feature = "span")]
            let span = node.name().span();
            #[cfg(not(feature = "span"))]
            let span = SourceSpan::new(0.into(), 0);
            errors.extend(validations.check(&name_value, span, &target_input));
        }
    }

    // Validate tag-names if specified
    if let Some(tag_names) = schema_children.get("tag-names") {
        let validations = build_validations(schema, tag_names);
        for node in target.nodes() {
            if let Some(ty) = node.ty() {
                let tag_value = KdlValue::String(ty.value().to_string());
                #[cfg(feature = "span")]
                let span = ty.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);
                errors.extend(validations.check(&tag_value, span, &target_input));
            }
        }
    }

    // Validate each node in the target document
    for target_node in target.nodes() {
        validate_node(
            schema,
            target_node,
            &node_defs,
            other_nodes_allowed,
            &target_input,
            &mut errors,
        );

        // Validate node tags
        validate_node_tag(
            schema,
            target_node,
            &tag_defs,
            other_tags_allowed,
            &target_input,
            &mut errors,
        );
    }

    // Check node count constraints
    check_node_counts(target, &node_defs, &target_input, &mut errors);

    errors
}

/// Get the effective node definition, resolving any ref to get merged children.
/// Returns the node def to use for validation (either the original or the ref target).
fn get_effective_node_def<'a>(schema: &'a KdlSchema, node_def: &'a KdlNode) -> &'a KdlNode {
    // If this node def has a ref, return the referenced definition
    // The ref'd node provides the children (value, prop, children, tag constraints)
    if let Some(ref_node) = resolve_ref(schema, node_def) {
        ref_node
    } else {
        node_def
    }
}

/// Validate a single node against schema definitions.
fn validate_node(
    schema: &KdlSchema,
    node: &KdlNode,
    node_defs: &[&KdlNode],
    other_allowed: bool,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let node_name = node.name().value();

    // Find matching schema definition
    let schema_def = find_matching_node_def(node_defs, node_name);

    match schema_def {
        Some(def) => {
            // Get effective definition (resolves ref if present)
            let effective_def = get_effective_node_def(schema, def);

            // Validate node's type annotation (tag) against node definition's tag constraints
            if let Some(def_children) = effective_def.children() {
                if let Some(tag_def) = def_children.get("tag") {
                    let tag_validations = build_validations(schema, tag_def);
                    if let Some(ty) = node.ty() {
                        let tag_value = KdlValue::String(ty.value().to_string());
                        #[cfg(feature = "span")]
                        let tag_span = ty.span();
                        #[cfg(not(feature = "span"))]
                        let tag_span = SourceSpan::new(0.into(), 0);
                        errors.extend(tag_validations.check(&tag_value, tag_span, input));
                    }
                }
            }

            // Validate node entries (properties and values)
            validate_node_entries(schema, node, effective_def, input, errors);

            // Validate children recursively
            validate_node_children(schema, node, effective_def, input, errors);
        }
        None if !other_allowed => {
            #[cfg(feature = "span")]
            let span = node.span();
            #[cfg(not(feature = "span"))]
            let span = SourceSpan::new(0.into(), 0);

            errors.push(KdlDiagnostic {
                input: input.clone(),
                span,
                message: Some(format!("Unknown node: '{}'", node_name)),
                label: Some("not defined in schema".into()),
                help: Some("Add this node to the schema or set 'other-nodes-allowed #true'".into()),
                severity: Severity::Error,
            });
        }
        None => {}
    }
}

/// Get effective tag definition by resolving any ref attribute.
fn get_effective_tag_def<'a>(schema: &'a KdlSchema, tag_def: &'a KdlNode) -> &'a KdlNode {
    if let Some(ref_node) = resolve_ref(schema, tag_def) {
        ref_node
    } else {
        tag_def
    }
}

/// Validate node's type annotation (tag) against schema tag definitions.
fn validate_node_tag(
    schema: &KdlSchema,
    node: &KdlNode,
    tag_defs: &[&KdlNode],
    other_tags_allowed: bool,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let Some(ty) = node.ty() else {
        return;
    };

    let tag_name = ty.value();

    // Find matching tag definition (by name or wildcard)
    let tag_def = tag_defs
        .iter()
        .find(|def| {
            def.get(0)
                .and_then(|v| v.as_string())
                .map(|s| s == tag_name)
                .unwrap_or(false)
        })
        .or_else(|| tag_defs.iter().find(|def| def.get(0).is_none()));

    match tag_def {
        Some(def) => {
            // Resolve ref if present to get the effective tag definition
            let effective_def = get_effective_tag_def(schema, def);

            // Tag is defined - validate node-names constraints if present
            if let Some(def_children) = effective_def.children() {
                if let Some(node_names) = def_children.get("node-names") {
                    let validations = build_validations(schema, node_names);
                    let name_value = KdlValue::String(node.name().value().to_string());
                    #[cfg(feature = "span")]
                    let span = node.name().span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);
                    errors.extend(validations.check(&name_value, span, input));
                }

                // Check other-nodes-allowed within tag
                let other_nodes_allowed = def_children
                    .get("other-nodes-allowed")
                    .and_then(|n| n.get(0))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Collect node definitions within the tag
                let tag_node_defs: Vec<&KdlNode> = def_children
                    .nodes()
                    .iter()
                    .filter(|n| n.name().value() == "node")
                    .collect();

                // If tag has node constraints, validate against them
                if !tag_node_defs.is_empty() || !other_nodes_allowed {
                    let node_name = node.name().value();
                    let matching_def = find_matching_node_def(&tag_node_defs, node_name);

                    if matching_def.is_none() && !other_nodes_allowed && !tag_node_defs.is_empty() {
                        #[cfg(feature = "span")]
                        let span = node.span();
                        #[cfg(not(feature = "span"))]
                        let span = SourceSpan::new(0.into(), 0);

                        errors.push(KdlDiagnostic {
                            input: input.clone(),
                            span,
                            message: Some(format!(
                                "Node '{}' not allowed for tag '{}'",
                                node_name, tag_name
                            )),
                            label: Some("not allowed for this tag".into()),
                            help: None,
                            severity: Severity::Error,
                        });
                    }
                }
            }
        }
        None if !other_tags_allowed => {
            #[cfg(feature = "span")]
            let span = ty.span();
            #[cfg(not(feature = "span"))]
            let span = SourceSpan::new(0.into(), 0);

            errors.push(KdlDiagnostic {
                input: input.clone(),
                span,
                message: Some(format!("Unknown tag: '{}'", tag_name)),
                label: Some("not defined in schema".into()),
                help: Some("Add this tag to the schema or set 'other-tags-allowed #true'".into()),
                severity: Severity::Error,
            });
        }
        None => {}
    }
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

/// Validate node entries (properties and arguments) against schema.
fn validate_node_entries(
    schema: &KdlSchema,
    node: &KdlNode,
    schema_def: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let Some(def_children) = schema_def.children() else {
        return;
    };

    // Collect property definitions
    let prop_defs: Vec<&KdlNode> = def_children
        .nodes()
        .iter()
        .filter(|n| n.name().value() == "prop")
        .collect();

    // Collect value definitions
    let value_defs: Vec<&KdlNode> = def_children
        .nodes()
        .iter()
        .filter(|n| n.name().value() == "value")
        .collect();

    // Check if other props are allowed
    let other_props_allowed = def_children
        .get("other-props-allowed")
        .and_then(|n| n.get(0))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Validate prop-names if specified
    if let Some(prop_names) = def_children.get("prop-names") {
        let validations = build_validations(schema, prop_names);
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

    // Track which properties were found
    let mut found_props: HashMap<&str, usize> = HashMap::new();
    let mut arg_idx: usize = 0;

    for entry in node.entries() {
        if let Some(name) = entry.name() {
            // This is a property
            let prop_name = name.value();
            *found_props.entry(prop_name).or_insert(0) += 1;

            // Find matching prop definition (by name or wildcard)
            let prop_def = prop_defs
                .iter()
                .find(|def| {
                    def.get(0)
                        .and_then(|v| v.as_string())
                        .map(|s| s == prop_name)
                        .unwrap_or(false)
                })
                .or_else(|| {
                    // Fall back to wildcard prop def (no key argument)
                    prop_defs.iter().find(|def| def.get(0).is_none())
                });

            match prop_def {
                Some(def) => {
                    // Validate property value with ref resolution
                    let validations = build_validations(schema, def);
                    #[cfg(feature = "span")]
                    let span = entry.span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);
                    errors.extend(validations.check(entry.value(), span, input));

                    // Validate entry's type annotation if tag constraints exist
                    if let Some(ref tag_validations) = validations.tag {
                        if let Some(ty) = entry.ty() {
                            let tag_value = KdlValue::String(ty.value().to_string());
                            #[cfg(feature = "span")]
                            let tag_span = ty.span();
                            #[cfg(not(feature = "span"))]
                            let tag_span = SourceSpan::new(0.into(), 0);
                            errors.extend(tag_validations.check(&tag_value, tag_span, input));
                        }
                    }
                }
                None if !other_props_allowed => {
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
                            "Add this property to the schema or set 'other-props-allowed #true'"
                                .into(),
                        ),
                        severity: Severity::Error,
                    });
                }
                None => {}
            }
        } else {
            // This is a positional argument
            // Find matching value definition by index, or fall back to wildcard
            let value_def = value_defs.get(arg_idx).or_else(|| {
                // Wildcard: value def without specific position constraints
                value_defs.iter().find(|def| {
                    def.children()
                        .map(|c| c.get("min").is_none() && c.get("max").is_none())
                        .unwrap_or(true)
                })
            });

            if let Some(def) = value_def {
                let validations = build_validations(schema, def);
                #[cfg(feature = "span")]
                let span = entry.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);
                errors.extend(validations.check(entry.value(), span, input));

                // Validate entry's type annotation if tag constraints exist
                if let Some(ref tag_validations) = validations.tag {
                    if let Some(ty) = entry.ty() {
                        let tag_value = KdlValue::String(ty.value().to_string());
                        #[cfg(feature = "span")]
                        let tag_span = ty.span();
                        #[cfg(not(feature = "span"))]
                        let tag_span = SourceSpan::new(0.into(), 0);
                        errors.extend(tag_validations.check(&tag_value, tag_span, input));
                    }
                }
            }

            arg_idx += 1;
        }
    }

    // Check required properties
    for prop_def in &prop_defs {
        let prop_name = prop_def.get(0).and_then(|v| v.as_string());
        let is_required = prop_def
            .children()
            .and_then(|c| c.get("required"))
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_required {
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

    // Check value count constraints
    if let Some(value_def) = value_defs.first() {
        let min = value_def
            .children()
            .and_then(|c| c.get("min"))
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_integer())
            .map(|n| n as usize);

        let max = value_def
            .children()
            .and_then(|c| c.get("max"))
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_integer())
            .map(|n| n as usize);

        if let Some(min) = min {
            if arg_idx < min {
                #[cfg(feature = "span")]
                let span = node.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);

                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Too few arguments: expected at least {}, found {}",
                        min, arg_idx
                    )),
                    label: Some("not enough arguments".into()),
                    help: None,
                    severity: Severity::Error,
                });
            }
        }

        if let Some(max) = max {
            if arg_idx > max {
                #[cfg(feature = "span")]
                let span = node.span();
                #[cfg(not(feature = "span"))]
                let span = SourceSpan::new(0.into(), 0);

                errors.push(KdlDiagnostic {
                    input: input.clone(),
                    span,
                    message: Some(format!(
                        "Too many arguments: expected at most {}, found {}",
                        max, arg_idx
                    )),
                    label: Some("too many arguments".into()),
                    help: None,
                    severity: Severity::Error,
                });
            }
        }
    }
}

/// Resolve a children definition, following any ref attribute.
fn resolve_children_def<'a>(
    schema: &'a KdlSchema,
    children_def: &'a KdlNode,
) -> Option<&'a KdlNode> {
    // Check if this children def has a ref
    if let Some(ref_node) = resolve_ref(schema, children_def) {
        Some(ref_node)
    } else {
        Some(children_def)
    }
}

/// Validate node children recursively.
fn validate_node_children(
    schema: &KdlSchema,
    node: &KdlNode,
    schema_def: &KdlNode,
    input: &Arc<String>,
    errors: &mut Vec<KdlDiagnostic>,
) {
    let target_children = node.children();
    let schema_children_def = schema_def.children().and_then(|c| c.get("children"));

    match (target_children, schema_children_def) {
        (Some(target_doc), Some(children_def)) => {
            // Resolve ref if present
            let resolved_def = resolve_children_def(schema, children_def);
            let effective_def = resolved_def.unwrap_or(children_def);

            // Get child node definitions from the effective (possibly resolved) definition
            let child_node_defs: Vec<&KdlNode> = effective_def
                .children()
                .map(|c| {
                    c.nodes()
                        .iter()
                        .filter(|n| n.name().value() == "node")
                        .collect()
                })
                .unwrap_or_default();

            let other_nodes_allowed = effective_def
                .children()
                .and_then(|c| c.get("other-nodes-allowed"))
                .and_then(|n| n.get(0))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Validate node-names if specified
            if let Some(node_names) = effective_def.children().and_then(|c| c.get("node-names")) {
                let validations = build_validations(schema, node_names);
                for child_node in target_doc.nodes() {
                    let name_value = KdlValue::String(child_node.name().value().to_string());
                    #[cfg(feature = "span")]
                    let span = child_node.name().span();
                    #[cfg(not(feature = "span"))]
                    let span = SourceSpan::new(0.into(), 0);
                    errors.extend(validations.check(&name_value, span, input));
                }
            }

            // Validate each child node
            for child_node in target_doc.nodes() {
                validate_node(
                    schema,
                    child_node,
                    &child_node_defs,
                    other_nodes_allowed,
                    input,
                    errors,
                );
            }

            // Check child node counts
            check_node_counts(target_doc, &child_node_defs, input, errors);
        }
        (Some(_), None) => {
            // Target has children but schema doesn't define any
            // This could be an error or allowed depending on strictness
        }
        (None, Some(_)) => {
            // Schema expects children but target has none
            // Check if any children are required
        }
        (None, None) => {}
    }
}

/// Check min/max constraints on node counts.
fn check_node_counts(
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

    // Check each node definition's min/max constraints
    for def in node_defs {
        let node_name = def.get(0).and_then(|v| v.as_string());

        let min = def
            .children()
            .and_then(|c| c.get("min"))
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_integer())
            .map(|n| n as usize);

        let max = def
            .children()
            .and_then(|c| c.get("max"))
            .and_then(|n| n.get(0))
            .and_then(|v| v.as_integer())
            .map(|n| n as usize);

        if let Some(name) = node_name {
            let count = counts.get(name).copied().unwrap_or(0);

            if let Some(min) = min {
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
                        label: Some("missing required nodes".into()),
                        help: None,
                        severity: Severity::Error,
                    });
                }
            }

            if let Some(max) = max {
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
