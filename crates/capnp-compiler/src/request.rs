//! M25 standard `CodeGeneratorRequest` construction and bootstrap support.
//!
//! The checked-in request for the pinned `schema.capnp` is the reflection seed.
//! It is data, not executable generated code, and lets clean Cargo builds create
//! standard request messages without invoking a system Cap'n Proto compiler.

use capnp_io::{FrameError, FrameLimits, encode_frame};
use capnp_message::{ArenaError, ExclusiveArena, ReaderLimits};
use capnp_schema::{
    Annotation, AnnotationSchema, AnnotationTargets, AnyPointerKind, AnyPointerType, Brand,
    BrandBinding, BrandScope, CapnpVersion, CompiledSchema, ConstSchema, DynamicError,
    DynamicInput, DynamicListBuilder, DynamicStructBuilder, ElementSize, EnumSchema, Enumerant,
    Field, FieldKind, Identifier, IdentifierTarget, Import, InterfaceSchema, LoadError, LoadLimits,
    MemberSourceInfo, Method, NestedNode, Node, NodeId, NodeKind, Ordinal, Parameter,
    RequestedFile, ScopeBinding, SourceInfo, StructSchema, Superclass, Type, Value,
};

use std::collections::{BTreeMap, BTreeSet};

use crate::SourceRange;
use crate::layout::{
    CompiledFieldKind, CompiledLayouts, compile_tuple_layout, generate_child_id,
    generate_method_params_id,
};
use crate::semantic::{
    AnnotationUse, DeclarationId, DeclarationKind, Expression, NameTarget, ResolvedDeclaration,
    ResolvedModule, ResolvedProgram,
};

const BOOTSTRAP_REQUEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
    "compiler-request-schema.bin"
));
const STREAM_RESULT_TYPE_ID: NodeId = 0x995f_9a33_77c0_b16e;

#[derive(Debug)]
pub enum RequestError {
    Bootstrap(LoadError),
    Dynamic(DynamicError),
    Arena(ArenaError),
    Frame(FrameError),
    MissingBootstrapType(&'static str),
    CountOverflow,
    Unsupported(&'static str),
    InvalidProgram,
    MissingModel(String),
    Model(LoadError),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for RequestError {}

impl From<DynamicError> for RequestError {
    fn from(value: DynamicError) -> Self {
        Self::Dynamic(value)
    }
}

impl From<ArenaError> for RequestError {
    fn from(value: ArenaError) -> Self {
        Self::Arena(value)
    }
}

impl From<FrameError> for RequestError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}

impl From<LoadError> for RequestError {
    fn from(value: LoadError) -> Self {
        Self::Model(value)
    }
}

/// Compiles a resolved source program and its deterministic M24 layouts into
/// the standard owned schema model consumed by request emission and codegen.
pub fn compile_program(
    program: &ResolvedProgram,
    version: CapnpVersion,
) -> Result<CompiledSchema, RequestError> {
    if !program.is_valid() {
        return Err(RequestError::InvalidProgram);
    }
    let layouts = program.compile_layouts();
    if !layouts.is_valid() {
        return Err(RequestError::InvalidProgram);
    }
    ModelCompiler::new(program, &layouts)?.compile(version)
}

struct ModelCompiler<'a> {
    program: &'a ResolvedProgram,
    layouts: &'a CompiledLayouts,
    ids: BTreeMap<(String, String), NodeId>,
}

impl<'a> ModelCompiler<'a> {
    fn new(
        program: &'a ResolvedProgram,
        layouts: &'a CompiledLayouts,
    ) -> Result<Self, RequestError> {
        let mut compiler = Self {
            program,
            layouts,
            ids: BTreeMap::new(),
        };
        for module in &program.modules {
            let file_id = module.file_id.ok_or_else(|| {
                RequestError::MissingModel(format!("file ID for {}", module.path))
            })?;
            compiler
                .ids
                .insert((module.path.clone(), String::new()), file_id);
            for declaration in &module.declarations {
                if is_node_declaration(declaration.kind) {
                    compiler.assign_declaration_id(module, declaration)?;
                }
            }
        }
        for layout in &layouts.structs {
            if let Some(id) = layout.id {
                compiler
                    .ids
                    .insert((layout.module.clone(), layout.qualified_name.clone()), id);
            }
        }
        for module in &program.modules {
            for declaration in &module.declarations {
                if declaration.kind != DeclarationKind::Method {
                    continue;
                }
                let parent = declaration
                    .parent
                    .as_deref()
                    .ok_or(RequestError::Unsupported("method without an interface"))?;
                let interface_id = compiler
                    .ids
                    .get(&(module.path.clone(), parent.to_owned()))
                    .copied()
                    .ok_or_else(|| {
                        RequestError::MissingModel(format!("interface ID for {parent}"))
                    })?;
                let ordinal = match declaration.id {
                    Some(DeclarationId::Ordinal(value)) => value,
                    _ => return Err(RequestError::Unsupported("method without an ordinal")),
                };
                compiler.ids.insert(
                    (module.path.clone(), method_node_key(declaration, false)),
                    generate_method_params_id(interface_id, ordinal, false),
                );
                compiler.ids.insert(
                    (module.path.clone(), method_node_key(declaration, true)),
                    if is_stream_method(declaration) {
                        STREAM_RESULT_TYPE_ID
                    } else {
                        generate_method_params_id(interface_id, ordinal, true)
                    },
                );
            }
        }
        Ok(compiler)
    }

    fn compile(self, version: CapnpVersion) -> Result<CompiledSchema, RequestError> {
        let mut nodes = Vec::new();
        let mut source_info = Vec::new();
        for module in &self.program.modules {
            nodes.push(self.file_node(module)?);
            source_info.push(self.file_source_info(module)?);
            for declaration in &module.declarations {
                if self.is_included_node(module, declaration) {
                    let node = self.declaration_node(module, declaration)?;
                    source_info.push(self.node_source_info(module, declaration, &node));
                    nodes.push(node);
                }
            }
            for declaration in &module.declarations {
                if declaration.kind == DeclarationKind::Method {
                    let params = self.method_struct_node(module, declaration, false)?;
                    source_info.push(self.node_source_info(module, declaration, &params));
                    nodes.push(params);
                    if !is_stream_method(declaration) {
                        let results = self.method_struct_node(module, declaration, true)?;
                        source_info.push(self.node_source_info(module, declaration, &results));
                        nodes.push(results);
                    }
                }
            }
            for layout in &self.layouts.structs {
                if layout.module == module.path
                    && layout.is_group
                    && !self
                        .ids
                        .contains_key(&(module.path.clone(), layout.qualified_name.clone()))
                {
                    return Err(RequestError::MissingModel(format!(
                        "group ID for {}",
                        layout.qualified_name
                    )));
                }
                if layout.module == module.path && layout.is_group {
                    let declaration = self.declaration(module, &layout.qualified_name)?;
                    let node = self.struct_node(module, declaration, layout)?;
                    source_info.push(self.node_source_info(module, declaration, &node));
                    nodes.push(node);
                }
            }
        }
        self.reorder_nodes(&mut nodes)?;
        source_info.sort_by_key(|info| info.id);

        let entry = self
            .program
            .module(&self.program.entry)
            .ok_or_else(|| RequestError::MissingModel("entry module".to_owned()))?;
        let entry_id = entry
            .file_id
            .ok_or_else(|| RequestError::MissingModel("entry file ID".to_owned()))?;
        let mut imports = entry
            .imports
            .iter()
            .map(|import| {
                let id = self
                    .program
                    .module(&import.resolved_path)
                    .and_then(|module| module.file_id)
                    .ok_or_else(|| {
                        RequestError::MissingModel(format!(
                            "imported file ID for {}",
                            import.resolved_path
                        ))
                    })?;
                Ok(Import {
                    id,
                    name: import.requested_path.clone(),
                })
            })
            .collect::<Result<Vec<_>, RequestError>>()?;
        if entry.declarations.iter().any(is_stream_method) {
            imports.push(Import {
                id: 0x86c3_66a9_1393_f3f8,
                name: "/capnp/stream.capnp".to_owned(),
            });
        }
        imports.sort_by(|left, right| left.name.cmp(&right.name));
        let requested_files = vec![RequestedFile {
            id: entry_id,
            filename: request_filename(&entry.path),
            imports,
            identifiers: self.identifiers(entry)?,
        }];
        let provisional = CompiledSchema::from_parts(
            version,
            nodes.clone(),
            source_info.clone(),
            requested_files.clone(),
        )?;
        self.fill_aggregate_constants(&provisional, &mut nodes)?;
        Ok(CompiledSchema::from_parts(
            version,
            nodes,
            source_info,
            requested_files,
        )?)
    }

    fn assign_declaration_id(
        &mut self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
    ) -> Result<NodeId, RequestError> {
        let key = (module.path.clone(), declaration.qualified_name.clone());
        if let Some(id) = self.ids.get(&key) {
            return Ok(*id);
        }
        let id = match declaration.id {
            Some(DeclarationId::Uid(id)) => id,
            Some(DeclarationId::Ordinal(_)) | None => {
                let scope = self.node_parent(module, declaration)?;
                generate_child_id(scope, &declaration.name)
            }
        };
        self.ids.insert(key, id);
        Ok(id)
    }

    fn node_parent(
        &mut self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
    ) -> Result<NodeId, RequestError> {
        let Some(parent) = declaration.parent.as_deref() else {
            return module
                .file_id
                .ok_or_else(|| RequestError::MissingModel("file parent ID".to_owned()));
        };
        if let Some(id) = self.ids.get(&(module.path.clone(), parent.to_owned())) {
            return Ok(*id);
        }
        let parent_declaration = module
            .declarations
            .iter()
            .find(|item| item.qualified_name == parent)
            .ok_or_else(|| RequestError::MissingModel(format!("parent {parent}")))?;
        if is_node_declaration(parent_declaration.kind) {
            self.assign_declaration_id(module, parent_declaration)
        } else {
            let mut current = parent_declaration;
            loop {
                let Some(next) = current.parent.as_deref() else {
                    return module
                        .file_id
                        .ok_or_else(|| RequestError::MissingModel("file parent ID".to_owned()));
                };
                if let Some(id) = self.ids.get(&(module.path.clone(), next.to_owned())) {
                    return Ok(*id);
                }
                current = self.declaration(module, next)?;
            }
        }
    }

    fn file_node(&self, module: &ResolvedModule) -> Result<Node, RequestError> {
        let id = module
            .file_id
            .ok_or_else(|| RequestError::MissingModel("file node ID".to_owned()))?;
        let display_name = request_filename(&module.path);
        let display_name_prefix_length = display_name
            .rfind('.')
            .map_or(0, |index| u32::try_from(index + 1).unwrap_or(u32::MAX));
        Ok(Node {
            id,
            display_name,
            display_name_prefix_length,
            scope_id: 0,
            parameters: Vec::new(),
            is_generic: false,
            nested_nodes: self.nested_nodes(module, None)?,
            annotations: self.annotations(module, None, &module.annotations)?,
            kind: NodeKind::File,
            start_byte: 0,
            end_byte: 0,
        })
    }

    fn declaration_node(
        &self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
    ) -> Result<Node, RequestError> {
        let id = self.id(module, &declaration.qualified_name)?;
        let display_name = display_name(module, declaration);
        let prefix = display_prefix(&display_name)?;
        let kind = match declaration.kind {
            DeclarationKind::Struct => {
                let layout = self
                    .layouts
                    .structure(&module.path, &declaration.qualified_name)
                    .ok_or_else(|| {
                        RequestError::MissingModel(format!(
                            "layout for {}",
                            declaration.qualified_name
                        ))
                    })?;
                self.struct_kind(module, layout)?
            }
            DeclarationKind::Enum => NodeKind::Enum(EnumSchema {
                enumerants: module
                    .declarations
                    .iter()
                    .filter(|item| {
                        item.parent.as_deref() == Some(&declaration.qualified_name)
                            && item.kind == DeclarationKind::Enumerant
                    })
                    .enumerate()
                    .map(|(index, item)| {
                        Ok(Enumerant {
                            name: item.name.clone(),
                            code_order: u16::try_from(index)
                                .map_err(|_| RequestError::CountOverflow)?,
                            annotations: self.annotations(module, Some(item), &item.annotations)?,
                        })
                    })
                    .collect::<Result<Vec<_>, RequestError>>()?,
            }),
            DeclarationKind::Const => {
                let ty = self.ty(
                    module,
                    declaration
                        .expression
                        .as_ref()
                        .ok_or(RequestError::Unsupported("constant without a type"))?,
                )?;
                let value = self
                    .value(
                        module,
                        &ty,
                        declaration
                            .value
                            .as_ref()
                            .ok_or(RequestError::Unsupported("constant without a value"))?,
                    )
                    .map_err(|error| {
                        RequestError::MissingModel(format!(
                            "value for {}: {error}",
                            declaration.qualified_name
                        ))
                    })?;
                NodeKind::Const(ConstSchema { ty, value })
            }
            DeclarationKind::Annotation => NodeKind::Annotation(AnnotationSchema {
                ty: self.ty(
                    module,
                    declaration
                        .expression
                        .as_ref()
                        .ok_or(RequestError::Unsupported("annotation without a type"))?,
                )?,
                targets: declaration.annotation_targets.unwrap_or_default(),
            }),
            DeclarationKind::Interface => self.interface_kind(module, declaration)?,
            _ => return Err(RequestError::Unsupported("non-node declaration")),
        };
        Ok(Node {
            id,
            display_name,
            display_name_prefix_length: prefix,
            scope_id: self.scope_id(module, declaration)?,
            parameters: declaration
                .generic_parameters
                .iter()
                .map(|name| Parameter { name: name.clone() })
                .collect(),
            is_generic: self.is_generic(module, declaration),
            nested_nodes: self.nested_nodes(module, Some(&declaration.qualified_name))?,
            annotations: self.annotations(module, Some(declaration), &declaration.annotations)?,
            kind,
            start_byte: declaration.range.start,
            end_byte: declaration.range.end,
        })
    }

