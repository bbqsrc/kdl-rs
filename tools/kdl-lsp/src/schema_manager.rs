//! Schema management for KDL LSP.
//!
//! Handles schema caching, resolution, and document-to-schema associations.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use dashmap::DashMap;
use globset::{Glob, GlobMatcher};
use kdl::schema_v2::KdlSchemaV2;
use kdl::KdlDocument;
use miette::SourceSpan;

/// Cached schema with its source path and parsed state.
#[derive(Debug, Clone)]
pub struct CachedSchema {
    /// The parsed schema (None if parsing failed)
    pub schema: Option<Arc<KdlSchemaV2>>,
    /// Error message if schema failed to parse
    pub error: Option<String>,
    /// Timestamp of last modification (for cache invalidation)
    pub modified: SystemTime,
}

/// Represents how a schema was resolved for a document.
#[derive(Debug, Clone)]
pub enum SchemaSource {
    /// From @ksl:schema directive in the document
    Directive {
        /// Resolved absolute path to schema file
        path: PathBuf,
        /// Span of the directive in the document (for error reporting)
        directive_span: SourceSpan,
    },
    /// From .kdl-config.kdl glob mapping
    ConfigFile {
        /// Resolved absolute path to schema file
        path: PathBuf,
    },
    /// No schema found
    None,
}

/// A single glob -> schema mapping from config.
#[derive(Debug, Clone)]
pub struct GlobMapping {
    pub matcher: GlobMatcher,
    pub schema_path: PathBuf,
}

/// Manages schema loading, caching, and document associations.
#[derive(Debug, Default)]
pub struct SchemaManager {
    /// Cache of loaded schemas by absolute path
    pub schemas: DashMap<PathBuf, CachedSchema>,

    /// Mapping from document URI to its schema source
    pub document_schemas: DashMap<String, SchemaSource>,

    /// Reverse mapping: schema path -> set of document URIs using it
    pub schema_to_documents: DashMap<PathBuf, HashSet<String>>,

    /// Parsed glob patterns from workspace configs, keyed by config file path
    pub glob_mappings: DashMap<PathBuf, Vec<GlobMapping>>,
}

