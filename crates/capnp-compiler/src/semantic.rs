//! Deterministic semantic indexing over the M22 lossless syntax tree.
//!
//! This module owns M23 concerns only: module selection, imports, lexical
//! scopes, aliases, IDs and ordinals, expression references, constants,
//! annotations, generic parameters, and cycle diagnostics. Struct layout and
//! wire offsets remain M24.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use capnp_schema::AnnotationTargets;

use crate::{
    Diagnostic, ParseLimits, SourceRange, Statement, StatementBody, SyntaxTree, Token, TokenKind,
    TokenSequence, parse_schema,
};

#[derive(Clone, Debug)]
pub struct ModuleSources {
    explicit: BTreeMap<String, Arc<str>>,
    standard: BTreeMap<String, Arc<str>>,
}

impl Default for ModuleSources {
    fn default() -> Self {
        let mut output = Self {
            explicit: BTreeMap::new(),
            standard: BTreeMap::new(),
        };
        output.insert_standard(
            "/capnp/stream.capnp",
            include_str!(concat!(
                "../../../conformance/upstream/capnproto/",
                "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/stream.capnp"
            )),
        );
        output.insert_standard(
            "/capnp/c++.capnp",
            include_str!(concat!(
                "../../../conformance/upstream/capnproto/",
                "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/c++.capnp"
            )),
        );
        output.insert_standard(
            "/capnp/compat/json.capnp",
            include_str!(concat!(
                "../../../conformance/upstream/capnproto/",
                "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/json.capnp"
            )),
        );
        output
    }
}

impl ModuleSources {
    pub fn insert_explicit(&mut self, path: impl AsRef<str>, source: impl Into<Arc<str>>) {
        self.explicit
            .insert(normalize_rooted(path.as_ref()), source.into());
    }

    pub fn insert_standard(&mut self, path: impl AsRef<str>, source: impl Into<Arc<str>>) {
        self.standard
            .insert(normalize_rooted(path.as_ref()), source.into());
    }

    pub fn resolve(&self, entry: &str, limits: ResolveLimits) -> ResolvedProgram {
        Resolver::new(self, limits).resolve(entry)
    }