    fn struct_node(
        &self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
        layout: &crate::layout::CompiledStruct,
    ) -> Result<Node, RequestError> {
        let display_name = display_name(module, declaration);
        Ok(Node {
            id: self.id(module, &declaration.qualified_name)?,
            display_name_prefix_length: display_prefix(&display_name)?,
            display_name,
            scope_id: self.scope_id(module, declaration)?,
            parameters: Vec::new(),
            is_generic: self.is_generic(module, declaration),
            nested_nodes: self.nested_nodes(module, Some(&declaration.qualified_name))?,
            annotations: self.annotations(module, Some(declaration), &declaration.annotations)?,
            kind: self.struct_kind(module, layout)?,
            start_byte: declaration.range.start,
            end_byte: declaration.range.end,
        })
    }

    fn interface_kind(
        &self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
    ) -> Result<NodeKind, RequestError> {
        let methods = module
            .declarations
            .iter()
            .filter(|item| {
                item.parent.as_deref() == Some(&declaration.qualified_name)
                    && item.kind == DeclarationKind::Method
            })
            .enumerate()
            .map(|(code_order, method)| {
                let param_struct_type = self.id(module, &method_node_key(method, false))?;
                let result_struct_type = self.id(module, &method_node_key(method, true))?;
                let inherited_brand = if self.is_generic(module, declaration) {
                    Brand {
                        scopes: vec![BrandScope {
                            scope_id: self.id(module, &declaration.qualified_name)?,
                            binding: ScopeBinding::Inherit,
                        }],
                    }
                } else {
                    Brand::default()
                };
                Ok(Method {
                    name: method.name.clone(),
                    code_order: u16::try_from(code_order)
                        .map_err(|_| RequestError::CountOverflow)?,
                    implicit_parameters: method
                        .generic_parameters
                        .iter()
                        .map(|name| Parameter { name: name.clone() })
                        .collect(),
                    param_struct_type,
                    param_brand: inherited_brand.clone(),
                    result_struct_type,
                    result_brand: inherited_brand,
                    annotations: self.annotations(module, Some(method), &method.annotations)?,
                })
            })
            .collect::<Result<Vec<_>, RequestError>>()?;

        let mut superclasses = Vec::new();
        if let Some(expression) = &declaration.expression {
            let values = match expression {
                Expression::Tuple { values, .. } => {
                    values.iter().map(|(_, value)| value).collect::<Vec<_>>()
                }
                value => vec![value],
            };
            for value in values {
                let Type::Interface { type_id, brand } = self.ty(module, value)? else {
                    return Err(RequestError::Unsupported(
                        "interface superclass is not an interface",
                    ));
                };
                superclasses.push(Superclass { id: type_id, brand });
            }
        }
        Ok(NodeKind::Interface(InterfaceSchema {
            methods,
            superclasses,
        }))
    }

