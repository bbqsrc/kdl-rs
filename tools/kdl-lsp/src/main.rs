mod schema_manager;

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use kdl::{KdlDiagnostic, KdlDocument, KdlError};
use miette::Diagnostic as _;
use ropey::Rope;
use schema_manager::{SchemaManager, SchemaSource};
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

#[derive(Debug)]
struct Backend {
    client: Client,
    document_map: DashMap<String, Rope>,
    schema_manager: Arc<SchemaManager>,
    workspace_roots: Arc<RwLock<Vec<PathBuf>>>,
}

impl Backend {
    fn on_change(&self, uri: Url, text: &str) {
        let rope = ropey::Rope::from_str(text);
        self.document_map.insert(uri.to_string(), rope);
    }

    /// Validate document against its associated schema(s).
    /// Per spec: ALL schemas must validate for the document to pass.
    fn validate_with_schema(
        &self,
        doc: &KdlDocument,
        rope: &Rope,
        schema_source: &SchemaSource,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        match &schema_source {
            SchemaSource::Directives(directives) => {
                // Validate against ALL schemas - ALL must pass
                for directive in directives {
                    match self.schema_manager.get_or_load_schema(&directive.path) {
                        Ok(schema) => {
                            // Use core's validate_with_severity for warn_only handling
                            let kdl_diags = schema.validate_with_severity(doc, directive.warn_only);
                            diagnostics.extend(self.convert_kdl_diagnostics(&kdl_diags, rope));
                        }
                        Err(err) => {
                            // Show error at the @ksl:schema directive location
                            let severity = if directive.warn_only {
                                DiagnosticSeverity::WARNING
                            } else {
                                DiagnosticSeverity::ERROR
                            };
                            diagnostics.push(Diagnostic::new(
                                Range::new(
                                    char_to_position(directive.span.offset(), rope),
                                    char_to_position(
                                        directive.span.offset() + directive.span.len(),
                                        rope,
                                    ),
                                ),
                                Some(severity),
                                Some(NumberOrString::String("schema-load-error".into())),
                                Some("kdl-schema-v2".into()),
                                format!("Failed to load schema: {}", err),
                                None,
                                None,
                            ));
                        }
                    }
                }
            }
            SchemaSource::ConfigFile { path, .. } => {
                match self.schema_manager.get_or_load_schema(path) {
                    Ok(schema) => {
                        let kdl_diags = schema.validate(doc);
                        diagnostics.extend(self.convert_kdl_diagnostics(&kdl_diags, rope));
                    }
                    Err(err) => {
                        // Config-based schema errors show at document start
                        tracing::warn!("Failed to load schema from config: {}", err);
                        diagnostics.push(Diagnostic::new(
                            Range::new(Position::new(0, 0), Position::new(0, 1)),
                            Some(DiagnosticSeverity::WARNING),
                            Some(NumberOrString::String("schema-load-error".into())),
                            Some("kdl-schema-v2".into()),
                            format!("Failed to load schema '{}': {}", path.display(), err),
                            None,
                            None,
                        ));
                    }
                }
            }
            SchemaSource::None => {}
        }

        diagnostics
    }