impl SchemaManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve which schema applies to a document.
    pub fn resolve_schema_for_document(
        &self,
        document_uri: &str,
        document_text: &str,
        workspace_roots: &[PathBuf],
    ) -> SchemaSource {
        // 1. First, check for @ksl:schema directive in the document
        if let Some(directive_source) =
            self.extract_directive_schema(document_uri, document_text, workspace_roots)
        {
            return directive_source;
        }

        // 2. Fall back to workspace config file mappings
        if let Some(config_source) = self.match_config_glob(document_uri, workspace_roots) {
            return config_source;
        }

        SchemaSource::None
    }

    /// Extract @ksl:schema directive from document.
    fn extract_directive_schema(
        &self,
        uri: &str,
        text: &str,
        workspace_roots: &[PathBuf],
    ) -> Option<SchemaSource> {
        // Parse document to find @ksl:schema node
        let doc: KdlDocument = text.parse().ok()?;
        let directive = doc.get("@ksl:schema")?;
        let schema_path_str = directive.get(0)?.as_string()?;

        // Get directive span for error reporting
        let directive_span = directive.span();

        // Resolve path (relative to document or absolute)
        let document_path = url::Url::parse(uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())?;
        let document_dir = document_path.parent()?;

        let resolved_path =
            self.resolve_schema_path(schema_path_str, document_dir, workspace_roots);

        Some(SchemaSource::Directive {
            path: resolved_path,
            directive_span,
        })
    }

    /// Resolve a schema path that may be relative or absolute.
    fn resolve_schema_path(
        &self,
        schema_path: &str,
        document_dir: &Path,
        _workspace_roots: &[PathBuf],
    ) -> PathBuf {
        let path = Path::new(schema_path);

        if path.is_absolute() {
            path.to_path_buf()
        } else {
            // Relative to the document's directory
            let resolved = document_dir.join(path);
            resolved.canonicalize().unwrap_or(resolved)
        }
    }

    /// Match document against config file glob patterns.
    fn match_config_glob(
        &self,
        document_uri: &str,
        workspace_roots: &[PathBuf],
    ) -> Option<SchemaSource> {
        let document_path = url::Url::parse(document_uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())?;

        // Check each workspace root's config
        for root in workspace_roots {
            let config_path = root.join(".kdl-config.kdl");
            if let Some(mappings) = self.glob_mappings.get(&config_path) {
                // Try each mapping in order (first match wins)
                for mapping in mappings.iter() {
                    // Make document path relative to workspace root for matching
                    if let Ok(relative) = document_path.strip_prefix(root) {
                        if mapping.matcher.is_match(relative) {
                            return Some(SchemaSource::ConfigFile {
                                path: mapping.schema_path.clone(),
                            });
                        }
                    }
                    // Also try matching the full path
                    if mapping.matcher.is_match(&document_path) {
                        return Some(SchemaSource::ConfigFile {
                            path: mapping.schema_path.clone(),
                        });
                    }
                }
            }
        }

        None
    }

    /// Load a schema from the cache or from disk.
    pub fn get_or_load_schema(&self, path: &Path) -> Result<Arc<KdlSchemaV2>, String> {
        // Check cache first
        if let Some(cached) = self.schemas.get(path) {
            // Check if file has been modified
            if let Ok(metadata) = std::fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    if modified <= cached.modified {
                        // Cache is still valid
                        return cached
                            .schema
                            .clone()
                            .ok_or_else(|| cached.error.clone().unwrap_or_default());
                    }
                }
            }
        }

        // Load from disk
        self.load_schema(path)
    }

    /// Load a schema from disk and cache it.
    fn load_schema(&self, path: &Path) -> Result<Arc<KdlSchemaV2>, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read schema file '{}': {}", path.display(), e))?;

        let modified = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| SystemTime::now());

        match KdlSchemaV2::parse(&content) {
            Ok(schema) => {
                let schema = Arc::new(schema);
                self.schemas.insert(
                    path.to_path_buf(),
                    CachedSchema {
                        schema: Some(schema.clone()),
                        error: None,
                        modified,
                    },
                );
                Ok(schema)
            }
            Err(e) => {
                let error_msg = format!("Failed to parse schema: {}", e);
                self.schemas.insert(
                    path.to_path_buf(),
                    CachedSchema {
                        schema: None,
                        error: Some(error_msg.clone()),
                        modified,
                    },
                );
                Err(error_msg)
            }
        }
    }

    /// Update the schema association for a document.
    pub fn update_document_schema(&self, document_uri: &str, source: SchemaSource) {
        // Remove from old schema's document set
        if let Some(old_source) = self.document_schemas.get(document_uri) {
            if let Some(old_path) = schema_source_path(&old_source) {
                if let Some(mut docs) = self.schema_to_documents.get_mut(&old_path) {
                    docs.remove(document_uri);
                }
            }
        }

        // Add to new schema's document set
        if let Some(new_path) = schema_source_path(&source) {
            self.schema_to_documents
                .entry(new_path)
                .or_default()
                .insert(document_uri.to_string());
        }

        self.document_schemas
            .insert(document_uri.to_string(), source);
    }

    /// Invalidate a cached schema (e.g., when the file changes).
    pub fn invalidate_schema(&self, path: &Path) {
        self.schemas.remove(path);
    }

    /// Remove a document from tracking (when it's closed).
    pub fn remove_document(&self, document_uri: &str) {
        if let Some((_, source)) = self.document_schemas.remove(document_uri) {
            if let Some(path) = schema_source_path(&source) {
                if let Some(mut docs) = self.schema_to_documents.get_mut(&path) {
                    docs.remove(document_uri);
                }
            }
        }
    }

    /// Load config file glob mappings.
    pub fn load_config(&self, config_path: &Path, workspace_root: &Path) {
        let content = match std::fs::read_to_string(config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "Failed to read config file '{}': {}",
                    config_path.display(),
                    e
                );
                return;
            }
        };

        let doc: KdlDocument = match content.parse() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    "Failed to parse config file '{}': {}",
                    config_path.display(),
                    e
                );
                return;
            }
        };

        let mut mappings = Vec::new();

        // Look for schemas block
        if let Some(schemas_node) = doc.get("schemas") {
            if let Some(children) = schemas_node.children() {
                for node in children.nodes() {
                    if node.name().value() == "mapping" {
                        // mapping "glob" schema="path"
                        if let (Some(pattern), Some(schema_path)) = (
                            node.get(0).and_then(|v| v.as_string()),
                            node.entry("schema").and_then(|e| e.value().as_string()),
                        ) {
                            match Glob::new(pattern) {
                                Ok(glob) => {
                                    let schema_abs = if Path::new(schema_path).is_absolute() {
                                        PathBuf::from(schema_path)
                                    } else {
                                        workspace_root.join(schema_path)
                                    };

                                    mappings.push(GlobMapping {
                                        matcher: glob.compile_matcher(),
                                        schema_path: schema_abs,
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Invalid glob pattern '{}' in config: {}",
                                        pattern,
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        self.glob_mappings
            .insert(config_path.to_path_buf(), mappings);
    }

    /// Reload config file (when it changes).
    pub fn reload_config(&self, config_path: &Path, workspace_root: &Path) {
        self.glob_mappings.remove(config_path);
        self.load_config(config_path, workspace_root);
    }
}

/// Extract the schema path from a SchemaSource.
fn schema_source_path(source: &SchemaSource) -> Option<PathBuf> {
    match source {
        SchemaSource::Directive { path, .. } => Some(path.clone()),
        SchemaSource::ConfigFile { path, .. } => Some(path.clone()),
        SchemaSource::None => None,
    }
}