    fn method_struct_node(
        &self,
        module: &ResolvedModule,
        method: &ResolvedDeclaration,
        results: bool,
    ) -> Result<Node, RequestError> {
        let expression = if results {
            method.value.as_ref()
        } else {
            method.expression.as_ref()
        }
        .ok_or(RequestError::Unsupported("method tuple is missing"))?;
        let layout = compile_tuple_layout(self.program, module, expression)
            .ok_or(RequestError::Unsupported("method signature is not a tuple"))?;
        let node_id = self.id(module, &method_node_key(method, results))?;
        let fields = layout
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let ty = replace_implicit_scope(self.ty(module, &field.ty)?, node_id);
                Ok(Field {
                    name: field.name.clone(),
                    code_order: u16::try_from(index).map_err(|_| RequestError::CountOverflow)?,
                    annotations: Vec::new(),
                    discriminant_value: None,
                    kind: FieldKind::Slot {
                        offset: field.offset,
                        default_value: zero_value(&ty),
                        ty,
                        had_explicit_default: false,
                    },
                    ordinal: Ordinal::Explicit(
                        u16::try_from(index).map_err(|_| RequestError::CountOverflow)?,
                    ),
                })
            })
            .collect::<Result<Vec<_>, RequestError>>()?;
        let suffix = if results { "Results" } else { "Params" };
        let display_name = format!(
            "{}:{}${suffix}",
            request_filename(&module.path),
            method.qualified_name
        );
        Ok(Node {
            id: node_id,
            display_name_prefix_length: display_prefix(&display_name)?,
            display_name,
            scope_id: 0,
            parameters: method
                .generic_parameters
                .iter()
                .map(|name| Parameter { name: name.clone() })
                .collect(),
            is_generic: self.is_generic(module, method),
            nested_nodes: Vec::new(),
            annotations: Vec::new(),
            kind: NodeKind::Struct(StructSchema {
                data_word_count: layout.data_word_count,
                pointer_count: layout.pointer_count,
                preferred_list_encoding: ElementSize::InlineComposite,
                is_group: false,
                discriminant_count: 0,
                discriminant_offset: 0,
                fields,
            }),
            start_byte: method.range.start,
            end_byte: method.range.end,
        })
    }

    fn struct_kind(
        &self,
        module: &ResolvedModule,
        layout: &crate::layout::CompiledStruct,
    ) -> Result<NodeKind, RequestError> {
        let fields = layout
            .fields
            .iter()
            .map(|compiled| {
                let declaration = self.declaration(module, &compiled.qualified_name)?;
                let ordinal = compiled
                    .ordinal
                    .map_or(Ordinal::Implicit, Ordinal::Explicit);
                let kind = match &compiled.kind {
                    CompiledFieldKind::Group { qualified_name } => FieldKind::Group {
                        type_id: self.id(module, qualified_name)?,
                    },
                    CompiledFieldKind::Slot { offset, .. } => {
                        let ty = self.ty(
                            module,
                            declaration
                                .expression
                                .as_ref()
                                .ok_or(RequestError::Unsupported("field without a type"))?,
                        )?;
                        let default_value = match declaration.value.as_ref() {
                            Some(value) => self.value(module, &ty, value).map_err(|error| {
                                RequestError::MissingModel(format!(
                                    "default for {}: {error}",
                                    declaration.qualified_name
                                ))
                            })?,
                            None => zero_value(&ty),
                        };
                        FieldKind::Slot {
                            offset: *offset,
                            ty,
                            default_value,
                            had_explicit_default: declaration.value.is_some(),
                        }
                    }
                };
                Ok(Field {
                    name: compiled.name.clone(),
                    code_order: compiled.code_order,
                    annotations: self.annotations(
                        module,
                        Some(declaration),
                        &declaration.annotations,
                    )?,
                    discriminant_value: compiled.discriminant_value,
                    kind,
                    ordinal,
                })
            })
            .collect::<Result<Vec<_>, RequestError>>()?;
        Ok(NodeKind::Struct(StructSchema {
            data_word_count: layout.data_word_count,
            pointer_count: layout.pointer_count,
            preferred_list_encoding: ElementSize::InlineComposite,
            is_group: layout.is_group,
            discriminant_count: layout.discriminant_count,
            discriminant_offset: layout.discriminant_offset.unwrap_or(0),
            fields,
        }))
    }

    fn ty(&self, module: &ResolvedModule, expression: &Expression) -> Result<Type, RequestError> {
        match expression {
            Expression::Apply {
                function,
                arguments,
                ..
            } => {
                if builtin_name(function) == Some("List") {
                    let element = arguments
                        .first()
                        .ok_or(RequestError::Unsupported("List without an element type"))?;
                    return Ok(Type::List(Box::new(self.ty(module, &element.1)?)));
                }
                let (target_module, declaration) = self.target_declaration(module, function)?;
                let ty = self.declaration_type(target_module, declaration, Some(arguments))?;
                let brand = self.brand_for_expression(module, expression)?;
                Ok(with_brand(ty, brand))
            }
            Expression::Name {
                path,
                target: NameTarget::Builtin,
                ..
            } => builtin_type(path.first().map(String::as_str).unwrap_or_default())
                .ok_or(RequestError::Unsupported("unknown builtin type")),
            Expression::Name {
                target: NameTarget::GenericParameter { declaration, name },
                ..
            } => {
                let scope = self.declaration(module, declaration)?;
                let index = scope
                    .generic_parameters
                    .iter()
                    .position(|candidate| candidate == name)
                    .ok_or_else(|| RequestError::MissingModel(format!("generic {name}")))?;
                if scope.kind == DeclarationKind::Method {
                    return Ok(Type::AnyPointer(AnyPointerType::ImplicitMethodParameter {
                        index: u16::try_from(index).map_err(|_| RequestError::CountOverflow)?,
                    }));
                }
                Ok(Type::AnyPointer(AnyPointerType::Parameter {
                    scope_id: self.id(module, declaration)?,
                    index: u16::try_from(index).map_err(|_| RequestError::CountOverflow)?,
                }))
            }
            Expression::Name { .. } | Expression::Import { .. } | Expression::Member { .. } => {
                let (target_module, declaration) = self.target_declaration(module, expression)?;
                if declaration.kind == DeclarationKind::Alias {
                    return self.ty(
                        target_module,
                        declaration
                            .expression
                            .as_ref()
                            .ok_or(RequestError::Unsupported("alias without a target"))?,
                    );
                }
                self.declaration_type(target_module, declaration, None)
            }
            _ => Err(RequestError::Unsupported("expression is not a type")),
        }
    }

    fn declaration_type(
        &self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
        arguments: Option<&[(Option<String>, Expression)]>,
    ) -> Result<Type, RequestError> {
        if declaration.kind == DeclarationKind::Alias {
            return self.ty(
                module,
                declaration
                    .expression
                    .as_ref()
                    .ok_or(RequestError::Unsupported("alias without a target"))?,
            );
        }
        let type_id = self.id(module, &declaration.qualified_name)?;
        let brand = if declaration.generic_parameters.is_empty() {
            Brand::default()
        } else {
            let bindings = arguments.map_or_else(
                || vec![BrandBinding::Unbound; declaration.generic_parameters.len()],
                |values| {
                    values
                        .iter()
                        .map(|(_, value)| self.ty(module, value).map(BrandBinding::Type))
                        .collect::<Result<Vec<_>, RequestError>>()
                        .unwrap_or_default()
                },
            );
            Brand {
                scopes: vec![BrandScope {
                    scope_id: type_id,
                    binding: ScopeBinding::Bind(bindings),
                }],
            }
        };
        match declaration.kind {
            DeclarationKind::Enum => Ok(Type::Enum { type_id, brand }),
            DeclarationKind::Struct => Ok(Type::Struct { type_id, brand }),
            DeclarationKind::Interface => Ok(Type::Interface { type_id, brand }),
            _ => Err(RequestError::Unsupported("declaration is not a type")),
        }
    }

    fn brand_for_expression(
        &self,
        module: &ResolvedModule,
        expression: &Expression,
    ) -> Result<Brand, RequestError> {
        let mut scopes = Vec::new();
        self.collect_brand_scopes(module, expression, &mut scopes)?;
        Ok(Brand { scopes })
    }

    fn collect_brand_scopes(
        &self,
        module: &ResolvedModule,
        expression: &Expression,
        output: &mut Vec<BrandScope>,
    ) -> Result<(), RequestError> {
        match expression {
            Expression::Apply {
                function,
                arguments,
                ..
            } => {
                if builtin_name(function) != Some("List") {
                    let (target_module, declaration) = self.target_declaration(module, function)?;
                    if !declaration.generic_parameters.is_empty() {
                        let bindings = arguments
                            .iter()
                            .map(|(_, value)| self.ty(module, value).map(BrandBinding::Type))
                            .collect::<Result<Vec<_>, RequestError>>()?;
                        output.push(BrandScope {
                            scope_id: self.id(target_module, &declaration.qualified_name)?,
                            binding: ScopeBinding::Bind(bindings),
                        });
                    }
                    self.collect_brand_scopes(module, function, output)?;
                }
            }
            Expression::Member { base, .. } => {
                self.collect_brand_scopes(module, base, output)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn value(
        &self,
        _module: &ResolvedModule,
        ty: &Type,
        expression: &Expression,
    ) -> Result<Value, RequestError> {
        if let Some((target_module, qualified_name)) = expression_declaration_target(expression) {
            let target_module = self
                .program
                .module(target_module)
                .ok_or_else(|| RequestError::MissingModel(format!("module {target_module}")))?;
            let declaration = self.declaration(target_module, qualified_name)?;
            if declaration.kind == DeclarationKind::Const {
                return self.value(
                    target_module,
                    ty,
                    declaration
                        .value
                        .as_ref()
                        .ok_or(RequestError::Unsupported("constant without a value"))?,
                );
            }
        }
        match (ty, expression) {
            (Type::Bool, Expression::Name { path, .. })
                if path.first().is_some_and(|v| v == "true") =>
            {
                Ok(Value::Bool(true))
            }
            (Type::Bool, Expression::Name { path, .. })
                if path.first().is_some_and(|v| v == "false") =>
            {
                Ok(Value::Bool(false))
            }
            (
                Type::Int8,
                Expression::Integer {
                    negative,
                    magnitude,
                    ..
                },
            ) => Ok(Value::Int8(signed(*negative, *magnitude)?)),
            (
                Type::Int16,
                Expression::Integer {
                    negative,
                    magnitude,
                    ..
                },
            ) => Ok(Value::Int16(signed(*negative, *magnitude)?)),
            (
                Type::Int32,
                Expression::Integer {
                    negative,
                    magnitude,
                    ..
                },
            ) => Ok(Value::Int32(signed(*negative, *magnitude)?)),
            (
                Type::Int64,
                Expression::Integer {
                    negative,
                    magnitude,
                    ..
                },
            ) => Ok(Value::Int64(signed(*negative, *magnitude)?)),
            (
                Type::UInt8,
                Expression::Integer {
                    negative: false,
                    magnitude,
                    ..
                },
            ) => Ok(Value::UInt8(
                (*magnitude)
                    .try_into()
                    .map_err(|_| RequestError::Unsupported("integer range"))?,
            )),
            (
                Type::UInt16,
                Expression::Integer {
                    negative: false,
                    magnitude,
                    ..
                },
            ) => Ok(Value::UInt16(
                (*magnitude)
                    .try_into()
                    .map_err(|_| RequestError::Unsupported("integer range"))?,
            )),
            (
                Type::UInt32,
                Expression::Integer {
                    negative: false,
                    magnitude,
                    ..
                },
            ) => Ok(Value::UInt32(
                (*magnitude)
                    .try_into()
                    .map_err(|_| RequestError::Unsupported("integer range"))?,
            )),
            (
                Type::UInt64,
                Expression::Integer {
                    negative: false,
                    magnitude,
                    ..
                },
            ) => Ok(Value::UInt64(*magnitude)),
            (Type::Float32, Expression::Float { value, .. }) => Ok(Value::Float32(*value as f32)),
            (Type::Float64, Expression::Float { value, .. }) => Ok(Value::Float64(*value)),
            (Type::Text, Expression::String { value, .. }) => Ok(Value::Text(value.clone())),
            (Type::Data, Expression::Binary { value, .. }) => Ok(Value::Data(value.clone())),
            (
                Type::Enum { .. },
                Expression::Name {
                    target:
                        NameTarget::Declaration {
                            module: target_module,
                            qualified_name,
                        },
                    ..
                },
            ) => {
                let target = self
                    .program
                    .module(target_module)
                    .and_then(|module| {
                        module
                            .declarations
                            .iter()
                            .find(|item| item.qualified_name == *qualified_name)
                    })
                    .ok_or_else(|| {
                        RequestError::MissingModel(format!("enumerant {qualified_name}"))
                    })?;
                let DeclarationId::Ordinal(value) = target
                    .id
                    .ok_or(RequestError::Unsupported("enumerant without ordinal"))?
                else {
                    return Err(RequestError::Unsupported("enumerant UID"));
                };
                Ok(Value::Enum(value))
            }
            (Type::List(_), Expression::List { .. }) => {
                Ok(Value::List(capnp_schema::OpaquePointer::null()))
            }
            (Type::Struct { .. }, Expression::Tuple { .. }) => {
                Ok(Value::Struct(capnp_schema::OpaquePointer::null()))
            }
            _ => Err(RequestError::Unsupported("value does not match its type")),
        }
    }

    fn annotations(
        &self,
        module: &ResolvedModule,
        _declaration: Option<&ResolvedDeclaration>,
        values: &[AnnotationUse],
    ) -> Result<Vec<Annotation>, RequestError> {
        values
            .iter()
            .map(|value| {
                let (target_module, annotation) = self.target_declaration(module, &value.name)?;
                let id = self.id(target_module, &annotation.qualified_name)?;
                let ty = self.ty(
                    target_module,
                    annotation
                        .expression
                        .as_ref()
                        .ok_or(RequestError::Unsupported("annotation without a value type"))?,
                )?;
                let actual = match value.value.as_ref() {
                    Some(expression) => self.value(module, &ty, expression)?,
                    None => zero_value(&ty),
                };
                Ok(Annotation {
                    id,
                    brand: Brand::default(),
                    value: actual,
                })
            })
            .collect()
    }

    fn fill_aggregate_constants(
        &self,
        schema: &CompiledSchema,
        nodes: &mut [Node],
    ) -> Result<(), RequestError> {
        for module in &self.program.modules {
            for declaration in &module.declarations {
                if declaration.kind != DeclarationKind::Const
                    || !self.is_included_node(module, declaration)
                {
                    continue;
                }
                let id = self.id(module, &declaration.qualified_name)?;
                let node = nodes
                    .iter_mut()
                    .find(|node| node.id == id)
                    .ok_or_else(|| RequestError::MissingModel(format!("constant node {id:#x}")))?;
                let NodeKind::Const(constant) = &mut node.kind else {
                    return Err(RequestError::Unsupported("constant node kind mismatch"));
                };
                let expression = declaration
                    .value
                    .as_ref()
                    .ok_or(RequestError::Unsupported("constant without a value"))?;
                constant.value = match (&constant.ty, expression) {
                    (Type::List(element), Expression::List { .. }) => {
                        Value::List(self.build_list_pointer(schema, module, element, expression)?)
                    }
                    (Type::Struct { type_id, brand }, Expression::Tuple { .. }) => {
                        Value::Struct(self.build_struct_pointer(
                            schema,
                            module,
                            *type_id,
                            brand.clone(),
                            expression,
                        )?)
                    }
                    _ => continue,
                };
            }
        }
        Ok(())
    }

    fn reorder_nodes(&self, nodes: &mut Vec<Node>) -> Result<(), RequestError> {
        let mut order = Vec::new();
        let mut seen = BTreeSet::new();
        for module in &self.program.modules {
            if let Some(file_id) = module.file_id {
                push_existing(file_id, nodes, &mut order, &mut seen);
            }
            for declaration in module
                .declarations
                .iter()
                .filter(|item| item.parent.is_none() && is_node_declaration(item.kind))
            {
                self.append_declaration_order(module, declaration, nodes, &mut order, &mut seen)?;
            }
        }
        for node in nodes.iter() {
            if seen.insert(node.id) {
                order.push(node.id);
            }
        }
        let mut by_id = nodes
            .drain(..)
            .map(|node| (node.id, node))
            .collect::<BTreeMap<_, _>>();
        *nodes = order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect();
        Ok(())
    }

    fn append_declaration_order(
        &self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
        nodes: &[Node],
        order: &mut Vec<NodeId>,
        seen: &mut BTreeSet<NodeId>,
    ) -> Result<(), RequestError> {
        if declaration.kind == DeclarationKind::Interface {
            for method in module.declarations.iter().filter(|item| {
                item.parent.as_deref() == Some(&declaration.qualified_name)
                    && item.kind == DeclarationKind::Method
            }) {
                push_existing(
                    self.id(module, &method_node_key(method, false))?,
                    nodes,
                    order,
                    seen,
                );
                push_existing(
                    self.id(module, &method_node_key(method, true))?,
                    nodes,
                    order,
                    seen,
                );
            }
        }
        if declaration.kind == DeclarationKind::Struct {
            for nested in module.declarations.iter().filter(|item| {
                item.parent.as_deref() == Some(&declaration.qualified_name)
                    && item.kind == DeclarationKind::Enum
            }) {
                self.append_declaration_order(module, nested, nodes, order, seen)?;
            }
        }
        push_existing(
            self.id(module, &declaration.qualified_name)?,
            nodes,
            order,
            seen,
        );
        if declaration.kind == DeclarationKind::Struct {
            for child in module
                .declarations
                .iter()
                .filter(|item| item.parent.as_deref() == Some(&declaration.qualified_name))
            {
                match child.kind {
                    DeclarationKind::Enum => {}
                    kind if is_node_declaration(kind) => {
                        self.append_declaration_order(module, child, nodes, order, seen)?;
                    }
                    DeclarationKind::Group | DeclarationKind::Union if !child.is_unnamed_union => {
                        if let Ok(id) = self.id(module, &child.qualified_name) {
                            push_existing(id, nodes, order, seen);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn build_list_pointer(
        &self,
        schema: &CompiledSchema,
        module: &ResolvedModule,
        element: &Type,
        expression: &Expression,
    ) -> Result<capnp_schema::OpaquePointer, RequestError> {
        let Expression::List { values, .. } = expression else {
            return Err(RequestError::Unsupported("list value is not a list"));
        };
        let mut arena = ExclusiveArena::new(128, 1 << 24)?;
        {
            let mut builder = DynamicListBuilder::root(
                schema,
                &mut arena,
                element.clone(),
                count(values.len())?,
            )?;
            self.fill_list(module, element, values, &mut builder)?;
        }
        capnp_schema::OpaquePointer::from_root_segments(
            arena.into_segments(),
            ReaderLimits::default(),
        )
        .map_err(DynamicError::from)
        .map_err(RequestError::from)
    }

    fn build_struct_pointer(
        &self,
        schema: &CompiledSchema,
        module: &ResolvedModule,
        type_id: NodeId,
        brand: Brand,
        expression: &Expression,
    ) -> Result<capnp_schema::OpaquePointer, RequestError> {
        let mut arena = ExclusiveArena::new(128, 1 << 24)?;
        {
            let mut builder =
                DynamicStructBuilder::root_branded(schema, &mut arena, type_id, brand)?;
            self.fill_struct(module, expression, &mut builder)?;
        }
        capnp_schema::OpaquePointer::from_root_segments(
            arena.into_segments(),
            ReaderLimits::default(),
        )
        .map_err(DynamicError::from)
        .map_err(RequestError::from)
    }

    fn fill_struct(
        &self,
        module: &ResolvedModule,
        expression: &Expression,
        builder: &mut DynamicStructBuilder<'_, '_>,
    ) -> Result<(), RequestError> {
        let Expression::Tuple { values, .. } = expression else {
            return Err(RequestError::Unsupported("struct value is not a tuple"));
        };
        for (name, expression) in values {
            let name = name
                .as_deref()
                .ok_or(RequestError::Unsupported("unnamed struct value field"))?;
            let ty = builder.field_type(name)?;
            match (&ty, expression) {
                (Type::List(element), Expression::List { values, .. }) => {
                    let mut child = builder.init_list(name, count(values.len())?)?;
                    self.fill_list(module, element, values, &mut child)?;
                }
                (Type::Struct { .. }, Expression::Tuple { .. }) => {
                    let mut child = builder.init_struct(name)?;
                    self.fill_struct(module, expression, &mut child)?;
                }
                _ => builder.set(name, self.dynamic_input(module, &ty, expression)?)?,
            }
        }
        Ok(())
    }

    fn fill_list(
        &self,
        module: &ResolvedModule,
        element: &Type,
        values: &[Expression],
        builder: &mut DynamicListBuilder<'_, '_>,
    ) -> Result<(), RequestError> {
        for (index, expression) in values.iter().enumerate() {
            let index = count(index)?;
            match (element, expression) {
                (Type::List(nested), Expression::List { values, .. }) => {
                    let mut child = builder.init_list(index, count(values.len())?)?;
                    self.fill_list(module, nested, values, &mut child)?;
                }
                (Type::Struct { .. }, Expression::Tuple { .. }) => {
                    let mut child = builder.struct_element(index)?;
                    self.fill_struct(module, expression, &mut child)?;
                }
                _ => builder.set(index, self.dynamic_input(module, element, expression)?)?,
            }
        }
        Ok(())
    }

    fn dynamic_input<'b>(
        &self,
        module: &ResolvedModule,
        ty: &Type,
        expression: &'b Expression,
    ) -> Result<DynamicInput<'b>, RequestError> {
        Ok(match self.value(module, ty, expression)? {
            Value::Void => DynamicInput::Void,
            Value::Bool(value) => DynamicInput::Bool(value),
            Value::Int8(value) => DynamicInput::Int8(value),
            Value::Int16(value) => DynamicInput::Int16(value),
            Value::Int32(value) => DynamicInput::Int32(value),
            Value::Int64(value) => DynamicInput::Int64(value),
            Value::UInt8(value) => DynamicInput::UInt8(value),
            Value::UInt16(value) => DynamicInput::UInt16(value),
            Value::UInt32(value) => DynamicInput::UInt32(value),
            Value::UInt64(value) => DynamicInput::UInt64(value),
            Value::Float32(value) => DynamicInput::Float32(value),
            Value::Float64(value) => DynamicInput::Float64(value),
            Value::Text(_) => match expression {
                Expression::String { value, .. } => DynamicInput::Text(value),
                _ => return Err(RequestError::Unsupported("text expression")),
            },
            Value::Data(_) => match expression {
                Expression::Binary { value, .. } => DynamicInput::Data(value),
                _ => return Err(RequestError::Unsupported("data expression")),
            },
            Value::Enum(value) => DynamicInput::Enum(value),
            Value::Interface => {
                return Err(RequestError::Unsupported("capability constant"));
            }
            Value::List(_) | Value::Struct(_) | Value::AnyPointer(_) => {
                return Err(RequestError::Unsupported("nested aggregate input"));
            }
        })
    }

    fn target_declaration(
        &self,
        _current: &ResolvedModule,
        expression: &Expression,
    ) -> Result<(&'a ResolvedModule, &'a ResolvedDeclaration), RequestError> {
        let target = match expression {
            Expression::Name { target, .. }
            | Expression::Import { target, .. }
            | Expression::Member { target, .. } => target,
            Expression::Apply { function, .. } => {
                return self.target_declaration(_current, function);
            }
            _ => {
                return Err(RequestError::Unsupported(
                    "expression has no declaration target",
                ));
            }
        };
        let NameTarget::Declaration {
            module,
            qualified_name,
        } = target
        else {
            return Err(RequestError::Unsupported(
                "expression target is not a declaration",
            ));
        };
        let module = self
            .program
            .module(module)
            .ok_or_else(|| RequestError::MissingModel(format!("module {module}")))?;
        let declaration = self.declaration(module, qualified_name)?;
        Ok((module, declaration))
    }

    fn nested_nodes(
        &self,
        module: &ResolvedModule,
        parent: Option<&str>,
    ) -> Result<Vec<NestedNode>, RequestError> {
        module
            .declarations
            .iter()
            .filter(|item| item.parent.as_deref() == parent && is_node_declaration(item.kind))
            .map(|item| {
                Ok(NestedNode {
                    name: item.name.clone(),
                    id: self.id(module, &item.qualified_name)?,
                })
            })
            .collect()
    }

    fn is_included_node(&self, module: &ResolvedModule, declaration: &ResolvedDeclaration) -> bool {
        if !is_node_declaration(declaration.kind) {
            return false;
        }
        if module.path == self.program.entry {
            return true;
        }
        match declaration.kind {
            DeclarationKind::Const => false,
            DeclarationKind::Annotation => self.program.modules.iter().any(|candidate| {
                candidate
                    .annotations
                    .iter()
                    .chain(
                        candidate
                            .declarations
                            .iter()
                            .flat_map(|item| item.annotations.iter()),
                    )
                    .any(|annotation| {
                        expression_declaration_target(&annotation.name)
                            == Some((module.path.as_str(), declaration.qualified_name.as_str()))
                    })
            }),
            _ => true,
        }
    }

    fn identifiers(&self, module: &ResolvedModule) -> Result<Vec<Identifier>, RequestError> {
        let mut output = Vec::new();
        for annotation in &module.annotations {
            self.collect_expression_identifiers(module, &annotation.name, &mut output)?;
            if let Some(value) = &annotation.value {
                self.collect_expression_identifiers(module, value, &mut output)?;
            }
        }
        for declaration in &module.declarations {
            if let Some(expression) = &declaration.expression {
                self.collect_expression_identifiers(module, expression, &mut output)?;
            }
            if let Some(value) = &declaration.value {
                self.collect_expression_identifiers(module, value, &mut output)?;
            }
            for annotation in &declaration.annotations {
                self.collect_expression_identifiers(module, &annotation.name, &mut output)?;
                if let Some(value) = &annotation.value {
                    self.collect_expression_identifiers(module, value, &mut output)?;
                }
            }
        }
        output.sort_by_key(|identifier| (identifier.start_byte, identifier.end_byte));
        output.dedup();
        Ok(output)
    }

    fn collect_expression_identifiers(
        &self,
        module: &ResolvedModule,
        expression: &Expression,
        output: &mut Vec<Identifier>,
    ) -> Result<(), RequestError> {
        match expression {
            Expression::Name {
                path,
                target,
                range,
                absolute,
            } => {
                if path.len() > 1 {
                    if let Some(import) =
                        module.imports.iter().find(|import| import.name == path[0])
                    {
                        let imported_id = self
                            .program
                            .module(&import.resolved_path)
                            .and_then(|value| value.file_id)
                            .ok_or_else(|| {
                                RequestError::MissingModel(format!(
                                    "imported file ID for {}",
                                    import.resolved_path
                                ))
                            })?;
                        output.push(Identifier {
                            start_byte: range.start,
                            end_byte: range
                                .start
                                .checked_add(
                                    u32::try_from(path[0].len())
                                        .map_err(|_| RequestError::CountOverflow)?,
                                )
                                .ok_or(RequestError::CountOverflow)?,
                            target: IdentifierTarget::Type(imported_id),
                        });
                    }
                }
                if let NameTarget::Declaration {
                    module: target_module,
                    qualified_name,
                } = target
                {
                    self.collect_name_prefix_identifiers(
                        path,
                        *absolute,
                        *range,
                        target_module,
                        qualified_name,
                        output,
                    )?;
                }
                let target =
                    match target {
                        NameTarget::Builtin => path
                            .first()
                            .and_then(|name| builtin_type_id(name))
                            .map(IdentifierTarget::Type),
                        NameTarget::GenericParameter { .. } => None,
                        NameTarget::Declaration {
                            module: target_module,
                            qualified_name,
                        } => {
                            let target_module =
                                self.program.module(target_module).ok_or_else(|| {
                                    RequestError::MissingModel(format!("module {target_module}"))
                                })?;
                            let declaration = self.declaration(target_module, qualified_name)?;
                            match declaration.kind {
                                DeclarationKind::Enumerant => None,
                                DeclarationKind::Field | DeclarationKind::Method => {
                                    let parent = declaration.parent.as_deref().ok_or(
                                        RequestError::Unsupported("member without a parent"),
                                    )?;
                                    let ordinal = match declaration.id {
                                        Some(DeclarationId::Ordinal(value)) => value,
                                        _ => return Ok(()),
                                    };
                                    Some(IdentifierTarget::Member {
                                        parent_type_id: self.id(target_module, parent)?,
                                        ordinal,
                                    })
                                }
                                DeclarationKind::Alias => identifier_type_id(&self.ty(
                                    target_module,
                                    declaration.expression.as_ref().ok_or(
                                        RequestError::Unsupported("alias without a target"),
                                    )?,
                                )?)
                                .map(IdentifierTarget::Type),
                                _ => Some(IdentifierTarget::Type(
                                    self.id(target_module, qualified_name)?,
                                )),
                            }
                        }
                        NameTarget::Pending
                        | NameTarget::Module { .. }
                        | NameTarget::Unresolved => None,
                    };
                if let Some(target) = target {
                    output.push(Identifier {
                        start_byte: range.start,
                        end_byte: range.end,
                        target,
                    });
                }
            }
            Expression::Import { target, range, .. } => {
                let id = match target {
                    NameTarget::Declaration {
                        module: target_module,
                        qualified_name,
                    } => {
                        let target_module =
                            self.program.module(target_module).ok_or_else(|| {
                                RequestError::MissingModel(format!("module {target_module}"))
                            })?;
                        Some(self.id(target_module, qualified_name)?)
                    }
                    NameTarget::Module { path } => self
                        .program
                        .module(path)
                        .and_then(|target_module| target_module.file_id),
                    _ => None,
                };
                if let Some(id) = id {
                    output.push(Identifier {
                        start_byte: range.start,
                        end_byte: range.end,
                        target: IdentifierTarget::Type(id),
                    });
                }
            }
            Expression::List { values, .. } => {
                for value in values {
                    self.collect_expression_identifiers(module, value, output)?;
                }
            }
            Expression::Tuple { values, .. } => {
                for (_, value) in values {
                    self.collect_expression_identifiers(module, value, output)?;
                }
            }
            Expression::Apply {
                function,
                arguments,
                ..
            } => {
                self.collect_expression_identifiers(module, function, output)?;
                for (_, argument) in arguments {
                    self.collect_expression_identifiers(module, argument, output)?;
                }
            }
            Expression::Member {
                base,
                target,
                range,
                ..
            } => {
                self.collect_expression_identifiers(module, base, output)?;
                if let NameTarget::Declaration {
                    module: target_module,
                    qualified_name,
                } = target
                {
                    let target_module = self.program.module(target_module).ok_or_else(|| {
                        RequestError::MissingModel(format!("module {target_module}"))
                    })?;
                    output.push(Identifier {
                        start_byte: range.start,
                        end_byte: range.end,
                        target: IdentifierTarget::Type(self.id(target_module, qualified_name)?),
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
        Ok(())
    }

    fn collect_name_prefix_identifiers(
        &self,
        path: &[String],
        absolute: bool,
        range: SourceRange,
        target_module: &str,
        qualified_name: &str,
        output: &mut Vec<Identifier>,
    ) -> Result<(), RequestError> {
        if path.len() < 2 {
            return Ok(());
        }
        let target_module = self
            .program
            .module(target_module)
            .ok_or_else(|| RequestError::MissingModel(format!("module {target_module}")))?;
        let target_parts = qualified_name.split('.').collect::<Vec<_>>();
        if target_parts.len() < path.len() {
            return Ok(());
        }
        let context_len = target_parts.len() - path.len();
        let mut end = range.start + u32::from(absolute);
        for (index, segment) in path.iter().enumerate().take(path.len() - 1) {
            if index != 0 {
                end = end.checked_add(1).ok_or(RequestError::CountOverflow)?;
            }
            end = end
                .checked_add(u32::try_from(segment.len()).map_err(|_| RequestError::CountOverflow)?)
                .ok_or(RequestError::CountOverflow)?;
            let prefix = target_parts[..context_len + index + 1].join(".");
            let Some(declaration) = target_module
                .declarations
                .iter()
                .find(|declaration| declaration.qualified_name == prefix)
            else {
                continue;
            };
            if is_node_declaration(declaration.kind) {
                output.push(Identifier {
                    start_byte: range.start,
                    end_byte: end,
                    target: IdentifierTarget::Type(self.id(target_module, &prefix)?),
                });
            }
        }
        Ok(())
    }

    fn scope_id(
        &self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
    ) -> Result<NodeId, RequestError> {
        let Some(parent) = declaration.parent.as_deref() else {
            return module
                .file_id
                .ok_or_else(|| RequestError::MissingModel("scope file ID".to_owned()));
        };
        let mut current = self.declaration(module, parent)?;
        loop {
            if let Some(id) = self
                .ids
                .get(&(module.path.clone(), current.qualified_name.clone()))
            {
                return Ok(*id);
            }
            let Some(parent) = current.parent.as_deref() else {
                return module
                    .file_id
                    .ok_or_else(|| RequestError::MissingModel("scope file ID".to_owned()));
            };
            current = self.declaration(module, parent)?;
        }
    }

    fn is_generic(&self, module: &ResolvedModule, declaration: &ResolvedDeclaration) -> bool {
        let mut current = Some(declaration);
        while let Some(value) = current {
            if !value.generic_parameters.is_empty() {
                return true;
            }
            current = value
                .parent
                .as_deref()
                .and_then(|parent| self.declaration(module, parent).ok());
        }
        false
    }

    fn id(&self, module: &ResolvedModule, qualified_name: &str) -> Result<NodeId, RequestError> {
        self.ids
            .get(&(module.path.clone(), qualified_name.to_owned()))
            .copied()
            .ok_or_else(|| RequestError::MissingModel(format!("node ID for {qualified_name}")))
    }

    fn declaration<'b>(
        &self,
        module: &'b ResolvedModule,
        qualified_name: &str,
    ) -> Result<&'b ResolvedDeclaration, RequestError> {
        module
            .declarations
            .iter()
            .find(|item| item.qualified_name == qualified_name)
            .ok_or_else(|| RequestError::MissingModel(format!("declaration {qualified_name}")))
    }

    fn file_source_info(&self, module: &ResolvedModule) -> Result<SourceInfo, RequestError> {
        Ok(SourceInfo {
            id: module
                .file_id
                .ok_or_else(|| RequestError::MissingModel("file source ID".to_owned()))?,
            doc_comment: String::new(),
            members: Vec::new(),
            start_byte: 0,
            end_byte: 0,
        })
    }

    fn node_source_info(
        &self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
        node: &Node,
    ) -> SourceInfo {
        let member_ranges = match &node.kind {
            NodeKind::Struct(schema) => schema
                .fields
                .iter()
                .filter_map(|field| {
                    module.declarations.iter().find(|item| {
                        item.name == field.name
                            && item.parent.as_deref().is_some_and(|parent| {
                                parent.starts_with(&declaration.qualified_name)
                            })
                    })
                })
                .collect::<Vec<_>>(),
            NodeKind::Enum(_) => module
                .declarations
                .iter()
                .filter(|item| {
                    item.parent.as_deref() == Some(&declaration.qualified_name)
                        && item.kind == DeclarationKind::Enumerant
                })
                .collect(),
            NodeKind::Interface(_) => module
                .declarations
                .iter()
                .filter(|item| {
                    item.parent.as_deref() == Some(&declaration.qualified_name)
                        && item.kind == DeclarationKind::Method
                })
                .collect(),
            _ => Vec::new(),
        };
        SourceInfo {
            id: node.id,
            doc_comment: declaration.doc_comment.clone().unwrap_or_default(),
            members: member_ranges
                .into_iter()
                .map(|member| MemberSourceInfo {
                    doc_comment: member.doc_comment.clone().unwrap_or_default(),
                    start_byte: member.range.start,
                    end_byte: member.range.end,
                })
                .collect(),
            start_byte: declaration.range.start,
            end_byte: declaration.range.end,
        }
    }
}

fn is_node_declaration(kind: DeclarationKind) -> bool {
    matches!(
        kind,
        DeclarationKind::Const
            | DeclarationKind::Enum
            | DeclarationKind::Struct
            | DeclarationKind::Interface
            | DeclarationKind::Annotation
    )
}

fn request_filename(path: &str) -> String {
    path.trim_start_matches('/').to_owned()
}

fn method_node_key(method: &ResolvedDeclaration, results: bool) -> String {
    format!(
        "{}${}",
        method.qualified_name,
        if results { "Results" } else { "Params" }
    )
}

fn is_stream_method(method: &ResolvedDeclaration) -> bool {
    matches!(
        method.value.as_ref(),
        Some(Expression::Name {
            path,
            target: NameTarget::Builtin,
            ..
        }) if path.first().is_some_and(|name| name == "stream")
    )
}

fn expression_declaration_target(expression: &Expression) -> Option<(&str, &str)> {
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
        }
        | Expression::Member {
            target:
                NameTarget::Declaration {
                    module,
                    qualified_name,
                },
            ..
        } => Some((module, qualified_name)),
        Expression::Apply { function, .. } => expression_declaration_target(function),
        _ => None,
    }
}

fn push_existing(id: NodeId, nodes: &[Node], order: &mut Vec<NodeId>, seen: &mut BTreeSet<NodeId>) {
    if seen.insert(id) && nodes.iter().any(|node| node.id == id) {
        order.push(id);
    }
}

fn display_name(module: &ResolvedModule, declaration: &ResolvedDeclaration) -> String {
    let mut visible = Vec::new();
    let mut prefix = String::new();
    for part in declaration.qualified_name.split('.') {
        if !prefix.is_empty() {
            prefix.push('.');
        }
        prefix.push_str(part);
        let is_unnamed_union = module
            .declarations
            .iter()
            .find(|item| item.qualified_name == prefix)
            .is_some_and(|item| item.kind == DeclarationKind::Union && item.is_unnamed_union);
        if !is_unnamed_union {
            visible.push(part);
        }
    }
    format!("{}:{}", request_filename(&module.path), visible.join("."))
}

fn display_prefix(display_name: &str) -> Result<u32, RequestError> {
    let separator = display_name
        .rfind([':', '.'])
        .ok_or(RequestError::Unsupported(
            "display name without a separator",
        ))?;
    byte(separator + 1)
}

fn byte(value: usize) -> Result<u32, RequestError> {
    u32::try_from(value).map_err(|_| RequestError::CountOverflow)
}

fn builtin_name(expression: &Expression) -> Option<&str> {
    match expression {
        Expression::Name {
            path,
            target: NameTarget::Builtin,
            ..
        } => path.first().map(String::as_str),
        _ => None,
    }
}

fn builtin_type(name: &str) -> Option<Type> {
    Some(match name {
        "Void" => Type::Void,
        "Bool" => Type::Bool,
        "Int8" => Type::Int8,
        "Int16" => Type::Int16,
        "Int32" => Type::Int32,
        "Int64" => Type::Int64,
        "UInt8" => Type::UInt8,
        "UInt16" => Type::UInt16,
        "UInt32" => Type::UInt32,
        "UInt64" => Type::UInt64,
        "Float32" => Type::Float32,
        "Float64" => Type::Float64,
        "Text" => Type::Text,
        "Data" => Type::Data,
        "AnyPointer" => Type::AnyPointer(AnyPointerType::Unconstrained(AnyPointerKind::Any)),
        "AnyStruct" => Type::AnyPointer(AnyPointerType::Unconstrained(AnyPointerKind::Struct)),
        "AnyList" => Type::AnyPointer(AnyPointerType::Unconstrained(AnyPointerKind::List)),
        "Capability" => Type::AnyPointer(AnyPointerType::Unconstrained(AnyPointerKind::Capability)),
        _ => return None,
    })
}

fn builtin_type_id(name: &str) -> Option<NodeId> {
    Some(match name {
        "Void" => 1014,
        "Bool" => 1015,
        "Int8" => 1016,
        "Int16" => 1017,
        "Int32" => 1018,
        "Int64" => 1019,
        "UInt8" => 1020,
        "UInt16" => 1021,
        "UInt32" => 1022,
        "UInt64" => 1023,
        "Float32" => 1024,
        "Float64" => 1025,
        "Text" => 1026,
        "Data" => 1027,
        "List" => 1028,
        "AnyPointer" => 1030,
        "AnyStruct" => 1031,
        "AnyList" => 1032,
        "Capability" => 1033,
        _ => return None,
    })
}

fn identifier_type_id(ty: &Type) -> Option<NodeId> {
    match ty {
        Type::Void => builtin_type_id("Void"),
        Type::Bool => builtin_type_id("Bool"),
        Type::Int8 => builtin_type_id("Int8"),
        Type::Int16 => builtin_type_id("Int16"),
        Type::Int32 => builtin_type_id("Int32"),
        Type::Int64 => builtin_type_id("Int64"),
        Type::UInt8 => builtin_type_id("UInt8"),
        Type::UInt16 => builtin_type_id("UInt16"),
        Type::UInt32 => builtin_type_id("UInt32"),
        Type::UInt64 => builtin_type_id("UInt64"),
        Type::Float32 => builtin_type_id("Float32"),
        Type::Float64 => builtin_type_id("Float64"),
        Type::Text => builtin_type_id("Text"),
        Type::Data => builtin_type_id("Data"),
        Type::List(_) => builtin_type_id("List"),
        Type::Enum { type_id, .. }
        | Type::Struct { type_id, .. }
        | Type::Interface { type_id, .. } => Some(*type_id),
        Type::AnyPointer(AnyPointerType::Unconstrained(kind)) => builtin_type_id(match kind {
            AnyPointerKind::Any => "AnyPointer",
            AnyPointerKind::Struct => "AnyStruct",
            AnyPointerKind::List => "AnyList",
            AnyPointerKind::Capability => "Capability",
        }),
        Type::AnyPointer(
            AnyPointerType::Parameter { .. } | AnyPointerType::ImplicitMethodParameter { .. },
        ) => None,
    }
}

fn replace_implicit_scope(ty: Type, scope_id: NodeId) -> Type {
    match ty {
        Type::List(element) => Type::List(Box::new(replace_implicit_scope(*element, scope_id))),
        Type::Enum { type_id, brand } => Type::Enum {
            type_id,
            brand: replace_implicit_brand(brand, scope_id),
        },
        Type::Struct { type_id, brand } => Type::Struct {
            type_id,
            brand: replace_implicit_brand(brand, scope_id),
        },
        Type::Interface { type_id, brand } => Type::Interface {
            type_id,
            brand: replace_implicit_brand(brand, scope_id),
        },
        Type::AnyPointer(AnyPointerType::ImplicitMethodParameter { index }) => {
            Type::AnyPointer(AnyPointerType::Parameter { scope_id, index })
        }
        other => other,
    }
}

fn with_brand(ty: Type, brand: Brand) -> Type {
    match ty {
        Type::Enum { type_id, .. } => Type::Enum { type_id, brand },
        Type::Struct { type_id, .. } => Type::Struct { type_id, brand },
        Type::Interface { type_id, .. } => Type::Interface { type_id, brand },
        other => other,
    }
}

fn replace_implicit_brand(brand: Brand, scope_id: NodeId) -> Brand {
    Brand {
        scopes: brand
            .scopes
            .into_iter()
            .map(|scope| BrandScope {
                scope_id: scope.scope_id,
                binding: match scope.binding {
                    ScopeBinding::Inherit => ScopeBinding::Inherit,
                    ScopeBinding::Bind(bindings) => ScopeBinding::Bind(
                        bindings
                            .into_iter()
                            .map(|binding| match binding {
                                BrandBinding::Unbound => BrandBinding::Unbound,
                                BrandBinding::Type(ty) => {
                                    BrandBinding::Type(replace_implicit_scope(ty, scope_id))
                                }
                            })
                            .collect(),
                    ),
                },
            })
            .collect(),
    }
}

fn zero_value(ty: &Type) -> Value {
    match ty {
        Type::Void => Value::Void,
        Type::Bool => Value::Bool(false),
        Type::Int8 => Value::Int8(0),
        Type::Int16 => Value::Int16(0),
        Type::Int32 => Value::Int32(0),
        Type::Int64 => Value::Int64(0),
        Type::UInt8 => Value::UInt8(0),
        Type::UInt16 => Value::UInt16(0),
        Type::UInt32 => Value::UInt32(0),
        Type::UInt64 => Value::UInt64(0),
        Type::Float32 => Value::Float32(0.0),
        Type::Float64 => Value::Float64(0.0),
        Type::Text => Value::Text(String::new()),
        Type::Data => Value::Data(Vec::new()),
        Type::List(_) => Value::List(capnp_schema::OpaquePointer::null()),
        Type::Enum { .. } => Value::Enum(0),
        Type::Struct { .. } => Value::Struct(capnp_schema::OpaquePointer::null()),
        Type::Interface { .. } => Value::Interface,
        Type::AnyPointer(_) => Value::AnyPointer(capnp_schema::OpaquePointer::null()),
    }
}

fn signed<T>(negative: bool, magnitude: u64) -> Result<T, RequestError>
where
    T: TryFrom<i128>,
{
    let value = if negative {
        -i128::from(magnitude)
    } else {
        i128::from(magnitude)
    };
    T::try_from(value).map_err(|_| RequestError::Unsupported("integer range"))
}

pub fn bootstrap_schema() -> Result<CompiledSchema, RequestError> {
    CompiledSchema::from_code_generator_request(BOOTSTRAP_REQUEST, LoadLimits::default())
        .map_err(RequestError::Bootstrap)
}

fn bootstrap_type(schema: &CompiledSchema, name: &'static str) -> Result<NodeId, RequestError> {
    schema
        .nodes()
        .iter()
        .find(|node| node.short_name() == Some(name))
        .map(|node| node.id)
        .ok_or(RequestError::MissingBootstrapType(name))
}

pub fn emit_empty_request(version: (u16, u8, u8)) -> Result<Vec<u8>, RequestError> {
    let schema = bootstrap_schema()?;
    let request_id = bootstrap_type(&schema, "CodeGeneratorRequest")?;
    let mut arena = ExclusiveArena::new(1024, 1 << 20)?;
    {
        let mut root = DynamicStructBuilder::root(&schema, &mut arena, request_id)?;
        root.init_list("nodes", 0)?;
        root.init_list("requestedFiles", 0)?;
        root.init_list("sourceInfo", 0)?;
        let mut capnp_version = root.init_struct("capnpVersion")?;
        capnp_version.set("major", DynamicInput::UInt16(version.0))?;
        capnp_version.set("minor", DynamicInput::UInt8(version.1))?;
        capnp_version.set("micro", DynamicInput::UInt8(version.2))?;
    }
    let segments = arena.into_segments();
    let borrowed = segments.iter().map(AsRef::as_ref).collect::<Vec<_>>();
    Ok(encode_frame(&borrowed, FrameLimits::default())?)
}

pub fn emit_compiled_schema(schema: &CompiledSchema) -> Result<Vec<u8>, RequestError> {
    let bootstrap = bootstrap_schema()?;
    let request_id = bootstrap_type(&bootstrap, "CodeGeneratorRequest")?;
    let mut arena = ExclusiveArena::new(4096, 1 << 24)?;
    {
        let mut root = DynamicStructBuilder::root(&bootstrap, &mut arena, request_id)?;
        {
            let mut nodes = root.init_list("nodes", count(schema.nodes().len())?)?;
            for (index, node) in schema.nodes().iter().enumerate() {
                write_node(&mut nodes.struct_element(count(index)?)?, node)?;
            }
        }
        {
            let mut files =
                root.init_list("requestedFiles", count(schema.requested_files().len())?)?;
            for (index, file) in schema.requested_files().iter().enumerate() {
                let mut output = files.struct_element(count(index)?)?;
                output.set("id", DynamicInput::UInt64(file.id))?;
                output.set("filename", DynamicInput::Text(&file.filename))?;
                let mut imports = output.init_list("imports", count(file.imports.len())?)?;
                for (import_index, import) in file.imports.iter().enumerate() {
                    let mut value = imports.struct_element(count(import_index)?)?;
                    value.set("id", DynamicInput::UInt64(import.id))?;
                    value.set("name", DynamicInput::Text(&import.name))?;
                }
                let mut file_info = output.init_struct("fileSourceInfo")?;
                let mut identifiers =
                    file_info.init_list("identifiers", count(file.identifiers.len())?)?;
                for (identifier_index, identifier) in file.identifiers.iter().enumerate() {
                    let mut value = identifiers.struct_element(count(identifier_index)?)?;
                    value.set("startByte", DynamicInput::UInt32(identifier.start_byte))?;
                    value.set("endByte", DynamicInput::UInt32(identifier.end_byte))?;
                    match identifier.target {
                        IdentifierTarget::Type(id) => {
                            value.set("typeId", DynamicInput::UInt64(id))?
                        }
                        IdentifierTarget::Member {
                            parent_type_id,
                            ordinal,
                        } => {
                            let mut member = value.group("member")?;
                            member.set("parentTypeId", DynamicInput::UInt64(parent_type_id))?;
                            member.set("ordinal", DynamicInput::UInt16(ordinal))?;
                        }
                    }
                }
            }
        }
        {
            let mut source_info =
                root.init_list("sourceInfo", count(schema.source_infos().len())?)?;
            for (index, source) in schema.source_infos().iter().enumerate() {
                let mut output = source_info.struct_element(count(index)?)?;
                output.set("id", DynamicInput::UInt64(source.id))?;
                output.set("docComment", DynamicInput::Text(&source.doc_comment))?;
                output.set("startByte", DynamicInput::UInt32(source.start_byte))?;
                output.set("endByte", DynamicInput::UInt32(source.end_byte))?;
                let mut members = output.init_list("members", count(source.members.len())?)?;
                for (member_index, member) in source.members.iter().enumerate() {
                    let mut value = members.struct_element(count(member_index)?)?;
                    value.set("docComment", DynamicInput::Text(&member.doc_comment))?;
                    value.set("startByte", DynamicInput::UInt32(member.start_byte))?;
                    value.set("endByte", DynamicInput::UInt32(member.end_byte))?;
                }
            }
        }
        let mut version = root.init_struct("capnpVersion")?;
        version.set("major", DynamicInput::UInt16(schema.version.major))?;
        version.set("minor", DynamicInput::UInt8(schema.version.minor))?;
        version.set("micro", DynamicInput::UInt8(schema.version.micro))?;
    }
    let segments = arena.into_segments();
    let borrowed = segments.iter().map(AsRef::as_ref).collect::<Vec<_>>();
    Ok(encode_frame(&borrowed, FrameLimits::default())?)
}

fn count(value: usize) -> Result<u32, RequestError> {
    u32::try_from(value).map_err(|_| RequestError::CountOverflow)
}

fn write_node(output: &mut DynamicStructBuilder<'_, '_>, node: &Node) -> Result<(), RequestError> {
    output.set("id", DynamicInput::UInt64(node.id))?;
    output.set("displayName", DynamicInput::Text(&node.display_name))?;
    output.set(
        "displayNamePrefixLength",
        DynamicInput::UInt32(node.display_name_prefix_length),
    )?;
    output.set("scopeId", DynamicInput::UInt64(node.scope_id))?;
    output.set("isGeneric", DynamicInput::Bool(node.is_generic))?;
    output.set("startByte", DynamicInput::UInt32(node.start_byte))?;
    output.set("endByte", DynamicInput::UInt32(node.end_byte))?;
    {
        let mut values = output.init_list("parameters", count(node.parameters.len())?)?;
        for (index, parameter) in node.parameters.iter().enumerate() {
            values
                .struct_element(count(index)?)?
                .set("name", DynamicInput::Text(&parameter.name))?;
        }
    }
    {
        let mut values = output.init_list("nestedNodes", count(node.nested_nodes.len())?)?;
        for (index, nested) in node.nested_nodes.iter().enumerate() {
            let mut value = values.struct_element(count(index)?)?;
            value.set("name", DynamicInput::Text(&nested.name))?;
            value.set("id", DynamicInput::UInt64(nested.id))?;
        }
    }
    write_annotations(output, "annotations", &node.annotations)?;
    match &node.kind {
        NodeKind::File => output.set("file", DynamicInput::Void)?,
        NodeKind::Struct(value) => write_struct(&mut output.group("struct")?, value)?,
        NodeKind::Enum(value) => {
            let mut group = output.group("enum")?;
            let mut enumerants = group.init_list("enumerants", count(value.enumerants.len())?)?;
            for (index, enumerant) in value.enumerants.iter().enumerate() {
                let mut item = enumerants.struct_element(count(index)?)?;
                item.set("name", DynamicInput::Text(&enumerant.name))?;
                item.set("codeOrder", DynamicInput::UInt16(enumerant.code_order))?;
                write_annotations(&mut item, "annotations", &enumerant.annotations)?;
            }
        }
        NodeKind::Const(value) => {
            let mut group = output.group("const")?;
            write_type(&mut group.init_struct("type")?, &value.ty)?;
            write_value(&mut group.init_struct("value")?, &value.value)?;
        }
        NodeKind::Annotation(value) => {
            let mut group = output.group("annotation")?;
            write_type(&mut group.init_struct("type")?, &value.ty)?;
            write_annotation_targets(&mut group, value.targets)?;
        }
        NodeKind::Interface(value) => {
            let mut group = output.group("interface")?;
            {
                let mut methods = group.init_list("methods", count(value.methods.len())?)?;
                for (index, method) in value.methods.iter().enumerate() {
                    let mut item = methods.struct_element(count(index)?)?;
                    item.set("name", DynamicInput::Text(&method.name))?;
                    item.set("codeOrder", DynamicInput::UInt16(method.code_order))?;
                    item.set(
                        "paramStructType",
                        DynamicInput::UInt64(method.param_struct_type),
                    )?;
                    item.set(
                        "resultStructType",
                        DynamicInput::UInt64(method.result_struct_type),
                    )?;
                    write_brand(&mut item.init_struct("paramBrand")?, &method.param_brand)?;
                    write_brand(&mut item.init_struct("resultBrand")?, &method.result_brand)?;
                    write_annotations(&mut item, "annotations", &method.annotations)?;
                    let mut parameters = item.init_list(
                        "implicitParameters",
                        count(method.implicit_parameters.len())?,
                    )?;
                    for (parameter_index, parameter) in
                        method.implicit_parameters.iter().enumerate()
                    {
                        parameters
                            .struct_element(count(parameter_index)?)?
                            .set("name", DynamicInput::Text(&parameter.name))?;
                    }
                }
            }
            let mut superclasses =
                group.init_list("superclasses", count(value.superclasses.len())?)?;
            for (index, superclass) in value.superclasses.iter().enumerate() {
                let mut item = superclasses.struct_element(count(index)?)?;
                item.set("id", DynamicInput::UInt64(superclass.id))?;
                write_brand(&mut item.init_struct("brand")?, &superclass.brand)?;
            }
        }
    }
    Ok(())
}

fn write_struct(
    output: &mut DynamicStructBuilder<'_, '_>,
    structure: &capnp_schema::StructSchema,
) -> Result<(), RequestError> {
    output.set(
        "dataWordCount",
        DynamicInput::UInt16(structure.data_word_count),
    )?;
    output.set(
        "pointerCount",
        DynamicInput::UInt16(structure.pointer_count),
    )?;
    output.set("preferredListEncoding", DynamicInput::Enum(7))?;
    output.set("isGroup", DynamicInput::Bool(structure.is_group))?;
    output.set(
        "discriminantCount",
        DynamicInput::UInt16(structure.discriminant_count),
    )?;
    output.set(
        "discriminantOffset",
        DynamicInput::UInt32(structure.discriminant_offset),
    )?;
    let mut fields = output.init_list("fields", count(structure.fields.len())?)?;
    for (index, field) in structure.fields.iter().enumerate() {
        let mut item = fields.struct_element(count(index)?)?;
        item.set("name", DynamicInput::Text(&field.name))?;
        item.set("codeOrder", DynamicInput::UInt16(field.code_order))?;
        item.set(
            "discriminantValue",
            DynamicInput::UInt16(field.discriminant_value.unwrap_or(u16::MAX)),
        )?;
        write_annotations(&mut item, "annotations", &field.annotations)?;
        match &field.kind {
            FieldKind::Slot {
                offset,
                ty,
                default_value,
                had_explicit_default,
            } => {
                let mut slot = item.group("slot")?;
                slot.set("offset", DynamicInput::UInt32(*offset))?;
                slot.set(
                    "hadExplicitDefault",
                    DynamicInput::Bool(*had_explicit_default),
                )?;
                write_type(&mut slot.init_struct("type")?, ty)?;
                write_value(&mut slot.init_struct("defaultValue")?, default_value)?;
            }
            FieldKind::Group { type_id } => {
                item.group("group")?
                    .set("typeId", DynamicInput::UInt64(*type_id))?;
            }
        }
        let mut ordinal = item.group("ordinal")?;
        match field.ordinal {
            Ordinal::Implicit => ordinal.set("implicit", DynamicInput::Void)?,
            Ordinal::Explicit(value) => ordinal.set("explicit", DynamicInput::UInt16(value))?,
        }
    }
    Ok(())
}

fn write_annotations(
    output: &mut DynamicStructBuilder<'_, '_>,
    field: &str,
    annotations: &[Annotation],
) -> Result<(), RequestError> {
    let mut values = output.init_list(field, count(annotations.len())?)?;
    for (index, annotation) in annotations.iter().enumerate() {
        let mut value = values.struct_element(count(index)?)?;
        value.set("id", DynamicInput::UInt64(annotation.id))?;
        write_brand(&mut value.init_struct("brand")?, &annotation.brand)?;
        write_value(&mut value.init_struct("value")?, &annotation.value)?;
    }
    Ok(())
}

fn write_annotation_targets(
    output: &mut DynamicStructBuilder<'_, '_>,
    targets: AnnotationTargets,
) -> Result<(), RequestError> {
    for (name, enabled) in [
        ("targetsFile", targets.file),
        ("targetsConst", targets.constant),
        ("targetsEnum", targets.enumeration),
        ("targetsEnumerant", targets.enumerant),
        ("targetsStruct", targets.structure),
        ("targetsField", targets.field),
        ("targetsUnion", targets.union),
        ("targetsGroup", targets.group),
        ("targetsInterface", targets.interface),
        ("targetsMethod", targets.method),
        ("targetsParam", targets.parameter),
        ("targetsAnnotation", targets.annotation),
    ] {
        output.set(name, DynamicInput::Bool(enabled))?;
    }
    Ok(())
}

fn write_type(output: &mut DynamicStructBuilder<'_, '_>, ty: &Type) -> Result<(), RequestError> {
    match ty {
        Type::Void => output.set("void", DynamicInput::Void)?,
        Type::Bool => output.set("bool", DynamicInput::Void)?,
        Type::Int8 => output.set("int8", DynamicInput::Void)?,
        Type::Int16 => output.set("int16", DynamicInput::Void)?,
        Type::Int32 => output.set("int32", DynamicInput::Void)?,
        Type::Int64 => output.set("int64", DynamicInput::Void)?,
        Type::UInt8 => output.set("uint8", DynamicInput::Void)?,
        Type::UInt16 => output.set("uint16", DynamicInput::Void)?,
        Type::UInt32 => output.set("uint32", DynamicInput::Void)?,
        Type::UInt64 => output.set("uint64", DynamicInput::Void)?,
        Type::Float32 => output.set("float32", DynamicInput::Void)?,
        Type::Float64 => output.set("float64", DynamicInput::Void)?,
        Type::Text => output.set("text", DynamicInput::Void)?,
        Type::Data => output.set("data", DynamicInput::Void)?,
        Type::List(element) => {
            write_type(
                &mut output.group("list")?.init_struct("elementType")?,
                element,
            )?;
        }
        Type::Enum { type_id, brand } => {
            let mut group = output.group("enum")?;
            group.set("typeId", DynamicInput::UInt64(*type_id))?;
            write_brand(&mut group.init_struct("brand")?, brand)?;
        }
        Type::Struct { type_id, brand } => {
            let mut group = output.group("struct")?;
            group.set("typeId", DynamicInput::UInt64(*type_id))?;
            write_brand(&mut group.init_struct("brand")?, brand)?;
        }
        Type::Interface { type_id, brand } => {
            let mut group = output.group("interface")?;
            group.set("typeId", DynamicInput::UInt64(*type_id))?;
            write_brand(&mut group.init_struct("brand")?, brand)?;
        }
        Type::AnyPointer(value) => {
            let mut any = output.group("anyPointer")?;
            match value {
                capnp_schema::AnyPointerType::Unconstrained(kind) => {
                    let mut group = any.group("unconstrained")?;
                    let name = match kind {
                        capnp_schema::AnyPointerKind::Any => "anyKind",
                        capnp_schema::AnyPointerKind::Struct => "struct",
                        capnp_schema::AnyPointerKind::List => "list",
                        capnp_schema::AnyPointerKind::Capability => "capability",
                    };
                    group.set(name, DynamicInput::Void)?;
                }
                capnp_schema::AnyPointerType::Parameter { scope_id, index } => {
                    let mut group = any.group("parameter")?;
                    group.set("scopeId", DynamicInput::UInt64(*scope_id))?;
                    group.set("parameterIndex", DynamicInput::UInt16(*index))?;
                }
                capnp_schema::AnyPointerType::ImplicitMethodParameter { index } => {
                    any.group("implicitMethodParameter")?
                        .set("parameterIndex", DynamicInput::UInt16(*index))?;
                }
            }
        }
    }
    Ok(())
}

fn write_brand(
    output: &mut DynamicStructBuilder<'_, '_>,
    brand: &Brand,
) -> Result<(), RequestError> {
    let mut scopes = output.init_list("scopes", count(brand.scopes.len())?)?;
    for (index, scope) in brand.scopes.iter().enumerate() {
        let mut item = scopes.struct_element(count(index)?)?;
        item.set("scopeId", DynamicInput::UInt64(scope.scope_id))?;
        match &scope.binding {
            ScopeBinding::Inherit => item.set("inherit", DynamicInput::Void)?,
            ScopeBinding::Bind(bindings) => {
                let mut values = item.init_list("bind", count(bindings.len())?)?;
                for (binding_index, binding) in bindings.iter().enumerate() {
                    let mut value = values.struct_element(count(binding_index)?)?;
                    match binding {
                        BrandBinding::Unbound => value.set("unbound", DynamicInput::Void)?,
                        BrandBinding::Type(ty) => write_type(&mut value.init_struct("type")?, ty)?,
                    }
                }
            }
        }
    }
    Ok(())
}

fn write_value(
    output: &mut DynamicStructBuilder<'_, '_>,
    value: &Value,
) -> Result<(), RequestError> {
    match value {
        Value::Void => output.set("void", DynamicInput::Void)?,
        Value::Bool(value) => output.set("bool", DynamicInput::Bool(*value))?,
        Value::Int8(value) => output.set("int8", DynamicInput::Int8(*value))?,
        Value::Int16(value) => output.set("int16", DynamicInput::Int16(*value))?,
        Value::Int32(value) => output.set("int32", DynamicInput::Int32(*value))?,
        Value::Int64(value) => output.set("int64", DynamicInput::Int64(*value))?,
        Value::UInt8(value) => output.set("uint8", DynamicInput::UInt8(*value))?,
        Value::UInt16(value) => output.set("uint16", DynamicInput::UInt16(*value))?,
        Value::UInt32(value) => output.set("uint32", DynamicInput::UInt32(*value))?,
        Value::UInt64(value) => output.set("uint64", DynamicInput::UInt64(*value))?,
        Value::Float32(value) => output.set("float32", DynamicInput::Float32(*value))?,
        Value::Float64(value) => output.set("float64", DynamicInput::Float64(*value))?,
        Value::Text(value) => output.set("text", DynamicInput::Text(value))?,
        Value::Data(value) => output.set("data", DynamicInput::Data(value))?,
        Value::Enum(value) => output.set("enum", DynamicInput::UInt16(*value))?,
        Value::Interface => output.set("interface", DynamicInput::Void)?,
        Value::List(value) => copy_opaque(output, "list", value)?,
        Value::Struct(value) => copy_opaque(output, "struct", value)?,
        Value::AnyPointer(value) => copy_opaque(output, "anyPointer", value)?,
    }
    Ok(())
}

fn copy_opaque(
    output: &mut DynamicStructBuilder<'_, '_>,
    field: &str,
    value: &capnp_schema::OpaquePointer,
) -> Result<(), RequestError> {
    output.set(field, DynamicInput::Pointer(value))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::semantic::{ModuleSources, ResolveLimits};

    macro_rules! request {
        ($name:literal) => {
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../conformance/fixtures/cpp/",
                "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
                "compiler-request-",
                $name,
                ".bin"
            ))
        };
    }
    const EVOLUTION: &[u8] = request!("evolution-v2");

    fn compile_source(name: &str, source: &str, oracle: &CompiledSchema) -> CompiledSchema {
        let path = format!("/{name}.capnp");
        let mut sources = ModuleSources::default();
        sources.insert_explicit(&path, source);
        let program = sources.resolve(&path, ResolveLimits::default());
        assert!(program.is_valid(), "{program:#?}");
        compile_program(&program, oracle.version).expect("native model compiles")
    }

    fn assert_struct_semantics(name: &str, left: &StructSchema, right: &StructSchema) {
        assert_eq!(left.data_word_count, right.data_word_count, "{name}");
        assert_eq!(left.pointer_count, right.pointer_count, "{name}");
        assert_eq!(
            left.preferred_list_encoding, right.preferred_list_encoding,
            "{name}"
        );
        assert_eq!(left.is_group, right.is_group, "{name}");
        assert_eq!(left.discriminant_count, right.discriminant_count, "{name}");
        assert_eq!(
            left.discriminant_offset, right.discriminant_offset,
            "{name}"
        );
        assert_eq!(left.fields.len(), right.fields.len(), "{name}");
        for (left, right) in left.fields.iter().zip(&right.fields) {
            assert_eq!(left.name, right.name);
            assert_eq!(left.code_order, right.code_order, "{}", left.name);
            assert_eq!(left.annotations, right.annotations, "{}", left.name);
            assert_eq!(
                left.discriminant_value, right.discriminant_value,
                "{}",
                left.name
            );
            assert_eq!(left.ordinal, right.ordinal, "{}", left.name);
            match (&left.kind, &right.kind) {
                (
                    FieldKind::Slot {
                        offset: lo,
                        ty: lt,
                        default_value: lv,
                        had_explicit_default: le,
                    },
                    FieldKind::Slot {
                        offset: ro,
                        ty: rt,
                        default_value: rv,
                        had_explicit_default: re,
                    },
                ) => {
                    assert_eq!(lo, ro, "{}", left.name);
                    assert_eq!(lt, rt, "{}", left.name);
                    assert_eq!(le, re, "{}", left.name);
                    assert_eq!(std::mem::discriminant(lv), std::mem::discriminant(rv));
                }
                (FieldKind::Group { type_id: left }, FieldKind::Group { type_id: right }) => {
                    assert_eq!(left, right)
                }
                _ => panic!("field kind mismatch for {}", left.name),
            }
        }
    }

    fn assert_node_semantics(native: &CompiledSchema, oracle: &CompiledSchema) {
        for node in native.nodes() {
            let expected = oracle.node(node.id).unwrap_or_else(|| {
                panic!(
                    "native node {} {:#x} missing upstream",
                    node.display_name, node.id
                )
            });
            assert_eq!(node.display_name, expected.display_name, "{:#x}", node.id);
            assert_eq!(
                node.display_name_prefix_length, expected.display_name_prefix_length,
                "{}",
                node.display_name
            );
            assert_eq!(node.scope_id, expected.scope_id, "{}", node.display_name);
            assert_eq!(
                node.parameters, expected.parameters,
                "{}",
                node.display_name
            );
            assert_eq!(
                node.is_generic, expected.is_generic,
                "{}",
                node.display_name
            );
            assert_eq!(
                node.nested_nodes, expected.nested_nodes,
                "{}",
                node.display_name
            );
            assert_eq!(
                node.annotations, expected.annotations,
                "{}",
                node.display_name
            );
            match (&node.kind, &expected.kind) {
                (NodeKind::File, NodeKind::File) => {}
                (NodeKind::Enum(left), NodeKind::Enum(right)) => assert_eq!(left, right),
                (NodeKind::Struct(left), NodeKind::Struct(right)) => {
                    assert_struct_semantics(&node.display_name, left, right)
                }
                (NodeKind::Interface(left), NodeKind::Interface(right)) => {
                    assert_eq!(left, right)
                }
                (NodeKind::Const(left), NodeKind::Const(right)) => {
                    assert_eq!(left.ty, right.ty);
                    assert_eq!(
                        std::mem::discriminant(&left.value),
                        std::mem::discriminant(&right.value)
                    );
                }
                (NodeKind::Annotation(left), NodeKind::Annotation(right)) => {
                    assert_eq!(left, right)
                }
                _ => panic!("node kind mismatch for {}", node.display_name),
            }
        }
        assert_eq!(native.nodes().len(), oracle.nodes().len());
    }

    fn assert_requested_semantics(native: &CompiledSchema, oracle: &CompiledSchema) {
        assert_eq!(
            native.requested_files().len(),
            oracle.requested_files().len()
        );
        for (left, right) in native
            .requested_files()
            .iter()
            .zip(oracle.requested_files())
        {
            assert_eq!(left.id, right.id);
            assert_eq!(left.filename, right.filename);
            assert_eq!(left.imports, right.imports);
            let mut left_identifiers = left.identifiers.clone();
            let mut right_identifiers = right.identifiers.clone();
            left_identifiers.sort_by_key(|value| (value.start_byte, value.end_byte));
            right_identifiers.sort_by_key(|value| (value.start_byte, value.end_byte));
            let missing = right_identifiers
                .iter()
                .filter(|value| !left_identifiers.contains(value))
                .collect::<Vec<_>>();
            let extra = left_identifiers
                .iter()
                .filter(|value| !right_identifiers.contains(value))
                .collect::<Vec<_>>();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "identifier mismatch for {}: missing={missing:?}, extra={extra:?}",
                left.filename
            );
        }
    }

    #[test]
    fn evolution_source_compiles_to_oracle_semantics() {
        let oracle = CompiledSchema::from_code_generator_request(EVOLUTION, LoadLimits::default())
            .expect("oracle request loads");
        let native = compile_source(
            "evolution-v2",
            include_str!("../../../conformance/schemas/evolution-v2.capnp"),
            &oracle,
        );
        assert_eq!(native.version, oracle.version);
        assert_eq!(native.requested_files(), oracle.requested_files());
        assert_eq!(native.nodes().len(), oracle.nodes().len());
        for node in native.nodes() {
            let expected = oracle
                .node(node.id)
                .expect("native node ID exists upstream");
            assert_eq!(
                node.display_name, expected.display_name,
                "node {:#x}",
                node.id
            );
            assert_eq!(
                node.display_name_prefix_length, expected.display_name_prefix_length,
                "{}",
                node.display_name
            );
            assert_eq!(node.scope_id, expected.scope_id);
            assert_eq!(node.parameters, expected.parameters);
            assert_eq!(node.is_generic, expected.is_generic);
            assert_eq!(node.nested_nodes, expected.nested_nodes);
            assert_eq!(node.annotations, expected.annotations);
            match (&node.kind, &expected.kind) {
                (NodeKind::File, NodeKind::File) => {}
                (NodeKind::Enum(left), NodeKind::Enum(right)) => assert_eq!(left, right),
                (NodeKind::Struct(left), NodeKind::Struct(right)) => {
                    assert_eq!(left.data_word_count, right.data_word_count);
                    assert_eq!(left.pointer_count, right.pointer_count);
                    assert_eq!(left.preferred_list_encoding, right.preferred_list_encoding);
                    assert_eq!(left.is_group, right.is_group);
                    assert_eq!(left.discriminant_count, right.discriminant_count);
                    assert_eq!(left.discriminant_offset, right.discriminant_offset);
                    assert_eq!(left.fields.len(), right.fields.len());
                    for (left, right) in left.fields.iter().zip(&right.fields) {
                        assert_eq!(left.name, right.name);
                        assert_eq!(left.code_order, right.code_order);
                        assert_eq!(left.annotations, right.annotations);
                        assert_eq!(left.discriminant_value, right.discriminant_value);
                        assert_eq!(left.ordinal, right.ordinal);
                        match (&left.kind, &right.kind) {
                            (
                                FieldKind::Slot {
                                    offset: lo,
                                    ty: lt,
                                    default_value: lv,
                                    had_explicit_default: le,
                                },
                                FieldKind::Slot {
                                    offset: ro,
                                    ty: rt,
                                    default_value: rv,
                                    had_explicit_default: re,
                                },
                            ) => {
                                assert_eq!(lo, ro, "{}", left.name);
                                assert_eq!(lt, rt, "{}", left.name);
                                assert_eq!(le, re, "{}", left.name);
                                assert_eq!(std::mem::discriminant(lv), std::mem::discriminant(rv));
                            }
                            (
                                FieldKind::Group { type_id: left },
                                FieldKind::Group { type_id: right },
                            ) => assert_eq!(left, right),
                            _ => panic!("field kind mismatch for {}", left.name),
                        }
                    }
                }
                _ => panic!("node kind mismatch for {}", node.display_name),
            }
        }
    }

    #[test]
    fn wire_source_compiles_interfaces_and_all_field_types() {
        let oracle = CompiledSchema::from_code_generator_request(
            request!("wire-fixture"),
            LoadLimits::default(),
        )
        .expect("oracle request loads");
        let native = compile_source(
            "wire-fixture",
            include_str!("../../../conformance/schemas/wire-fixture.capnp"),
            &oracle,
        );
        assert_requested_semantics(&native, &oracle);
        assert_node_semantics(&native, &oracle);
    }

    #[test]
    fn language_source_compiles_generics_constants_and_annotations() {
        let oracle = CompiledSchema::from_code_generator_request(
            request!("language-fixture"),
            LoadLimits::default(),
        )
        .expect("oracle request loads");
        let native = compile_source(
            "language-fixture",
            include_str!("../../../conformance/schemas/language-fixture.capnp"),
            &oracle,
        );
        assert_requested_semantics(&native, &oracle);
        assert_node_semantics(&native, &oracle);
        let native_generated =
            capnp_codegen::generate_requested_files(&native).expect("native model generates");
        let repeated =
            capnp_codegen::generate_requested_files(&native).expect("native model regenerates");
        assert_eq!(native_generated, repeated);
        let oracle_generated =
            capnp_codegen::generate_requested_files(&oracle).expect("oracle model generates");
        assert_eq!(native_generated, oracle_generated);
        let first_request = emit_compiled_schema(&native).expect("native request emits");
        let second_request = emit_compiled_schema(&native).expect("native request re-emits");
        assert_eq!(first_request, second_request);
        let reloaded =
            CompiledSchema::from_code_generator_request(&first_request, LoadLimits::default())
                .expect("native request reloads");
        assert_requested_semantics(&reloaded, &oracle);
        assert_node_semantics(&reloaded, &oracle);
        assert_eq!(
            capnp_codegen::generate_requested_files(&reloaded)
                .expect("reloaded native request generates"),
            oracle_generated
        );
    }

    #[test]
    fn imported_sources_compile_to_one_standard_request() {
        let oracle = CompiledSchema::from_code_generator_request(
            request!("import-fixture"),
            LoadLimits::default(),
        )
        .expect("oracle request loads");
        let mut sources = ModuleSources::default();
        sources.insert_explicit(
            "/import-fixture.capnp",
            include_str!("../../../conformance/schemas/import-fixture.capnp"),
        );
        sources.insert_explicit(
            "/wire-fixture.capnp",
            include_str!("../../../conformance/schemas/wire-fixture.capnp"),
        );
        sources.insert_explicit(
            "/language-fixture.capnp",
            include_str!("../../../conformance/schemas/language-fixture.capnp"),
        );
        let program = sources.resolve("/import-fixture.capnp", ResolveLimits::default());
        assert!(program.is_valid(), "{program:#?}");
        let native = compile_program(&program, oracle.version).expect("native model compiles");
        assert_requested_semantics(&native, &oracle);
        assert_node_semantics(&native, &oracle);
    }

    #[test]
    fn streaming_source_compiles_to_standard_method_metadata() {
        let oracle = CompiledSchema::from_code_generator_request(
            request!("streaming-fixture"),
            LoadLimits::default(),
        )
        .expect("oracle request loads");
        let native = compile_source(
            "streaming-fixture",
            include_str!("../../../conformance/schemas/streaming-fixture.capnp"),
            &oracle,
        );
        assert_requested_semantics(&native, &oracle);
        assert_node_semantics(&native, &oracle);
    }

    #[test]
    fn schema_source_bootstraps_its_own_standard_request() {
        let oracle =
            CompiledSchema::from_code_generator_request(request!("schema"), LoadLimits::default())
                .expect("schema oracle request loads");
        let entry = format!("/{}", oracle.requested_files()[0].filename);
        let mut sources = ModuleSources::default();
        sources.insert_explicit(
            &entry,
            include_str!(concat!(
                "../../../conformance/upstream/capnproto/",
                "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/schema.capnp"
            )),
        );
        let program = sources.resolve(&entry, ResolveLimits::default());
        assert!(program.is_valid(), "{program:#?}");
        let native = compile_program(&program, oracle.version).expect("native schema compiles");
        assert_requested_semantics(&native, &oracle);
        assert_node_semantics(&native, &oracle);
        let request = emit_compiled_schema(&native).expect("self-hosted request emits");
        let reloaded = CompiledSchema::from_code_generator_request(&request, LoadLimits::default())
            .expect("self-hosted request reloads");
        assert_requested_semantics(&reloaded, &oracle);
        assert_node_semantics(&reloaded, &oracle);
        let native_generated = capnp_codegen::generate_requested_files(&native)
            .expect("native schema request generates");
        let oracle_generated = capnp_codegen::generate_requested_files(&oracle)
            .expect("oracle schema request generates");
        assert_eq!(native_generated.len(), oracle_generated.len());
        for (left, right) in native_generated.iter().zip(&oracle_generated) {
            assert_eq!(left.module_name, right.module_name);
            if left.source != right.source {
                let mismatch = left
                    .source
                    .lines()
                    .zip(right.source.lines())
                    .enumerate()
                    .find(|(_, (left, right))| left != right);
                let line = mismatch.as_ref().map_or(0, |(line, _)| *line);
                let native_context = left.source.lines().skip(line).take(12).collect::<Vec<_>>();
                let oracle_context = right.source.lines().skip(line).take(12).collect::<Vec<_>>();
                panic!(
                    "generated {} differs: first mismatch={mismatch:?}, native bytes={}, oracle bytes={}, native={native_context:?}, oracle={oracle_context:?}",
                    left.module_name,
                    left.source.len(),
                    right.source.len()
                );
            }
        }
    }

    #[test]
    fn checked_in_schema_request_bootstraps_without_a_system_compiler() {
        let schema = bootstrap_schema().expect("bootstrap request loads");
        assert!(
            schema
                .nodes()
                .iter()
                .any(|node| node.short_name() == Some("CodeGeneratorRequest"))
        );

        let bytes = emit_empty_request((2, 0, 0)).expect("standard request emits");
        let empty = CompiledSchema::from_code_generator_request(&bytes, LoadLimits::default())
            .expect("native request is standard and loadable");
        assert!(empty.nodes().is_empty());
        assert!(empty.requested_files().is_empty());
        assert_eq!(empty.version.major, 2);
    }

    #[test]
    fn evolution_request_round_trips_semantic_nodes_natively() {
        let oracle = CompiledSchema::from_code_generator_request(EVOLUTION, LoadLimits::default())
            .expect("oracle request loads");
        let bytes = emit_compiled_schema(&oracle).expect("native request emits");
        let native = CompiledSchema::from_code_generator_request(&bytes, LoadLimits::default())
            .expect("native request reloads");
        assert_eq!(native.version, oracle.version);
        assert_eq!(native.nodes().len(), oracle.nodes().len());
        assert_eq!(native.source_infos(), oracle.source_infos());
        assert_eq!(
            native.requested_files().len(),
            oracle.requested_files().len()
        );
        for (left, right) in native.nodes().iter().zip(oracle.nodes()) {
            assert_eq!(left.id, right.id);
            assert_eq!(left.display_name, right.display_name);
            assert_eq!(
                left.display_name_prefix_length,
                right.display_name_prefix_length
            );
            assert_eq!(left.scope_id, right.scope_id);
            assert_eq!(left.parameters, right.parameters);
            assert_eq!(left.nested_nodes, right.nested_nodes);
            match (&left.kind, &right.kind) {
                (NodeKind::File, NodeKind::File) => {}
                (NodeKind::Enum(left), NodeKind::Enum(right)) => {
                    assert_eq!(left, right);
                }
                (NodeKind::Struct(left), NodeKind::Struct(right)) => {
                    assert_eq!(left.data_word_count, right.data_word_count);
                    assert_eq!(left.pointer_count, right.pointer_count);
                    assert_eq!(left.discriminant_count, right.discriminant_count);
                    assert_eq!(left.discriminant_offset, right.discriminant_offset);
                    assert_eq!(left.fields.len(), right.fields.len());
                    for (left, right) in left.fields.iter().zip(&right.fields) {
                        assert_eq!(left.name, right.name);
                        assert_eq!(left.code_order, right.code_order);
                        assert_eq!(left.discriminant_value, right.discriminant_value);
                        assert_eq!(left.ordinal, right.ordinal);
                        match (&left.kind, &right.kind) {
                            (
                                FieldKind::Slot {
                                    offset: left_offset,
                                    ty: left_type,
                                    had_explicit_default: left_explicit,
                                    ..
                                },
                                FieldKind::Slot {
                                    offset: right_offset,
                                    ty: right_type,
                                    had_explicit_default: right_explicit,
                                    ..
                                },
                            ) => {
                                assert_eq!(left_offset, right_offset);
                                assert_eq!(left_type, right_type);
                                assert_eq!(left_explicit, right_explicit);
                            }
                            (
                                FieldKind::Group { type_id: left },
                                FieldKind::Group { type_id: right },
                            ) => {
                                assert_eq!(left, right);
                            }
                            _ => assert!(matches!(
                                (&left.kind, &right.kind),
                                (FieldKind::Slot { .. }, FieldKind::Slot { .. })
                                    | (FieldKind::Group { .. }, FieldKind::Group { .. })
                            )),
                        }
                    }
                }
                _ => assert!(matches!(
                    (&left.kind, &right.kind),
                    (NodeKind::File, NodeKind::File)
                        | (NodeKind::Enum(_), NodeKind::Enum(_))
                        | (NodeKind::Struct(_), NodeKind::Struct(_))
                )),
            }
        }
        let left = &native.requested_files()[0];
        let right = &oracle.requested_files()[0];
        assert_eq!(left.id, right.id);
        assert_eq!(left.filename, right.filename);
        assert_eq!(left.imports, right.imports);
    }

    #[test]
    fn every_pinned_request_serializes_and_reloads_without_capnp() {
        for (name, bytes) in [
            ("evolution-v1", request!("evolution-v1").as_slice()),
            ("evolution-v2", request!("evolution-v2").as_slice()),
            ("evolution-v3", request!("evolution-v3").as_slice()),
            ("imports", request!("import-fixture").as_slice()),
            ("language", request!("language-fixture").as_slice()),
            ("schema", request!("schema").as_slice()),
            ("streaming", request!("streaming-fixture").as_slice()),
            ("wire", request!("wire-fixture").as_slice()),
        ] {
            let oracle = CompiledSchema::from_code_generator_request(bytes, LoadLimits::default())
                .expect("pinned request loads");
            let emitted = emit_compiled_schema(&oracle).expect("native serializer accepts model");
            let native =
                CompiledSchema::from_code_generator_request(&emitted, LoadLimits::default())
                    .expect("native request reloads");
            assert_eq!(native.nodes().len(), oracle.nodes().len(), "{name}");
            assert_eq!(
                native.requested_files().len(),
                oracle.requested_files().len(),
                "{name}"
            );
            assert_eq!(
                native.source_infos().len(),
                oracle.source_infos().len(),
                "{name}"
            );
        }
    }
}