    /// Convert KdlDiagnostic to LSP Diagnostic.
    fn convert_kdl_diagnostics(&self, kdl_diags: &[KdlDiagnostic], rope: &Rope) -> Vec<Diagnostic> {
        kdl_diags
            .iter()
            .map(|diag| {
                Diagnostic::new(
                    Range::new(
                        char_to_position(diag.span.offset(), rope),
                        char_to_position(diag.span.offset() + diag.span.len(), rope),
                    ),
                    Some(to_lsp_sev(diag.severity)),
                    Some(NumberOrString::String("kdl-schema".into())),
                    Some("kdl-schema-v2".into()),
                    diag.message
                        .clone()
                        .unwrap_or_else(|| "Schema validation error".into()),
                    None,
                    None,
                )
            })
            .collect()
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Store workspace roots for relative path resolution
        {
            let mut roots = self.workspace_roots.write().await;
            if let Some(folders) = params.workspace_folders {
                *roots = folders
                    .iter()
                    .filter_map(|f| {
                        url::Url::parse(&f.uri.to_string())
                            .ok()
                            .and_then(|u| u.to_file_path().ok())
                    })
                    .collect();
            } else if let Some(root_uri) = params.root_uri {
                if let Some(path) = url::Url::parse(&root_uri.to_string())
                    .ok()
                    .and_then(|u| u.to_file_path().ok())
                    .map(|p| p.canonicalize().unwrap_or(p))
                {
                    roots.push(path);
                }
            }

            // Load workspace config files
            for root in roots.iter() {
                let config_path = root.join(".kdl-config.kdl");
                if config_path.exists() {
                    self.schema_manager.load_config(&config_path, root);
                }
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                diagnostic_provider: Some(DiagnosticServerCapabilities::RegistrationOptions(
                    DiagnosticRegistrationOptions {
                        text_document_registration_options: TextDocumentRegistrationOptions {
                            document_selector: Some(vec![DocumentFilter {
                                language: Some("kdl".into()),
                                scheme: Some("file".into()),
                                pattern: None,
                            }]),
                        },
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(
                MessageType::INFO,
                "KDL LSP server initialized with schema-v2 support",
            )
            .await;

        // Register for file change notifications on .kdl files
        let registration = Registration {
            id: "kdl-file-watcher".into(),
            method: "workspace/didChangeWatchedFiles".into(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![FileSystemWatcher {
                        glob_pattern: GlobPattern::String("**/*.kdl".into()),
                        kind: Some(WatchKind::all()),
                    }],
                })
                .unwrap(),
            ),
        };

        if let Err(e) = self.client.register_capability(vec![registration]).await {
            tracing::warn!("Failed to register file watcher: {}", e);
        }
    }

    async fn shutdown(&self) -> Result<()> {
        self.client
            .log_message(MessageType::INFO, "server shutting down")
            .await;
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.on_change(params.text_document.uri, &params.text_document.text);
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        self.on_change(params.text_document.uri, &params.content_changes[0].text);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text.as_ref() {
            self.on_change(params.text_document.uri, text);
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        self.document_map.remove(&uri);
        self.schema_manager.remove_document(&uri);
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let roots = self.workspace_roots.read().await;

        for change in params.changes {
            let path = match url::Url::parse(&change.uri.to_string())
                .ok()
                .and_then(|u| u.to_file_path().ok())
            {
                Some(p) => p,
                None => continue,
            };

            // Check if this is a schema file we're tracking
            if self.schema_manager.schemas.contains_key(&path) {
                tracing::info!("Schema file changed: {}", path.display());
                self.schema_manager.invalidate_schema(&path);

                // Refresh diagnostics for affected documents
                if let Err(e) = self.client.workspace_diagnostic_refresh().await {
                    tracing::warn!("Failed to refresh diagnostics: {}", e);
                }
            }

            // Check if this is a config file
            if path
                .file_name()
                .map(|n| n == ".kdl-config.kdl")
                .unwrap_or(false)
            {
                tracing::info!("Config file changed: {}", path.display());
                if let Some(root) = roots.iter().find(|r| path.starts_with(r)) {
                    self.schema_manager.reload_config(&path, root);

                    // Refresh diagnostics for all documents
                    if let Err(e) = self.client.workspace_diagnostic_refresh().await {
                        tracing::warn!("Failed to refresh diagnostics: {}", e);
                    }
                }
            }
        }
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        tracing::debug!("diagnostic req");
        let uri = params.text_document.uri.to_string();

        if let Some(rope) = self.document_map.get(&uri) {
            let text = rope.to_string();
            let res: std::result::Result<KdlDocument, KdlError> = text.parse();

            match res {
                Ok(parsed_doc) => {
                    // Resolve schema from the already-parsed document (no double-parsing!)
                    let roots = self.workspace_roots.read().await;
                    let schema_source = self.schema_manager.resolve_schema_for_parsed_document(
                        &uri,
                        &parsed_doc,
                        &roots,
                    );

                    // Update cached association
                    self.schema_manager
                        .update_document_schema(&uri, schema_source.clone());

                    // Run schema validation
                    let schema_diags =
                        self.validate_with_schema(&parsed_doc, &rope, &schema_source);

                    return Ok(DocumentDiagnosticReportResult::Report(
                        DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                            related_documents: None,
                            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                                result_id: None,
                                items: schema_diags,
                            },
                        }),
                    ));
                }
                Err(kdl_err) => {
                    // Parse errors take precedence over schema validation
                    let diags = kdl_err
                        .diagnostics
                        .into_iter()
                        .map(|diag| {
                            Diagnostic::new(
                                Range::new(
                                    char_to_position(diag.span.offset(), &rope),
                                    char_to_position(diag.span.offset() + diag.span.len(), &rope),
                                ),
                                diag.severity().map(to_lsp_sev),
                                diag.code().map(|c| NumberOrString::String(c.to_string())),
                                None,
                                diag.to_string(),
                                None,
                                None,
                            )
                        })
                        .collect();

                    return Ok(DocumentDiagnosticReportResult::Report(
                        DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                            related_documents: None,
                            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                                result_id: None,
                                items: diags,
                            },
                        }),
                    ));
                }
            }
        }

        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport::default()),
        ))
    }
}

fn char_to_position(char_idx: usize, rope: &Rope) -> Position {
    let line_idx = rope.char_to_line(char_idx.min(rope.len_chars().saturating_sub(1)));
    let line_char_idx = rope.line_to_char(line_idx);
    let column_idx = char_idx.saturating_sub(line_char_idx);
    Position::new(line_idx as u32, column_idx as u32)
}

fn to_lsp_sev(sev: miette::Severity) -> DiagnosticSeverity {
    match sev {
        miette::Severity::Advice => DiagnosticSeverity::HINT,
        miette::Severity::Warning => DiagnosticSeverity::WARNING,
        miette::Severity::Error => DiagnosticSeverity::ERROR,
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .map_writer(move |_| std::io::stderr)
                .with_ansi(false),
        )
        .with(EnvFilter::from_default_env())
        .init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        document_map: DashMap::new(),
        schema_manager: Arc::new(SchemaManager::new()),
        workspace_roots: Arc::new(RwLock::new(Vec::new())),
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}