    fn get(&self, path: &str) -> Option<Arc<str>> {
        self.explicit
            .get(path)
            .or_else(|| self.standard.get(path))
            .cloned()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolveLimits {
    pub parse: ParseLimits,
    pub max_modules: usize,
    pub max_declarations: usize,
    pub max_expression_nodes: usize,
}

impl Default for ResolveLimits {
    fn default() -> Self {
        Self {
            parse: ParseLimits::default(),
            max_modules: 4096,
            max_declarations: 1_000_000,
            max_expression_nodes: 4_000_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedProgram {
    pub entry: String,
    pub modules: Vec<ResolvedModule>,
    pub diagnostics: Vec<SemanticDiagnostic>,
}

impl ResolvedProgram {
    pub fn is_valid(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn module(&self, path: &str) -> Option<&ResolvedModule> {
        let path = normalize_rooted(path);
        self.modules.iter().find(|module| module.path == path)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedModule {
    pub path: String,
    pub file_id: Option<u64>,
    pub imports: Vec<ImportBinding>,
    pub annotations: Vec<AnnotationUse>,
    pub declarations: Vec<ResolvedDeclaration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportBinding {
    pub name: String,
    pub parent: Option<String>,
    pub requested_path: String,
    pub resolved_path: String,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedDeclaration {
    pub name: String,
    pub qualified_name: String,
    pub parent: Option<String>,
    pub kind: DeclarationKind,
    pub is_unnamed_union: bool,
    pub id: Option<DeclarationId>,
    pub id_range: Option<SourceRange>,
    pub generic_parameters: Vec<String>,
    pub annotation_targets: Option<AnnotationTargets>,
    pub expression: Option<Expression>,
    pub value: Option<Expression>,
    pub annotations: Vec<AnnotationUse>,
    pub doc_comment: Option<String>,
    pub range: SourceRange,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeclarationId {
    Uid(u64),
    Ordinal(u16),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeclarationKind {
    Alias,
    Const,
    Enum,
    Enumerant,
    Struct,
    Field,
    Union,
    Group,
    Interface,
    Method,
    Annotation,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationUse {
    pub name: Expression,
    pub value: Option<Expression>,
    pub range: SourceRange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    Name {
        absolute: bool,
        path: Vec<String>,
        target: NameTarget,
        range: SourceRange,
    },
    Import {
        path: String,
        member_path: Vec<String>,
        target: NameTarget,
        range: SourceRange,
    },
    Embed {
        path: String,
        range: SourceRange,
    },
    Integer {
        negative: bool,
        magnitude: u64,
        range: SourceRange,
    },
    Float {
        value: f64,
        range: SourceRange,
    },
    String {
        value: String,
        range: SourceRange,
    },
    Binary {
        value: Vec<u8>,
        range: SourceRange,
    },
    List {
        values: Vec<Expression>,
        range: SourceRange,
    },
    Tuple {
        values: Vec<(Option<String>, Expression)>,
        range: SourceRange,
    },
    Apply {
        function: Box<Expression>,
        arguments: Vec<(Option<String>, Expression)>,
        range: SourceRange,
    },
    Member {
        base: Box<Expression>,
        name: String,
        target: NameTarget,
        range: SourceRange,
    },
    Unknown {
        range: SourceRange,
    },
}

impl Expression {
    pub fn range(&self) -> SourceRange {
        match self {
            Self::Name { range, .. }
            | Self::Import { range, .. }
            | Self::Embed { range, .. }
            | Self::Integer { range, .. }
            | Self::Float { range, .. }
            | Self::String { range, .. }
            | Self::Binary { range, .. }
            | Self::List { range, .. }
            | Self::Tuple { range, .. }
            | Self::Apply { range, .. }
            | Self::Member { range, .. }
            | Self::Unknown { range } => *range,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum NameTarget {
    #[default]
    Pending,
    Builtin,
    GenericParameter {
        declaration: String,
        name: String,
    },
    Declaration {
        module: String,
        qualified_name: String,
    },
    Module {
        path: String,
    },
    Unresolved,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticDiagnostic {
    pub module: String,
    pub range: SourceRange,
    pub message: String,
}

struct Resolver<'a> {
    sources: &'a ModuleSources,
    limits: ResolveLimits,
    diagnostics: Vec<SemanticDiagnostic>,
    declaration_count: usize,
    expression_count: usize,
}

impl<'a> Resolver<'a> {
    fn new(sources: &'a ModuleSources, limits: ResolveLimits) -> Self {
        Self {
            sources,
            limits,
            diagnostics: Vec::new(),
            declaration_count: 0,
            expression_count: 0,
        }
    }

    fn resolve(mut self, entry: &str) -> ResolvedProgram {
        let entry = normalize_rooted(entry);
        let mut pending = BTreeSet::from([entry.clone()]);
        let mut modules = BTreeMap::new();
        while let Some(path) = pending.pop_first() {
            if modules.contains_key(&path) {
                continue;
            }
            if modules.len() >= self.limits.max_modules {
                self.error(&path, SourceRange::default(), "module limit exceeded");
                break;
            }
            let Some(source) = self.sources.get(&path) else {
                self.error(
                    &path,
                    SourceRange::default(),
                    "module source was not provided",
                );
                continue;
            };
            let syntax = parse_schema(source, self.limits.parse);
            self.copy_syntax_diagnostics(&path, &syntax);
            let module = self.parse_module(&path, &syntax);
            let mut dependencies = BTreeSet::new();
            collect_module_dependencies(&module, &mut dependencies);
            for dependency in dependencies {
                pending.insert(resolve_import_path(&path, &dependency));
            }
            modules.insert(path, module);
        }

        let mut modules = modules.into_values().collect::<Vec<_>>();
        let index = SymbolIndex::build(&modules);
        for module in &mut modules {
            self.validate_module(module);
            resolve_module_names(module, &index, &mut self.diagnostics);
        }
        self.validate_global_ids(&modules);
        self.detect_value_cycles(&modules);
        self.diagnostics.sort_by(|left, right| {
            (
                &left.module,
                left.range.start,
                left.range.end,
                &left.message,
            )
                .cmp(&(
                    &right.module,
                    right.range.start,
                    right.range.end,
                    &right.message,
                ))
        });
        self.diagnostics.dedup();
        ResolvedProgram {
            entry,
            modules,
            diagnostics: self.diagnostics,
        }
    }

    fn copy_syntax_diagnostics(&mut self, module: &str, syntax: &SyntaxTree) {
        for Diagnostic { range, message } in &syntax.diagnostics {
            self.error(module, *range, message);
        }
        if !syntax.is_valid() && syntax.diagnostics.is_empty() {
            self.error(module, SourceRange::default(), "syntax parsing failed");
        }
    }

    fn parse_module(&mut self, path: &str, syntax: &SyntaxTree) -> ResolvedModule {
        let mut module = ResolvedModule {
            path: path.to_owned(),
            file_id: None,
            imports: Vec::new(),
            annotations: Vec::new(),
            declarations: Vec::new(),
        };
        for statement in &syntax.statements {
            if let Some(id) = naked_id(&statement.tokens) {
                if module.file_id.replace(id).is_some() {
                    self.error(path, statement.range, "duplicate file ID");
                }
                if id < (1u64 << 63) {
                    self.error(path, statement.range, "file ID must have its high bit set");
                }
                continue;
            }
            if is_naked_annotation(&statement.tokens) {
                module
                    .annotations
                    .extend(parse_annotations(&statement.tokens));
                continue;
            }
            self.parse_statement(path, statement, None, None, &mut module);
        }
        if module.file_id.is_none() {
            self.error(path, SourceRange::default(), "file is missing its ID");
        }
        module
    }

    fn parse_statement(
        &mut self,
        module_path: &str,
        statement: &Statement,
        parent: Option<&str>,
        parent_kind: Option<DeclarationKind>,
        module: &mut ResolvedModule,
    ) {
        if self.declaration_count >= self.limits.max_declarations {
            self.error(module_path, statement.range, "declaration limit exceeded");
            return;
        }
        let Some(declaration) = parse_declaration(statement, parent, parent_kind) else {
            if !is_naked_annotation(&statement.tokens) {
                self.error(module_path, statement.range, "unrecognized declaration");
            }
            return;
        };
        self.declaration_count += 1;
        self.expression_count += expression_nodes(declaration.expression.as_ref())
            + expression_nodes(declaration.value.as_ref());
        if self.expression_count > self.limits.max_expression_nodes {
            self.error(
                module_path,
                statement.range,
                "expression node limit exceeded",
            );
        }

        if declaration.kind == DeclarationKind::Alias {
            if let Some(Expression::Import {
                path: requested_path,
                range,
                ..
            }) = &declaration.expression
            {
                module.imports.push(ImportBinding {
                    name: declaration.name.clone(),
                    parent: declaration.parent.clone(),
                    requested_path: requested_path.clone(),
                    resolved_path: resolve_import_path(module_path, requested_path),
                    range: *range,
                });
            }
        }
        let qualified = declaration.qualified_name.clone();
        let kind = declaration.kind;
        let body = match &statement.body {
            StatementBody::Block(body) => Some(body.as_slice()),
            StatementBody::Line | StatementBody::MissingTerminator => None,
        };
        module.declarations.push(declaration);
        if let Some(body) = body {
            for child in body {
                self.parse_statement(module_path, child, Some(&qualified), Some(kind), module);
            }
        }
    }

    fn validate_module(&mut self, module: &ResolvedModule) {
        let mut names = BTreeSet::new();
        let mut ordinals: BTreeMap<Option<&str>, BTreeSet<u16>> = BTreeMap::new();
        for declaration in &module.declarations {
            let key = (declaration.parent.as_deref(), declaration.name.as_str());
            if !names.insert(key) {
                self.error(
                    &module.path,
                    declaration.range,
                    "duplicate declaration name",
                );
            }
            match declaration.id {
                Some(DeclarationId::Uid(id)) if id < (1u64 << 63) => self.error(
                    &module.path,
                    declaration.range,
                    "declaration ID must have its high bit set",
                ),
                Some(DeclarationId::Ordinal(ordinal)) => {
                    if !ordinals
                        .entry(declaration.parent.as_deref())
                        .or_default()
                        .insert(ordinal)
                    {
                        self.error(&module.path, declaration.range, "duplicate ordinal");
                    }
                }
                Some(DeclarationId::Uid(_)) | None => {}
            }
        }
        let mut imports = BTreeSet::new();
        for import in &module.imports {
            if !imports.insert((import.parent.as_deref(), import.name.as_str())) {
                self.error(&module.path, import.range, "duplicate import alias");
            }
        }
    }

    fn validate_global_ids(&mut self, modules: &[ResolvedModule]) {
        let mut ids = BTreeMap::new();
        for module in modules {
            if let Some(id) = module.file_id {
                if ids.insert(id, module.path.clone()).is_some() {
                    self.error(&module.path, SourceRange::default(), "duplicate global ID");
                }
            }
            for declaration in &module.declarations {
                if let Some(DeclarationId::Uid(id)) = declaration.id {
                    if ids.insert(id, module.path.clone()).is_some() {
                        self.error(&module.path, declaration.range, "duplicate global ID");
                    }
                }
            }
        }
    }

    fn detect_value_cycles(&mut self, modules: &[ResolvedModule]) {
        let mut graph: BTreeMap<(String, String), BTreeSet<(String, String)>> = BTreeMap::new();
        let mut ranges = BTreeMap::new();
        for module in modules {
            for declaration in &module.declarations {
                if !matches!(
                    declaration.kind,
                    DeclarationKind::Alias | DeclarationKind::Const
                ) {
                    continue;
                }
                let key = (module.path.clone(), declaration.qualified_name.clone());
                ranges.insert(key.clone(), declaration.range);
                let mut edges = BTreeSet::new();
                collect_declaration_targets(declaration.expression.as_ref(), &mut edges);
                collect_declaration_targets(declaration.value.as_ref(), &mut edges);
                graph.insert(key, edges);
            }
        }
        for start in graph.keys() {
            let mut visiting = BTreeSet::new();
            let mut visited = BTreeSet::new();
            if reaches_cycle(start, start, &graph, &mut visiting, &mut visited) {
                self.error(
                    &start.0,
                    ranges.get(start).copied().unwrap_or_default(),
                    "constant or alias cycle",
                );
            }
        }
    }

    fn error(&mut self, module: &str, range: SourceRange, message: &str) {
        self.diagnostics.push(SemanticDiagnostic {
            module: module.to_owned(),
            range,
            message: message.to_owned(),
        });
    }
}

#[derive(Default)]
struct SymbolIndex {
    declarations: BTreeSet<(String, String)>,
    children: BTreeMap<(String, Option<String>, String), String>,
    kinds: BTreeMap<(String, String), DeclarationKind>,
    annotation_targets: BTreeMap<(String, String), AnnotationTargets>,
}

impl SymbolIndex {
    fn build(modules: &[ResolvedModule]) -> Self {
        let mut output = Self::default();
        for module in modules {
            for declaration in &module.declarations {
                output
                    .declarations
                    .insert((module.path.clone(), declaration.qualified_name.clone()));
                output.kinds.insert(
                    (module.path.clone(), declaration.qualified_name.clone()),
                    declaration.kind,
                );
                if let Some(targets) = declaration.annotation_targets {
                    output.annotation_targets.insert(
                        (module.path.clone(), declaration.qualified_name.clone()),
                        targets,
                    );
                }
                output.children.insert(
                    (
                        module.path.clone(),
                        declaration.parent.clone(),
                        declaration.name.clone(),
                    ),
                    declaration.qualified_name.clone(),
                );
            }
        }
        output
    }
}

fn resolve_module_names(
    module: &mut ResolvedModule,
    index: &SymbolIndex,
    diagnostics: &mut Vec<SemanticDiagnostic>,
) {
    let imports = module
        .imports
        .iter()
        .map(|import| {
            (
                (import.parent.clone(), import.name.clone()),
                import.resolved_path.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let generic_scopes = module
        .declarations
        .iter()
        .map(|declaration| {
            (
                declaration.qualified_name.clone(),
                declaration.generic_parameters.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let module_path = module.path.clone();
    for annotation in &mut module.annotations {
        resolve_expression(
            Some(&mut annotation.name),
            &module_path,
            None,
            &imports,
            &generic_scopes,
            index,
            diagnostics,
        );
        validate_annotation_use(annotation, None, &module_path, index, diagnostics);
        resolve_expression(
            annotation.value.as_mut(),
            &module_path,
            None,
            &imports,
            &generic_scopes,
            index,
            diagnostics,
        );
    }
    for declaration in &mut module.declarations {
        let context = Some(declaration.qualified_name.as_str());
        resolve_expression(
            declaration.expression.as_mut(),
            &module_path,
            context,
            &imports,
            &generic_scopes,
            index,
            diagnostics,
        );
        resolve_value_from_expected_type(
            declaration.expression.as_ref(),
            declaration.value.as_mut(),
            index,
        );
        resolve_expression(
            declaration.value.as_mut(),
            &module_path,
            context,
            &imports,
            &generic_scopes,
            index,
            diagnostics,
        );
        for annotation in &mut declaration.annotations {
            resolve_expression(
                Some(&mut annotation.name),
                &module_path,
                context,
                &imports,
                &generic_scopes,
                index,
                diagnostics,
            );
            resolve_expression(
                annotation.value.as_mut(),
                &module_path,
                context,
                &imports,
                &generic_scopes,
                index,
                diagnostics,
            );
            validate_annotation_use(
                annotation,
                Some(declaration.kind),
                &module_path,
                index,
                diagnostics,
            );
        }
    }
}

fn validate_annotation_use(
    annotation: &AnnotationUse,
    applied_to: Option<DeclarationKind>,
    module: &str,
    index: &SymbolIndex,
    diagnostics: &mut Vec<SemanticDiagnostic>,
) {
    let Some(NameTarget::Declaration {
        module: target_module,
        qualified_name,
    }) = expression_target(&annotation.name)
    else {
        return;
    };
    let key = (target_module.clone(), qualified_name.clone());
    if index.kinds.get(&key) != Some(&DeclarationKind::Annotation) {
        diagnostics.push(SemanticDiagnostic {
            module: module.to_owned(),
            range: annotation.range,
            message: "annotation name does not refer to an annotation declaration".to_owned(),
        });
        return;
    }
    let Some(targets) = index.annotation_targets.get(&key) else {
        return;
    };
    let allowed = match applied_to {
        None => targets.file,
        Some(DeclarationKind::Const) => targets.constant,
        Some(DeclarationKind::Enum) => targets.enumeration,
        Some(DeclarationKind::Enumerant) => targets.enumerant,
        Some(DeclarationKind::Struct) => targets.structure,
        Some(DeclarationKind::Field) => targets.field,
        Some(DeclarationKind::Union) => targets.union,
        Some(DeclarationKind::Group) => targets.group,
        Some(DeclarationKind::Interface) => targets.interface,
        Some(DeclarationKind::Method) => targets.method,
        Some(DeclarationKind::Annotation) => targets.annotation,
        Some(DeclarationKind::Alias | DeclarationKind::Unknown) => false,
    };
    if !allowed {
        diagnostics.push(SemanticDiagnostic {
            module: module.to_owned(),
            range: annotation.range,
            message: "annotation is not valid on this declaration kind".to_owned(),
        });
    }
}

fn resolve_expression(
    expression: Option<&mut Expression>,
    module: &str,
    context: Option<&str>,
    imports: &BTreeMap<(Option<String>, String), String>,
    generic_scopes: &BTreeMap<String, Vec<String>>,
    index: &SymbolIndex,
    diagnostics: &mut Vec<SemanticDiagnostic>,
) {
    let Some(expression) = expression else {
        return;
    };
    match expression {
        Expression::Name {
            absolute,
            path,
            target,
            range,
        } => {
            if *target != NameTarget::Pending {
                return;
            }
            *target = resolve_name(
                *absolute,
                path,
                module,
                context,
                imports,
                generic_scopes,
                index,
            );
            if *target == NameTarget::Unresolved {
                diagnostics.push(SemanticDiagnostic {
                    module: module.to_owned(),
                    range: *range,
                    message: format!("unresolved name `{}`", path.join(".")),
                });
            }
        }
        Expression::List { values, .. } => {
            for value in values {
                resolve_expression(
                    Some(value),
                    module,
                    context,
                    imports,
                    generic_scopes,
                    index,
                    diagnostics,
                );
            }
        }
        Expression::Tuple { values, .. } => {
            for (_, value) in values {
                resolve_expression(
                    Some(value),
                    module,
                    context,
                    imports,
                    generic_scopes,
                    index,
                    diagnostics,
                );
            }
        }
        Expression::Apply {
            function,
            arguments,
            ..
        } => {
            resolve_expression(
                Some(function),
                module,
                context,
                imports,
                generic_scopes,
                index,
                diagnostics,
            );
            for (_, value) in arguments {
                resolve_expression(
                    Some(value),
                    module,
                    context,
                    imports,
                    generic_scopes,
                    index,
                    diagnostics,
                );
            }
        }
        Expression::Member {
            base,
            name,
            target,
            range,
        } => {
            resolve_expression(
                Some(base),
                module,
                context,
                imports,
                generic_scopes,
                index,
                diagnostics,
            );
            *target = match terminal_target(base) {
                Some(NameTarget::Declaration {
                    module: target_module,
                    qualified_name,
                }) => resolve_member_tail(
                    target_module,
                    qualified_name,
                    std::slice::from_ref(name),
                    index,
                ),
                _ => NameTarget::Unresolved,
            };
            if *target == NameTarget::Unresolved {
                diagnostics.push(SemanticDiagnostic {
                    module: module.to_owned(),
                    range: *range,
                    message: format!("unresolved member `{name}`"),
                });
            }
        }
        Expression::Import {
            path,
            member_path,
            target,
            range,
        } => {
            let imported = resolve_import_path(module, path);
            *target = if member_path.is_empty() {
                NameTarget::Module { path: imported }
            } else {
                resolve_member_tail(&imported, "", member_path, index)
            };
            if *target == NameTarget::Unresolved {
                diagnostics.push(SemanticDiagnostic {
                    module: module.to_owned(),
                    range: *range,
                    message: format!("unresolved imported name `{}`", member_path.join(".")),
                });
            }
        }
        Expression::Embed { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Binary { .. }
        | Expression::Unknown { .. } => {}
    }
}

fn resolve_value_from_expected_type(
    expected: Option<&Expression>,
    value: Option<&mut Expression>,
    index: &SymbolIndex,
) {
    let Some(Expression::Name {
        target:
            NameTarget::Declaration {
                module: expected_module,
                qualified_name,
            },
        ..
    }) = expected
    else {
        return;
    };
    let Some(Expression::Name {
        absolute: false,
        path,
        target,
        ..
    }) = value
    else {
        return;
    };
    if path.len() != 1 || *target != NameTarget::Pending {
        return;
    }
    if let Some(child) = index.children.get(&(
        expected_module.clone(),
        Some(qualified_name.clone()),
        path[0].clone(),
    )) {
        *target = NameTarget::Declaration {
            module: expected_module.clone(),
            qualified_name: child.clone(),
        };
    }
}

fn resolve_name(
    absolute: bool,
    path: &[String],
    module: &str,
    context: Option<&str>,
    imports: &BTreeMap<(Option<String>, String), String>,
    generic_scopes: &BTreeMap<String, Vec<String>>,
    index: &SymbolIndex,
) -> NameTarget {
    let Some(first) = path.first() else {
        return NameTarget::Unresolved;
    };
    if path.len() == 1 && is_builtin(first) {
        return NameTarget::Builtin;
    }
    if !absolute {
        let mut scope = context.map(str::to_owned);
        while let Some(current) = scope {
            if generic_scopes
                .get(&current)
                .is_some_and(|parameters| parameters.contains(first))
                && path.len() == 1
            {
                return NameTarget::GenericParameter {
                    declaration: current,
                    name: first.clone(),
                };
            }
            if let Some(imported) = imports.get(&(Some(current.clone()), first.clone())) {
                return resolve_import_tail(imported, path, index);
            }
            if let Some(qualified) =
                index
                    .children
                    .get(&(module.to_owned(), Some(current.clone()), first.clone()))
            {
                return resolve_member_tail(module, qualified, &path[1..], index);
            }
            scope = current
                .rsplit_once('.')
                .map(|(parent, _)| parent.to_owned());
        }
        if let Some(imported) = imports.get(&(None, first.clone())) {
            return resolve_import_tail(imported, path, index);
        }
    }
    if let Some(qualified) = index
        .children
        .get(&(module.to_owned(), None, first.clone()))
    {
        return resolve_member_tail(module, qualified, &path[1..], index);
    }
    NameTarget::Unresolved
}

fn resolve_import_tail(imported: &str, path: &[String], index: &SymbolIndex) -> NameTarget {
    if path.len() == 1 {
        NameTarget::Module {
            path: imported.to_owned(),
        }
    } else {
        resolve_member_tail(imported, "", &path[1..], index)
    }
}

fn resolve_member_tail(
    module: &str,
    base: &str,
    tail: &[String],
    index: &SymbolIndex,
) -> NameTarget {
    let mut current = base.to_owned();
    for name in tail {
        let parent = (!current.is_empty()).then_some(current.clone());
        let Some(next) = index
            .children
            .get(&(module.to_owned(), parent, name.clone()))
        else {
            return NameTarget::Unresolved;
        };
        current = next.clone();
    }
    if current.is_empty() {
        NameTarget::Module {
            path: module.to_owned(),
        }
    } else {
        NameTarget::Declaration {
            module: module.to_owned(),
            qualified_name: current,
        }
    }
}

fn parse_declaration(
    statement: &Statement,
    parent: Option<&str>,
    parent_kind: Option<DeclarationKind>,
) -> Option<ResolvedDeclaration> {
    let tokens = &statement.tokens;
    let first = identifier(tokens.first()?)?;
    let begins_named_declaration = matches!(
        first,
        "using" | "const" | "enum" | "struct" | "interface" | "annotation"
    ) && tokens.get(1).and_then(identifier).is_some();
    let contextual = match parent_kind {
        Some(DeclarationKind::Enum) => Some((DeclarationKind::Enumerant, 0)),
        Some(DeclarationKind::Interface) if !begins_named_declaration => {
            Some((DeclarationKind::Method, 0))
        }
        Some(DeclarationKind::Struct | DeclarationKind::Union | DeclarationKind::Group)
            if !begins_named_declaration
                && tokens.iter().any(|token| operator(token) == Some(":")) =>
        {
            Some(if has_named_group_marker(tokens, "group") {
                (DeclarationKind::Group, 0)
            } else if has_named_group_marker(tokens, "union") {
                (DeclarationKind::Union, 0)
            } else {
                (DeclarationKind::Field, 0)
            })
        }
        _ => None,
    };
    let (kind, name_index) = contextual.unwrap_or(match first {
        "using" => (DeclarationKind::Alias, 1),
        "const" => (DeclarationKind::Const, 1),
        "enum" => (DeclarationKind::Enum, 1),
        "struct" => (DeclarationKind::Struct, 1),
        "interface" => (DeclarationKind::Interface, 1),
        "annotation" => (DeclarationKind::Annotation, 1),
        "union" => (DeclarationKind::Union, 0),
        _ => (DeclarationKind::Unknown, 0),
    });
    if kind == DeclarationKind::Unknown {
        return None;
    }
    let name = if kind == DeclarationKind::Union && name_index == 0 && first == "union" {
        "union".to_owned()
    } else if kind == DeclarationKind::Alias
        && !tokens.iter().any(|token| operator(token) == Some("="))
    {
        tokens
            .iter()
            .rev()
            .find_map(identifier)
            .unwrap_or(first)
            .to_owned()
    } else {
        identifier(tokens.get(name_index)?)?.to_owned()
    };
    let qualified_name = parent.map_or_else(|| name.clone(), |parent| format!("{parent}.{name}"));
    let (id, id_range) = find_id(tokens, kind).unzip();
    let generic_parameters = find_generics(tokens, kind);
    let annotation_targets = (kind == DeclarationKind::Annotation)
        .then(|| parse_annotation_targets(tokens))
        .flatten();
    let annotations = parse_annotations(tokens);
    let (expression, value) = declaration_expressions(tokens, kind);
    Some(ResolvedDeclaration {
        name,
        qualified_name,
        parent: parent.map(str::to_owned),
        kind,
        is_unnamed_union: kind == DeclarationKind::Union && first == "union",
        id,
        id_range,
        generic_parameters,
        annotation_targets,
        expression,
        value,
        annotations,
        doc_comment: statement.doc_comment.clone(),
        range: statement.range,
    })
}

fn declaration_expressions(
    tokens: &[Token],
    kind: DeclarationKind,
) -> (Option<Expression>, Option<Expression>) {
    match kind {
        DeclarationKind::Alias => {
            let start = tokens
                .iter()
                .position(|token| operator(token) == Some("="))
                .map_or(1, |index| index + 1);
            (parse_expression(&tokens[start..]), None)
        }
        DeclarationKind::Const | DeclarationKind::Field => {
            let colon = tokens.iter().position(|token| operator(token) == Some(":"));
            let equals = tokens.iter().position(|token| operator(token) == Some("="));
            let expression = colon.and_then(|colon| {
                let end = equals.unwrap_or_else(|| annotation_start(tokens));
                parse_expression(&tokens[colon + 1..end])
            });
            let value = equals
                .and_then(|equals| parse_expression(&tokens[equals + 1..annotation_start(tokens)]));
            (expression, value)
        }
        DeclarationKind::Annotation => {
            let colon = tokens.iter().position(|token| operator(token) == Some(":"));
            (
                colon.and_then(|index| {
                    parse_expression(&tokens[index + 1..annotation_start(tokens)])
                }),
                None,
            )
        }
        DeclarationKind::Interface => {
            let extends = tokens
                .iter()
                .position(|token| token_is_identifier(Some(token), "extends"));
            (
                extends.and_then(|index| parse_expression(&tokens[index + 1..])),
                None,
            )
        }
        DeclarationKind::Method => {
            let params = tokens
                .iter()
                .position(|token| matches!(token.kind, TokenKind::Parenthesized(_)));
            let results = tokens
                .iter()
                .position(|token| operator(token) == Some("->"));
            let expression =
                params.and_then(|index| parse_expression(std::slice::from_ref(&tokens[index])));
            let value = results.and_then(|index| parse_expression(&tokens[index + 1..]));
            (expression, value)
        }
        DeclarationKind::Enum
        | DeclarationKind::Enumerant
        | DeclarationKind::Struct
        | DeclarationKind::Union
        | DeclarationKind::Group
        | DeclarationKind::Unknown => (None, None),
    }
}

fn parse_expression(tokens: &[Token]) -> Option<Expression> {
    let mut cursor = 0usize;
    parse_expression_at(tokens, &mut cursor)
}

fn parse_expression_at(tokens: &[Token], cursor: &mut usize) -> Option<Expression> {
    let token = tokens.get(*cursor)?;
    let mut expression = if operator(token) == Some("-") {
        *cursor += 1;
        let value = tokens.get(*cursor)?;
        match value.kind {
            TokenKind::IntegerLiteral(magnitude) => Expression::Integer {
                negative: true,
                magnitude,
                range: merge(token.range, value.range),
            },
            TokenKind::FloatLiteral(value) => Expression::Float {
                value: -value,
                range: merge(token.range, tokens[*cursor].range),
            },
            _ => Expression::Unknown { range: token.range },
        }
    } else if operator(token) == Some(".") {
        *cursor += 1;
        let name = identifier(tokens.get(*cursor)?)?.to_owned();
        Expression::Name {
            absolute: true,
            path: vec![name],
            target: NameTarget::Pending,
            range: merge(token.range, tokens[*cursor].range),
        }
    } else {
        match &token.kind {
            TokenKind::Identifier(name) if name == "import" || name == "embed" => {
                *cursor += 1;
                let next = tokens.get(*cursor)?;
                let TokenKind::StringLiteral(path) = &next.kind else {
                    return Some(Expression::Unknown { range: token.range });
                };
                if name == "import" {
                    Expression::Import {
                        path: path.clone(),
                        member_path: Vec::new(),
                        target: NameTarget::Pending,
                        range: merge(token.range, next.range),
                    }
                } else {
                    Expression::Embed {
                        path: path.clone(),
                        range: merge(token.range, next.range),
                    }
                }
            }
            TokenKind::Identifier(name) => Expression::Name {
                absolute: false,
                path: vec![name.clone()],
                target: NameTarget::Pending,
                range: token.range,
            },
            TokenKind::IntegerLiteral(magnitude) => Expression::Integer {
                negative: false,
                magnitude: *magnitude,
                range: token.range,
            },
            TokenKind::FloatLiteral(value) => Expression::Float {
                value: *value,
                range: token.range,
            },
            TokenKind::StringLiteral(value) => Expression::String {
                value: value.clone(),
                range: token.range,
            },
            TokenKind::BinaryLiteral(value) => Expression::Binary {
                value: value.clone(),
                range: token.range,
            },
            TokenKind::Bracketed(items) => Expression::List {
                values: parse_items(items)
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect(),
                range: token.range,
            },
            TokenKind::Parenthesized(items) => Expression::Tuple {
                values: parse_items(items),
                range: token.range,
            },
            TokenKind::Operator(_) | TokenKind::Invalid(_) => {
                Expression::Unknown { range: token.range }
            }
        }
    };
    *cursor += 1;
    loop {
        if tokens.get(*cursor).and_then(operator) == Some(".") {
            let Some(name) = tokens.get(*cursor + 1).and_then(identifier) else {
                break;
            };
            if let Expression::Name { path, range, .. } = &mut expression {
                path.push(name.to_owned());
                *range = merge(*range, tokens[*cursor + 1].range);
                *cursor += 2;
                continue;
            }
            if let Expression::Import {
                member_path, range, ..
            } = &mut expression
            {
                member_path.push(name.to_owned());
                *range = merge(*range, tokens[*cursor + 1].range);
                *cursor += 2;
                continue;
            }
            let joined = merge(expression.range(), tokens[*cursor + 1].range);
            expression = Expression::Member {
                base: Box::new(expression),
                name: name.to_owned(),
                target: NameTarget::Pending,
                range: joined,
            };
            *cursor += 2;
            continue;
        }
        if let Some(Token {
            kind: TokenKind::Parenthesized(items),
            range,
        }) = tokens.get(*cursor)
        {
            let joined = merge(expression.range(), *range);
            expression = Expression::Apply {
                function: Box::new(expression),
                arguments: parse_items(items),
                range: joined,
            };
            *cursor += 1;
            continue;
        }
        break;
    }
    Some(expression)
}

fn parse_items(items: &[TokenSequence]) -> Vec<(Option<String>, Expression)> {
    items
        .iter()
        .filter_map(|item| {
            let delimiter = item
                .tokens
                .iter()
                .position(|token| matches!(operator(token), Some("=") | Some(":")));
            let name = delimiter
                .and_then(|_| item.tokens.first().and_then(identifier))
                .map(str::to_owned);
            let start = delimiter.map_or(0, |index| index + 1);
            parse_expression(&item.tokens[start..]).map(|value| (name, value))
        })
        .collect()
}

fn parse_annotations(tokens: &[Token]) -> Vec<AnnotationUse> {
    let mut output = Vec::new();
    let mut cursor = 0usize;
    while cursor < tokens.len() {
        if operator(&tokens[cursor]) != Some("$") {
            cursor += 1;
            continue;
        }
        let start = tokens[cursor].range;
        cursor += 1;
        let expression_start = cursor;
        let Some(expression) = parse_expression_at(tokens, &mut cursor) else {
            continue;
        };
        let (name, value) = match expression {
            Expression::Apply {
                function,
                mut arguments,
                ..
            } if arguments.len() == 1 && arguments[0].0.is_none() => {
                let (_, value) = arguments.remove(0);
                (*function, Some(value))
            }
            Expression::Apply {
                function,
                arguments,
                range,
            } => (
                *function,
                Some(Expression::Tuple {
                    values: arguments,
                    range,
                }),
            ),
            other => (other, None),
        };
        let end = tokens
            .get(cursor.saturating_sub(1).max(expression_start))
            .map_or(start, |token| token.range);
        output.push(AnnotationUse {
            name,
            value,
            range: merge(start, end),
        });
    }
    output
}

fn find_id(tokens: &[Token], kind: DeclarationKind) -> Option<(DeclarationId, SourceRange)> {
    let at = tokens
        .iter()
        .position(|token| operator(token) == Some("@"))?;
    let TokenKind::IntegerLiteral(value) = tokens.get(at + 1)?.kind else {
        return None;
    };
    if matches!(
        kind,
        DeclarationKind::Field
            | DeclarationKind::Enumerant
            | DeclarationKind::Method
            | DeclarationKind::Union
    ) {
        u16::try_from(value)
            .ok()
            .map(DeclarationId::Ordinal)
            .map(|id| (id, tokens[at + 1].range))
    } else {
        Some((DeclarationId::Uid(value), tokens[at + 1].range))
    }
}

fn find_generics(tokens: &[Token], kind: DeclarationKind) -> Vec<String> {
    let wanted = match kind {
        DeclarationKind::Struct | DeclarationKind::Interface => TokenListKind::Parenthesized,
        DeclarationKind::Method => TokenListKind::Bracketed,
        _ => return Vec::new(),
    };
    tokens
        .iter()
        .find_map(|token| match (&token.kind, wanted) {
            (TokenKind::Parenthesized(items), TokenListKind::Parenthesized)
            | (TokenKind::Bracketed(items), TokenListKind::Bracketed) => Some(items),
            _ => None,
        })
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.tokens.first().and_then(identifier).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_annotation_targets(tokens: &[Token]) -> Option<AnnotationTargets> {
    let items = tokens.iter().find_map(|token| match &token.kind {
        TokenKind::Parenthesized(items) => Some(items),
        _ => None,
    })?;
    let mut targets = AnnotationTargets::default();
    for item in items {
        let value = item
            .tokens
            .first()
            .and_then(|token| identifier(token).or_else(|| operator(token)));
        match value {
            Some("*") => {
                return Some(AnnotationTargets {
                    file: true,
                    constant: true,
                    enumeration: true,
                    enumerant: true,
                    structure: true,
                    field: true,
                    union: true,
                    group: true,
                    interface: true,
                    method: true,
                    parameter: true,
                    annotation: true,
                });
            }
            Some("file") => targets.file = true,
            Some("const") => targets.constant = true,
            Some("enum") => targets.enumeration = true,
            Some("enumerant") => targets.enumerant = true,
            Some("struct") => targets.structure = true,
            Some("field") => targets.field = true,
            Some("union") => targets.union = true,
            Some("group") => targets.group = true,
            Some("interface") => targets.interface = true,
            Some("method") => targets.method = true,
            Some("param") => targets.parameter = true,
            Some("annotation") => targets.annotation = true,
            Some(_) | None => {}
        }
    }
    Some(targets)
}

#[derive(Clone, Copy)]
enum TokenListKind {
    Parenthesized,
    Bracketed,
}

fn annotation_start(tokens: &[Token]) -> usize {
    tokens
        .iter()
        .position(|token| operator(token) == Some("$"))
        .unwrap_or(tokens.len())
}

fn naked_id(tokens: &[Token]) -> Option<u64> {
    if tokens.len() == 2 && operator(&tokens[0]) == Some("@") {
        if let TokenKind::IntegerLiteral(value) = tokens[1].kind {
            return Some(value);
        }
    }
    None
}

fn is_naked_annotation(tokens: &[Token]) -> bool {
    tokens.first().and_then(operator) == Some("$")
}

fn identifier(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Identifier(value) => Some(value),
        _ => None,
    }
}

fn operator(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Operator(value) => Some(value),
        _ => None,
    }
}

fn token_is_identifier(token: Option<&Token>, expected: &str) -> bool {
    token.and_then(identifier) == Some(expected)
}

fn has_named_group_marker(tokens: &[Token], marker: &str) -> bool {
    tokens
        .windows(2)
        .any(|pair| operator(&pair[0]) == Some(":") && token_is_identifier(pair.get(1), marker))
}

fn merge(left: SourceRange, right: SourceRange) -> SourceRange {
    SourceRange {
        start: left.start.min(right.start),
        end: left.end.max(right.end),
    }
}

fn normalize_rooted(path: &str) -> String {
    let mut parts = Vec::new();
    let normalized = path.replace('\\', "/");
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value),
        }
    }
    format!("/{}", parts.join("/"))
}

fn resolve_import_path(module: &str, requested: &str) -> String {
    if requested.starts_with('/') {
        normalize_rooted(requested)
    } else {
        let parent = module.rsplit_once('/').map_or("/", |(parent, _)| parent);
        normalize_rooted(&format!("{parent}/{requested}"))
    }
}

fn is_builtin(value: &str) -> bool {
    matches!(
        value,
        "Void"
            | "Bool"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Float32"
            | "Float64"
            | "Text"
            | "Data"
            | "List"
            | "AnyPointer"
            | "AnyStruct"
            | "AnyList"
            | "Capability"
            | "true"
            | "false"
            | "void"
            | "inf"
            | "nan"
            | "stream"
    )
}

fn expression_nodes(expression: Option<&Expression>) -> usize {
    let Some(expression) = expression else {
        return 0;
    };
    1 + match expression {
        Expression::List { values, .. } => values
            .iter()
            .map(|value| expression_nodes(Some(value)))
            .sum(),
        Expression::Tuple { values, .. } => values
            .iter()
            .map(|(_, value)| expression_nodes(Some(value)))
            .sum(),
        Expression::Apply {
            function,
            arguments,
            ..
        } => {
            expression_nodes(Some(function))
                + arguments
                    .iter()
                    .map(|(_, value)| expression_nodes(Some(value)))
                    .sum::<usize>()
        }
        Expression::Member { base, .. } => expression_nodes(Some(base)),
        _ => 0,
    }
}

fn collect_declaration_targets(
    expression: Option<&Expression>,
    output: &mut BTreeSet<(String, String)>,
) {
    let Some(expression) = expression else {
        return;
    };
    match expression {
        Expression::Name {
            target:
                NameTarget::Declaration {
                    module,
                    qualified_name,
                },
            ..
        }
        | Expression::Import {
            target:
                NameTarget::Declaration {
                    module,
                    qualified_name,
                },
            ..
        } => {
            output.insert((module.clone(), qualified_name.clone()));
        }
        Expression::List { values, .. } => {
            for value in values {
                collect_declaration_targets(Some(value), output);
            }
        }
        Expression::Tuple { values, .. } => {
            for (_, value) in values {
                collect_declaration_targets(Some(value), output);
            }
        }
        Expression::Apply {
            function,
            arguments,
            ..
        } => {
            collect_declaration_targets(Some(function), output);
            for (_, value) in arguments {
                collect_declaration_targets(Some(value), output);
            }
        }
        Expression::Member { base, target, .. } => {
            if let NameTarget::Declaration {
                module,
                qualified_name,
            } = target
            {
                output.insert((module.clone(), qualified_name.clone()));
            }
            collect_declaration_targets(Some(base), output);
        }
        _ => {}
    }
}

fn expression_target(expression: &Expression) -> Option<&NameTarget> {
    match expression {
        Expression::Name { target, .. }
        | Expression::Import { target, .. }
        | Expression::Member { target, .. } => Some(target),
        Expression::Apply { function, .. } => terminal_target(function),
        _ => None,
    }
}

fn terminal_target(expression: &Expression) -> Option<&NameTarget> {
    expression_target(expression)
}

fn collect_module_dependencies(module: &ResolvedModule, output: &mut BTreeSet<String>) {
    for annotation in &module.annotations {
        collect_expression_imports(Some(&annotation.name), output);
        collect_expression_imports(annotation.value.as_ref(), output);
    }
    for declaration in &module.declarations {
        collect_expression_imports(declaration.expression.as_ref(), output);
        collect_expression_imports(declaration.value.as_ref(), output);
        for annotation in &declaration.annotations {
            collect_expression_imports(Some(&annotation.name), output);
            collect_expression_imports(annotation.value.as_ref(), output);
        }
    }
}

fn collect_expression_imports(expression: Option<&Expression>, output: &mut BTreeSet<String>) {
    let Some(expression) = expression else {
        return;
    };
    match expression {
        Expression::Name { path, .. }
            if path.len() == 1 && path.first().is_some_and(|name| name == "stream") =>
        {
            output.insert("/capnp/stream.capnp".to_owned());
        }
        Expression::Import { path, .. } => {
            output.insert(path.clone());
        }
        Expression::List { values, .. } => {
            for value in values {
                collect_expression_imports(Some(value), output);
            }
        }
        Expression::Tuple { values, .. } => {
            for (_, value) in values {
                collect_expression_imports(Some(value), output);
            }
        }
        Expression::Apply {
            function,
            arguments,
            ..
        } => {
            collect_expression_imports(Some(function), output);
            for (_, value) in arguments {
                collect_expression_imports(Some(value), output);
            }
        }
        Expression::Member { base, .. } => collect_expression_imports(Some(base), output),
        Expression::Name { .. }
        | Expression::Embed { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Binary { .. }
        | Expression::Unknown { .. } => {}
    }
}

fn reaches_cycle(
    start: &(String, String),
    current: &(String, String),
    graph: &BTreeMap<(String, String), BTreeSet<(String, String)>>,
    visiting: &mut BTreeSet<(String, String)>,
    visited: &mut BTreeSet<(String, String)>,
) -> bool {
    if !visiting.insert(current.clone()) {
        return current == start;
    }
    if !visited.insert(current.clone()) {
        visiting.remove(current);
        return false;
    }
    let cyclic = graph.get(current).is_some_and(|edges| {
        edges
            .iter()
            .any(|edge| edge == start || reaches_cycle(start, edge, graph, visiting, visited))
    });
    visiting.remove(current);
    cyclic
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::compile_program;
    use capnp_schema::{CapnpVersion, CompiledSchema, LoadLimits, NodeKind, Value};

    const WIRE: &str = include_str!("../../../conformance/schemas/wire-fixture.capnp");
    const LANGUAGE: &str = include_str!("../../../conformance/schemas/language-fixture.capnp");
    const IMPORTS: &str = include_str!("../../../conformance/schemas/import-fixture.capnp");
    const LANGUAGE_REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-language-fixture.bin"
    ));
    const IMPORT_REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-import-fixture.bin"
    ));

    fn fixture_sources() -> ModuleSources {
        let mut sources = ModuleSources::default();
        sources.insert_explicit("/schemas/wire-fixture.capnp", WIRE);
        sources.insert_explicit("/schemas/language-fixture.capnp", LANGUAGE);
        sources.insert_explicit("/schemas/import-fixture.capnp", IMPORTS);
        sources
    }

    #[test]
    fn named_declarations_nested_in_interfaces_are_not_methods() {
        let mut sources = ModuleSources::default();
        sources.insert_explicit(
            "/nested-interface.capnp",
            r#"
                @0x8000000000000100;
                interface Outer @0x8000000000000101 {
                    struct Payload @0x8000000000000102 {
                        value @0 :UInt64;
                    }
                    enum Choice @0x8000000000000103 {
                        first @0;
                        second @1;
                    }
                    interface Callback @0x8000000000000104 {
                        call @0 (value :Payload) -> (result :Payload);
                    }
                    consume @0 (payload :Payload, choice :Choice, callback :Callback)
                        -> (result :Payload);
                }
            "#,
        );
        let program = sources.resolve("/nested-interface.capnp", ResolveLimits::default());
        assert!(program.is_valid(), "{:#?}", program.diagnostics);
        let module = program
            .module("/nested-interface.capnp")
            .expect("nested interface module");
        for (name, kind) in [
            ("Outer.Payload", DeclarationKind::Struct),
            ("Outer.Choice", DeclarationKind::Enum),
            ("Outer.Callback", DeclarationKind::Interface),
            ("Outer.consume", DeclarationKind::Method),
            ("Outer.Callback.call", DeclarationKind::Method),
        ] {
            assert_eq!(
                module
                    .declarations
                    .iter()
                    .find(|declaration| declaration.qualified_name == name)
                    .map(|declaration| declaration.kind),
                Some(kind),
                "wrong declaration kind for {name}"
            );
        }

        let schema = compile_program(
            &program,
            CapnpVersion {
                major: 1,
                minor: 0,
                micro: 2,
            },
        )
        .expect("compile nested interface declarations");
        let outer = schema
            .node(0x8000_0000_0000_0101)
            .expect("compiled outer interface");
        assert!(matches!(outer.kind, NodeKind::Interface(_)));
        assert!(matches!(
            schema.nested(outer.id, "Payload").map(|node| &node.kind),
            Some(NodeKind::Struct(_))
        ));
        assert!(matches!(
            schema.nested(outer.id, "Choice").map(|node| &node.kind),
            Some(NodeKind::Enum(_))
        ));
        assert!(matches!(
            schema.nested(outer.id, "Callback").map(|node| &node.kind),
            Some(NodeKind::Interface(_))
        ));
    }

    #[test]
    fn imports_generics_ids_constants_annotations_and_types_resolve() {
        let program =
            fixture_sources().resolve("/schemas/import-fixture.capnp", ResolveLimits::default());
        assert!(program.is_valid(), "{:#?}", program.diagnostics);
        assert_eq!(program.modules.len(), 3, "{program:#?}");
        let language = program
            .module("/schemas/language-fixture.capnp")
            .expect("language module");
        assert_eq!(language.file_id, Some(0xcd6b_fdd0_88fa_4545));
        assert_eq!(language.annotations.len(), 1);
        let generic = language
            .declarations
            .iter()
            .find(|declaration| declaration.qualified_name == "GenericService")
            .expect("generic interface");
        assert_eq!(generic.id, Some(DeclarationId::Uid(0xa452_e51f_e34f_10ac)));
        assert_eq!(generic.generic_parameters, ["T"]);
        let transform = language
            .declarations
            .iter()
            .find(|declaration| declaration.qualified_name == "GenericService.transform")
            .expect("generic method");
        assert_eq!(transform.generic_parameters, ["U"]);
        let answer = language
            .declarations
            .iter()
            .find(|declaration| declaration.qualified_name == "LanguageFixture.answer")
            .expect("constant");
        assert!(matches!(
            answer.value,
            Some(Expression::Integer {
                magnitude: 42,
                negative: false,
                ..
            })
        ));
        let imports = program
            .module("/schemas/import-fixture.capnp")
            .expect("import module");
        assert_eq!(
            imports
                .imports
                .iter()
                .map(|import| (import.name.as_str(), import.resolved_path.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Wire", "/schemas/wire-fixture.capnp"),
                ("Language", "/schemas/language-fixture.capnp")
            ]
        );
    }

    #[test]
    fn pinned_cpp_requests_agree_on_ids_names_kinds_imports_and_values() {
        let program =
            fixture_sources().resolve("/schemas/import-fixture.capnp", ResolveLimits::default());
        assert!(program.is_valid(), "{:#?}", program.diagnostics);
        let language = program
            .module("/schemas/language-fixture.capnp")
            .expect("language module");
        let oracle =
            CompiledSchema::from_code_generator_request(LANGUAGE_REQUEST, LoadLimits::default())
                .expect("pinned language compiler request");
        assert_eq!(language.file_id, Some(oracle.requested_files()[0].id));

        for declaration in &language.declarations {
            let Some(DeclarationId::Uid(id)) = declaration.id else {
                continue;
            };
            let node = oracle.node(id).expect("native explicit ID exists upstream");
            assert_eq!(node.short_name(), Some(declaration.name.as_str()));
            assert!(
                matches!(
                    (declaration.kind, &node.kind),
                    (DeclarationKind::Struct, NodeKind::Struct(_))
                        | (DeclarationKind::Enum, NodeKind::Enum(_))
                        | (DeclarationKind::Interface, NodeKind::Interface(_))
                        | (DeclarationKind::Const, NodeKind::Const(_))
                        | (DeclarationKind::Annotation, NodeKind::Annotation(_))
                ),
                "kind mismatch for {declaration:#?}: {node:#?}"
            );
            assert_eq!(
                declaration.generic_parameters,
                node.parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>()
            );
            if let NodeKind::Annotation(schema) = &node.kind {
                assert_eq!(declaration.annotation_targets, Some(schema.targets));
            }
        }

        let fixture = oracle
            .node(0xb4c4_35b2_1aa9_b116)
            .expect("oracle LanguageFixture");
        let answer = oracle
            .nested(fixture.id, "answer")
            .expect("oracle nested constant");
        assert!(matches!(
            answer.kind,
            NodeKind::Const(capnp_schema::ConstSchema {
                value: Value::UInt64(42),
                ..
            })
        ));
        let native_answer = language
            .declarations
            .iter()
            .find(|declaration| declaration.qualified_name == "LanguageFixture.answer")
            .expect("native nested constant");
        assert!(matches!(
            native_answer.value,
            Some(Expression::Integer { magnitude: 42, .. })
        ));

        let import_oracle =
            CompiledSchema::from_code_generator_request(IMPORT_REQUEST, LoadLimits::default())
                .expect("pinned import compiler request");
        let native_imports = program
            .module("/schemas/import-fixture.capnp")
            .expect("native import module")
            .imports
            .iter()
            .map(|import| {
                (
                    import.requested_path.as_str(),
                    program
                        .module(&import.resolved_path)
                        .and_then(|module| module.file_id)
                        .expect("imported native file ID"),
                )
            })
            .collect::<BTreeSet<_>>();
        let oracle_imports = import_oracle.requested_files()[0]
            .imports
            .iter()
            .map(|import| (import.name.as_str(), import.id))
            .collect::<BTreeSet<_>>();
        assert_eq!(native_imports, oracle_imports);
    }

    #[test]
    fn import_aliases_are_lexically_scoped_and_bare_aliases_use_final_name() {
        let mut sources = ModuleSources::default();
        sources.insert_explicit(
            "/entry.capnp",
            r#"
                @0x8000000000000020;
                using Root = import "/one.capnp";
                using Root.Thing;
                struct Container @0x8000000000000021 {
                    using Root = import "/two.capnp";
                    field @0 :Root.Thing;
                }
                struct Outer @0x8000000000000022 { field @0 :Thing; }
            "#,
        );
        sources.insert_explicit(
            "/one.capnp",
            "@0x8000000000000023; struct Thing @0x8000000000000024 {}",
        );
        sources.insert_explicit(
            "/two.capnp",
            "@0x8000000000000025; struct Thing @0x8000000000000026 {}",
        );
        let program = sources.resolve("/entry.capnp", ResolveLimits::default());
        assert!(program.is_valid(), "{program:#?}");
        let entry = program.module("/entry.capnp").expect("entry module");
        let field_target = |name: &str| {
            entry
                .declarations
                .iter()
                .find(|declaration| declaration.qualified_name == name)
                .and_then(|declaration| declaration.expression.as_ref())
                .and_then(|expression| match expression {
                    Expression::Name { target, .. } => Some(target.clone()),
                    _ => None,
                })
                .expect("resolved field target")
        };
        assert_eq!(
            field_target("Container.field"),
            NameTarget::Declaration {
                module: "/two.capnp".to_owned(),
                qualified_name: "Thing".to_owned(),
            }
        );
        assert_eq!(
            field_target("Outer.field"),
            NameTarget::Declaration {
                module: "/entry.capnp".to_owned(),
                qualified_name: "Thing".to_owned(),
            }
        );
        let bare_alias = entry
            .declarations
            .iter()
            .find(|declaration| declaration.name == "Thing")
            .expect("bare alias uses final path component");
        assert!(matches!(
            bare_alias.expression,
            Some(Expression::Name {
                target: NameTarget::Declaration { ref module, ref qualified_name },
                ..
            }) if module == "/one.capnp" && qualified_name == "Thing"
        ));
    }

    #[test]
    fn explicit_standard_override_and_insertion_order_are_deterministic() {
        let standard =
            "@0x8000000000000001; const version :UInt32 = 1; struct Type @0x8000000000000004 {}";
        let explicit =
            "@0x8000000000000001; const version :UInt32 = 2; struct Type @0x8000000000000004 {}";
        let entry = "@0x8000000000000002; using Base = import \"/capnp/base.capnp\"; struct Root @0x8000000000000003 { field @0 :Base.Type; }";

        let mut forward = ModuleSources::default();
        forward.insert_standard("/capnp/base.capnp", standard);
        forward.insert_explicit("/entry.capnp", entry);
        forward.insert_explicit("/capnp/base.capnp", explicit);
        let mut reverse = ModuleSources::default();
        reverse.insert_explicit("/capnp/base.capnp", explicit);
        reverse.insert_explicit("/entry.capnp", entry);
        reverse.insert_standard("/capnp/base.capnp", standard);

        let left = forward.resolve("/entry.capnp", ResolveLimits::default());
        let right = reverse.resolve("/entry.capnp", ResolveLimits::default());
        assert_eq!(left, right);
        assert!(left.is_valid(), "{left:#?}");
        let Some(base) = left.module("/capnp/base.capnp") else {
            assert!(left.module("/capnp/base.capnp").is_some(), "{left:#?}");
            return;
        };
        assert!(matches!(
            base.declarations[0].value,
            Some(Expression::Integer { magnitude: 2, .. })
        ));
    }

    #[test]
    fn duplicates_missing_names_and_value_cycles_are_diagnosed_stably() {
        let source = r#"
            @0x8000000000000010;
            const a :UInt32 = b;
            const b :UInt32 = a;
            struct Duplicate @0x8000000000000011 {}
            struct Duplicate @0x8000000000000012 {}
            struct UsesMissing @0x8000000000000013 { value @0 :Missing; }
        "#;
        let mut sources = ModuleSources::default();
        sources.insert_explicit("/bad.capnp", source);
        let program = sources.resolve("/bad.capnp", ResolveLimits::default());
        assert!(!program.is_valid());
        let messages = program
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert!(messages.contains(&"duplicate declaration name"));
        assert!(
            messages.contains(&"constant or alias cycle"),
            "{program:#?}"
        );
        assert!(messages.iter().any(|message| message.contains("Missing")));
        let mut sorted = program.diagnostics.clone();
        sorted.sort_by(|left, right| {
            (
                &left.module,
                left.range.start,
                left.range.end,
                &left.message,
            )
                .cmp(&(
                    &right.module,
                    right.range.start,
                    right.range.end,
                    &right.message,
                ))
        });
        assert_eq!(program.diagnostics, sorted);
    }

    #[test]
    fn annotation_targets_are_enforced() {
        let source = r#"
            @0x8000000000000030;
            annotation fieldOnly @0x8000000000000031 (field) :Void;
            $fieldOnly;
            struct Target @0x8000000000000032 {
                value @0 :UInt32 $fieldOnly;
            }
        "#;
        let mut sources = ModuleSources::default();
        sources.insert_explicit("/annotations.capnp", source);
        let program = sources.resolve("/annotations.capnp", ResolveLimits::default());
        assert_eq!(program.diagnostics.len(), 1, "{program:#?}");
        assert_eq!(
            program.diagnostics[0].message,
            "annotation is not valid on this declaration kind"
        );
    }

    #[test]
    fn direct_import_members_discover_modules_and_resolve() {
        let mut sources = ModuleSources::default();
        sources.insert_explicit(
            "/entry.capnp",
            r#"
                @0x8000000000000040;
                struct Root @0x8000000000000041 {
                    value @0 :import "/defs.capnp".Thing;
                }
            "#,
        );
        sources.insert_explicit(
            "/defs.capnp",
            "@0x8000000000000042; struct Thing @0x8000000000000043 {}",
        );
        let program = sources.resolve("/entry.capnp", ResolveLimits::default());
        assert!(program.is_valid(), "{program:#?}");
        assert_eq!(program.modules.len(), 2);
        let field = program
            .module("/entry.capnp")
            .and_then(|module| {
                module
                    .declarations
                    .iter()
                    .find(|declaration| declaration.qualified_name == "Root.value")
            })
            .expect("direct-import field");
        assert!(matches!(
            field.expression,
            Some(Expression::Import {
                target: NameTarget::Declaration { ref module, ref qualified_name },
                ..
            }) if module == "/defs.capnp" && qualified_name == "Thing"
        ));
    }

    #[test]
    fn semantic_work_limits_fail_deterministically() {
        let mut sources = ModuleSources::default();
        sources.insert_explicit(
            "/entry.capnp",
            r#"
                @0x8000000000000050;
                using Other = import "/other.capnp";
                const first :UInt32 = 1;
                const second :List(UInt32) = [1, 2, 3];
            "#,
        );
        sources.insert_explicit("/other.capnp", "@0x8000000000000051;");

        let modules = sources.resolve(
            "/entry.capnp",
            ResolveLimits {
                max_modules: 1,
                ..ResolveLimits::default()
            },
        );
        assert!(
            modules
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "module limit exceeded")
        );

        let declarations = sources.resolve(
            "/entry.capnp",
            ResolveLimits {
                max_declarations: 1,
                ..ResolveLimits::default()
            },
        );
        assert!(
            declarations
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "declaration limit exceeded")
        );

        let expressions = sources.resolve(
            "/entry.capnp",
            ResolveLimits {
                max_expression_nodes: 2,
                ..ResolveLimits::default()
            },
        );
        assert!(
            expressions
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "expression node limit exceeded")
        );
    }
}
