//! Deterministic M24 struct layout over M23 resolved declarations.
//!
//! The allocator follows the pinned C++ compiler's ordinal-order allocation,
//! power-of-two padding reuse, union lane sharing, and 16-bit discriminants.
//! It does not emit a CodeGeneratorRequest; serialization belongs to M25.

use std::collections::{BTreeMap, BTreeSet};

use crate::SourceRange;
use crate::semantic::{
    DeclarationId, DeclarationKind, Expression, NameTarget, ResolvedDeclaration, ResolvedModule,
    ResolvedProgram, SemanticDiagnostic,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledLayouts {
    pub structs: Vec<CompiledStruct>,
    pub diagnostics: Vec<SemanticDiagnostic>,
}

impl CompiledLayouts {
    pub fn is_valid(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn structure(&self, module: &str, qualified_name: &str) -> Option<&CompiledStruct> {
        self.structs
            .iter()
            .find(|item| item.module == module && item.qualified_name == qualified_name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledStruct {
    pub module: String,
    pub qualified_name: String,
    pub id: Option<u64>,
    pub is_group: bool,
    pub data_word_count: u16,
    pub pointer_count: u16,
    pub preferred_list_encoding: PreferredListEncoding,
    pub discriminant_count: u16,
    pub discriminant_offset: Option<u32>,
    pub fields: Vec<CompiledField>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferredListEncoding {
    InlineComposite,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledField {
    pub name: String,
    pub qualified_name: String,
    pub code_order: u16,
    pub ordinal: Option<u16>,
    pub discriminant_value: Option<u16>,
    pub kind: CompiledFieldKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompiledFieldKind {
    Slot { offset: u32, size: SlotSize },
    Group { qualified_name: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotSize {
    Void,
    Data { log2_bits: u8 },
    Pointer,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Holes {
    offsets: [u32; 6],
}

impl Holes {
    fn allocate(&mut self, log2_bits: u8) -> Option<u32> {
        let index = usize::from(log2_bits);
        if index >= self.offsets.len() {
            return None;
        }
        if self.offsets[index] != 0 {
            let result = self.offsets[index];
            self.offsets[index] = 0;
            return Some(result);
        }
        let result = self.allocate(log2_bits + 1)? * 2;
        self.offsets[index] = result + 1;
        Some(result)
    }

    fn add_at_end(&mut self, mut log2_bits: u8, mut offset: u32, limit: u8) {
        while log2_bits < limit {
            self.offsets[usize::from(log2_bits)] = offset;
            log2_bits += 1;
            offset = offset.div_ceil(2);
        }
    }

    fn try_expand(&mut self, old_size: u8, old_offset: u32, factors: u8) -> bool {
        if factors == 0 {
            return true;
        }
        if old_size >= 6 || self.offsets[usize::from(old_size)] != old_offset + 1 {
            return false;
        }
        if self.try_expand(old_size + 1, old_offset >> 1, factors - 1) {
            self.offsets[usize::from(old_size)] = 0;
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
struct TopLayout {
    data_words: u32,
    pointers: u32,
    holes: Holes,
}

impl TopLayout {
    fn add_data(&mut self, log2_bits: u8) -> u32 {
        if let Some(offset) = self.holes.allocate(log2_bits) {
            offset
        } else {
            let offset = self.data_words << (6 - log2_bits);
            self.data_words += 1;
            self.holes.add_at_end(log2_bits, offset + 1, 6);
            offset
        }
    }

    fn add_pointer(&mut self) -> u32 {
        let result = self.pointers;
        self.pointers += 1;
        result
    }

    fn try_expand(&mut self, old_size: u8, old_offset: u32, new_size: u8) -> bool {
        new_size <= 6
            && old_offset & ((1u32 << (new_size - old_size)) - 1) == 0
            && self
                .holes
                .try_expand(old_size, old_offset, new_size - old_size)
    }
}

#[derive(Clone, Copy)]
struct DataLane {
    log2_bits: u8,
    offset: u32,
}

#[derive(Default)]
struct ArmUsage {
    data_masks: Vec<u64>,
    pointers: usize,
}

#[derive(Default)]
struct UnionLayout {
    lanes: Vec<DataLane>,
    pointer_lanes: Vec<u32>,
    arms: BTreeMap<String, ArmUsage>,
    started_arms: BTreeSet<String>,
    discriminant_offset: Option<u32>,
    discriminant_values: BTreeMap<String, u16>,
}

impl UnionLayout {
    fn add_discriminant(&mut self, top: &mut TopLayout) -> bool {
        if self.discriminant_offset.is_some() {
            false
        } else {
            self.discriminant_offset = Some(top.add_data(4));
            true
        }
    }

    fn max_data_size(&self) -> Option<u8> {
        self.lanes
            .iter()
            .map(|lane| lane.log2_bits)
            .chain(self.discriminant_offset.map(|_| 4))
            .max()
    }
}

fn begin_union_arm(
    unions: &mut BTreeMap<String, UnionLayout>,
    scopes: &[(String, String)],
    depth: usize,
    top: &mut TopLayout,
) {
    let (name, arm) = &scopes[depth];
    let needs_discriminant = {
        let union = unions.get_mut(name).expect("union scope exists");
        if !union.started_arms.insert(arm.clone()) {
            return;
        }
        let value = u16::try_from(union.started_arms.len() - 1).unwrap_or(u16::MAX);
        union.discriminant_values.insert(arm.clone(), value);
        union.started_arms.len() == 2 && union.discriminant_offset.is_none()
    };
    if needs_discriminant {
        let offset = if depth == 0 {
            top.add_data(4)
        } else {
            union_add_data(unions, scopes, depth - 1, 4, top)
        };
        unions
            .get_mut(name)
            .expect("union scope exists")
            .discriminant_offset = Some(offset);
    }
}

fn union_add_data(
    unions: &mut BTreeMap<String, UnionLayout>,
    scopes: &[(String, String)],
    depth: usize,
    log2_bits: u8,
    top: &mut TopLayout,
) -> u32 {
    begin_union_arm(unions, scopes, depth, top);
    let (name, arm) = &scopes[depth];
    let bits = 1u32 << log2_bits;
    {
        let union = unions.get_mut(name).expect("union scope exists");
        let usage = union.arms.entry(arm.clone()).or_default();
        usage.data_masks.resize(union.lanes.len(), 0);
        let mut best = None;
        for (index, lane) in union.lanes.iter().enumerate() {
            let lane_bits = 1u32 << lane.log2_bits;
            if lane_bits < bits {
                continue;
            }
            for local in (0..lane_bits).step_by(bits as usize) {
                let mask = bit_mask(bits, local);
                if usage.data_masks[index] & mask == 0 {
                    if depth == 0 {
                        usage.data_masks[index] |= mask;
                        return (lane.offset << (lane.log2_bits - log2_bits)) + local / bits;
                    }
                    let candidate = (lane.log2_bits, index, local, mask);
                    if best.is_none_or(|current| candidate < current) {
                        best = Some(candidate);
                    }
                }
            }
        }
        if let Some((_, index, local, mask)) = best {
            usage.data_masks[index] |= mask;
            let lane = union.lanes[index];
            return (lane.offset << (lane.log2_bits - log2_bits)) + local / bits;
        }
    }
    if depth == 0 {
        let union = unions.get_mut(name).expect("union scope exists");
        for index in 0..union.lanes.len() {
            let lane = union.lanes[index];
            let arm_already_uses_lane = union
                .arms
                .get(arm)
                .and_then(|usage| usage.data_masks.get(index))
                .is_some_and(|mask| *mask != 0);
            if !arm_already_uses_lane
                && lane.log2_bits < log2_bits
                && top.try_expand(lane.log2_bits, lane.offset, log2_bits)
            {
                union.lanes[index].offset >>= log2_bits - lane.log2_bits;
                union.lanes[index].log2_bits = log2_bits;
                for arm_usage in union.arms.values_mut() {
                    arm_usage.data_masks.resize(union.lanes.len(), 0);
                }
                union.arms.get_mut(arm).expect("arm exists").data_masks[index] = bit_mask(bits, 0);
                return union.lanes[index].offset;
            }
        }
    }
    let offset = if depth == 0 {
        top.add_data(log2_bits)
    } else {
        union_add_data(unions, scopes, depth - 1, log2_bits, top)
    };
    let union = unions.get_mut(name).expect("union scope exists");
    union.lanes.push(DataLane { log2_bits, offset });
    for arm_usage in union.arms.values_mut() {
        arm_usage.data_masks.resize(union.lanes.len(), 0);
    }
    union.arms.get_mut(arm).expect("arm exists").data_masks[union.lanes.len() - 1] =
        bit_mask(bits, 0);
    offset
}

fn union_add_pointer(
    unions: &mut BTreeMap<String, UnionLayout>,
    scopes: &[(String, String)],
    depth: usize,
    top: &mut TopLayout,
) -> u32 {
    begin_union_arm(unions, scopes, depth, top);
    let (name, arm) = &scopes[depth];
    let (index, existing) = {
        let union = unions.get_mut(name).expect("union scope exists");
        let usage = union.arms.entry(arm.clone()).or_default();
        let index = usage.pointers;
        usage.pointers += 1;
        (index, union.pointer_lanes.get(index).copied())
    };
    if let Some(offset) = existing {
        return offset;
    }
    let offset = if depth == 0 {
        top.add_pointer()
    } else {
        union_add_pointer(unions, scopes, depth - 1, top)
    };
    let union = unions.get_mut(name).expect("union scope exists");
    debug_assert_eq!(index, union.pointer_lanes.len());
    union.pointer_lanes.push(offset);
    offset
}

fn finish_union_padding(top: &mut TopLayout, max_size: Option<u8>, before: Holes) {
    let Some(max_size) = max_size else {
        return;
    };
    for size in 0..usize::from(max_size) {
        if top.holes.offsets[size] != before.offsets[size] {
            top.holes.offsets[size] = 0;
        }
    }
}

fn bit_mask(bits: u32, start: u32) -> u64 {
    if bits == 64 {
        u64::MAX
    } else {
        ((1u64 << bits) - 1) << start
    }
}

impl ResolvedProgram {
    pub fn compile_layouts(&self) -> CompiledLayouts {
        LayoutCompiler::new(self).compile()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompiledTupleLayout {
    pub data_word_count: u16,
    pub pointer_count: u16,
    pub fields: Vec<CompiledTupleField>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompiledTupleField {
    pub name: String,
    pub offset: u32,
    pub size: SlotSize,
    pub ty: Expression,
}

pub(crate) fn compile_tuple_layout(
    program: &ResolvedProgram,
    _module: &ResolvedModule,
    expression: &Expression,
) -> Option<CompiledTupleLayout> {
    let Expression::Tuple { values, .. } = expression else {
        return None;
    };
    let compiler = LayoutCompiler::new(program);
    let mut top = TopLayout::default();
    let mut fields = Vec::new();
    for (index, (name, ty)) in values.iter().enumerate() {
        let size = compiler.expression_size(ty, &mut BTreeSet::new())?;
        let offset = match size {
            SlotSize::Void => 0,
            SlotSize::Data { log2_bits } => top.add_data(log2_bits),
            SlotSize::Pointer => top.add_pointer(),
        };
        fields.push(CompiledTupleField {
            name: name.clone().unwrap_or_else(|| format!("arg{index}")),
            offset,
            size,
            ty: ty.clone(),
        });
    }
    Some(CompiledTupleLayout {
        data_word_count: u16::try_from(top.data_words).ok()?,
        pointer_count: u16::try_from(top.pointers).ok()?,
        fields,
    })
}

struct LayoutCompiler<'a> {
    program: &'a ResolvedProgram,
    diagnostics: Vec<SemanticDiagnostic>,
    output: Vec<CompiledStruct>,
}

#[derive(Clone, Copy)]
enum OrdinalMember<'a> {
    Field(&'a ResolvedDeclaration),
    Union(&'a ResolvedDeclaration),
}

impl<'a> LayoutCompiler<'a> {
    fn new(program: &'a ResolvedProgram) -> Self {
        Self {
            program,
            diagnostics: program.diagnostics.clone(),
            output: Vec::new(),
        }
    }

    fn compile(mut self) -> CompiledLayouts {
        for module in &self.program.modules {
            for declaration in &module.declarations {
                if declaration.kind == DeclarationKind::Struct {
                    self.compile_struct(module, declaration);
                }
            }
        }
        self.output.sort_by(|left, right| {
            (&left.module, &left.qualified_name).cmp(&(&right.module, &right.qualified_name))
        });
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
        CompiledLayouts {
            structs: self.output,
            diagnostics: self.diagnostics,
        }
    }

    fn compile_struct(&mut self, module: &ResolvedModule, root: &ResolvedDeclaration) {
        let descendants = module
            .declarations
            .iter()
            .filter(|item| belongs_to_struct_layout(module, item, root))
            .collect::<Vec<_>>();
        let mut ordinals = BTreeMap::new();
        for declaration in &descendants {
            match declaration.kind {
                DeclarationKind::Field => {
                    let Some(DeclarationId::Ordinal(ordinal)) = declaration.id else {
                        self.error(module, declaration.range, "field is missing an ordinal");
                        continue;
                    };
                    ordinals.insert(ordinal, OrdinalMember::Field(declaration));
                }
                DeclarationKind::Union => {
                    if let Some(DeclarationId::Ordinal(ordinal)) = declaration.id {
                        ordinals.insert(ordinal, OrdinalMember::Union(declaration));
                    }
                    let members = descendants
                        .iter()
                        .filter(|item| item.parent == Some(declaration.qualified_name.clone()))
                        .filter(|item| {
                            matches!(
                                item.kind,
                                DeclarationKind::Field
                                    | DeclarationKind::Group
                                    | DeclarationKind::Union
                            )
                        })
                        .count();
                    if members < 2 {
                        self.error(
                            module,
                            declaration.range,
                            "union must have at least two members",
                        );
                    }
                }
                DeclarationKind::Group => {
                    let members = descendants
                        .iter()
                        .filter(|item| item.parent == Some(declaration.qualified_name.clone()))
                        .filter(|item| {
                            matches!(
                                item.kind,
                                DeclarationKind::Field
                                    | DeclarationKind::Group
                                    | DeclarationKind::Union
                            )
                        })
                        .count();
                    if members == 0 {
                        self.error(
                            module,
                            declaration.range,
                            "group must have at least one member",
                        );
                    }
                }
                _ => {}
            }
        }
        for (expected, actual) in ordinals.keys().copied().enumerate() {
            if usize::from(actual) != expected {
                self.error(
                    module,
                    root.range,
                    "field ordinals must be sequential from zero",
                );
                break;
            }
        }

        let mut top = TopLayout::default();
        let mut unions = descendants
            .iter()
            .filter(|item| item.kind == DeclarationKind::Union)
            .map(|item| (item.qualified_name.clone(), UnionLayout::default()))
            .collect::<BTreeMap<_, _>>();
        let mut compiled_fields = BTreeMap::<String, CompiledField>::new();
        let mut active_union_region: Option<(Holes, BTreeSet<String>)> = None;
        for member in ordinals.values() {
            let OrdinalMember::Field(declaration) = member else {
                let OrdinalMember::Union(declaration) = member else {
                    unreachable!();
                };
                let union = unions
                    .get_mut(&declaration.qualified_name)
                    .expect("indexed union");
                if !union.add_discriminant(&mut top) {
                    self.error(
                        module,
                        declaration.range,
                        "union ordinal may follow at most one arm ordinal",
                    );
                }
                continue;
            };
            let Some(size) = self.slot_size(module, declaration, &mut BTreeSet::new()) else {
                continue;
            };
            let union_scopes = union_scopes(module, declaration, &root.qualified_name);
            if union_scopes.is_empty() {
                if let Some((before, names)) = active_union_region.take() {
                    let max_size = names
                        .iter()
                        .filter_map(|name| unions.get(name).and_then(UnionLayout::max_data_size))
                        .max();
                    finish_union_padding(&mut top, max_size, before);
                }
            } else {
                let (_, names) =
                    active_union_region.get_or_insert_with(|| (top.holes, BTreeSet::new()));
                names.extend(union_scopes.iter().map(|(name, _)| name.clone()));
            }
            for depth in 0..union_scopes.len() {
                begin_union_arm(&mut unions, &union_scopes, depth, &mut top);
            }
            let (offset, discriminant_value) = if let Some((union_name, arm)) = union_scopes.last()
            {
                let depth = union_scopes.len() - 1;
                let offset = match size {
                    SlotSize::Void => 0,
                    SlotSize::Data { log2_bits } => {
                        union_add_data(&mut unions, &union_scopes, depth, log2_bits, &mut top)
                    }
                    SlotSize::Pointer => {
                        union_add_pointer(&mut unions, &union_scopes, depth, &mut top)
                    }
                };
                let discriminant = (declaration.parent.as_deref() == Some(union_name.as_str()))
                    .then(|| {
                        unions
                            .get(union_name)
                            .and_then(|union| union.discriminant_values.get(arm).copied())
                    })
                    .flatten();
                (offset, discriminant)
            } else {
                let offset = match size {
                    SlotSize::Void => 0,
                    SlotSize::Data { log2_bits } => top.add_data(log2_bits),
                    SlotSize::Pointer => top.add_pointer(),
                };
                (offset, None)
            };
            compiled_fields.insert(
                declaration.qualified_name.clone(),
                CompiledField {
                    name: declaration.name.clone(),
                    qualified_name: declaration.qualified_name.clone(),
                    code_order: visible_code_order(module, declaration),
                    ordinal: declaration.id.and_then(|id| match id {
                        DeclarationId::Ordinal(value) => Some(value),
                        DeclarationId::Uid(_) => None,
                    }),
                    discriminant_value,
                    kind: CompiledFieldKind::Slot { offset, size },
                },
            );
        }
        if let Some((before, names)) = active_union_region.take() {
            let max_size = names
                .iter()
                .filter_map(|name| unions.get(name).and_then(UnionLayout::max_data_size))
                .max();
            finish_union_padding(&mut top, max_size, before);
        }

        if top.data_words > u32::from(u16::MAX) {
            self.error(
                module,
                root.range,
                "struct data section exceeds the schema limit",
            );
        }
        if top.pointers > u32::from(u16::MAX) {
            self.error(
                module,
                root.range,
                "struct pointer section exceeds the schema limit",
            );
        }
        let data_word_count = u16::try_from(top.data_words).unwrap_or(u16::MAX);
        let pointer_count = u16::try_from(top.pointers).unwrap_or(u16::MAX);
        let mut nodes = vec![root.qualified_name.clone()];
        nodes.extend(
            descendants
                .iter()
                .filter(|item| {
                    matches!(item.kind, DeclarationKind::Group | DeclarationKind::Union)
                        && !item.is_unnamed_union
                })
                .map(|item| item.qualified_name.clone()),
        );
        let mut group_ids = BTreeMap::new();
        for node_name in nodes {
            let declaration = if node_name == root.qualified_name {
                root
            } else {
                descendants
                    .iter()
                    .find(|item| item.qualified_name == node_name)
                    .copied()
                    .expect("layout node exists")
            };
            let direct = visible_children(module, &node_name);
            let mut fields = Vec::new();
            for child in direct {
                match child.kind {
                    DeclarationKind::Field => {
                        if let Some(field) = compiled_fields.get(&child.qualified_name) {
                            fields.push(field.clone());
                        }
                    }
                    DeclarationKind::Group | DeclarationKind::Union => fields.push(CompiledField {
                        name: child.name.clone(),
                        qualified_name: child.qualified_name.clone(),
                        code_order: visible_code_order(module, child),
                        ordinal: child.id.and_then(|id| match id {
                            DeclarationId::Ordinal(value) => Some(value),
                            DeclarationId::Uid(_) => None,
                        }),
                        discriminant_value: nearest_union(module, child, &root.qualified_name)
                            .and_then(|(union_name, arm)| {
                                unions
                                    .get(&union_name)
                                    .and_then(|layout| layout.discriminant_values.get(&arm))
                                    .copied()
                            }),
                        kind: CompiledFieldKind::Group {
                            qualified_name: child.qualified_name.clone(),
                        },
                    }),
                    _ => {}
                }
            }
            fields.sort_by_key(|field| {
                module
                    .declarations
                    .iter()
                    .find(|item| item.qualified_name == field.qualified_name)
                    .map_or(u16::MAX, |item| activation_ordinal(module, item))
            });
            let union = unions.get(&node_name).or_else(|| {
                module
                    .declarations
                    .iter()
                    .find(|item| {
                        item.parent.as_deref() == Some(node_name.as_str())
                            && item.kind == DeclarationKind::Union
                            && item.is_unnamed_union
                    })
                    .and_then(|item| unions.get(&item.qualified_name))
            });
            let discriminant_count = union.map_or(0usize, |layout| layout.started_arms.len());
            let discriminant_offset = union.and_then(|layout| layout.discriminant_offset);
            if discriminant_count > usize::from(u16::MAX) {
                self.error(module, declaration.range, "union has too many arms");
            }
            let id = if declaration.kind == DeclarationKind::Struct {
                declaration_node_id(module, declaration)
            } else {
                visible_parent(module, declaration).and_then(|parent_name| {
                    let parent_id = if parent_name == &root.qualified_name {
                        declaration_node_id(module, root)
                    } else {
                        group_ids.get(parent_name).copied()
                    }?;
                    let mut siblings = visible_children(module, parent_name);
                    siblings.sort_by_key(|item| activation_ordinal(module, item));
                    let index = siblings
                        .iter()
                        .position(|item| item.qualified_name == declaration.qualified_name)
                        .and_then(|value| u16::try_from(value).ok())?;
                    Some(generate_group_id(parent_id, index))
                })
            };
            if let Some(id) = id {
                group_ids.insert(node_name.clone(), id);
            }
            self.output.push(CompiledStruct {
                module: module.path.clone(),
                qualified_name: node_name,
                id,
                is_group: declaration.kind != DeclarationKind::Struct,
                data_word_count,
                pointer_count,
                preferred_list_encoding: PreferredListEncoding::InlineComposite,
                discriminant_count: discriminant_count.try_into().unwrap_or(u16::MAX),
                discriminant_offset,
                fields,
            });
        }
    }

    fn slot_size(
        &mut self,
        module: &ResolvedModule,
        declaration: &ResolvedDeclaration,
        visiting: &mut BTreeSet<(String, String)>,
    ) -> Option<SlotSize> {
        let Some(expression) = declaration.expression.as_ref() else {
            self.error(module, declaration.range, "field is missing a type");
            return None;
        };
        self.expression_size(expression, visiting).or_else(|| {
            self.error(
                module,
                expression.range(),
                "field type is not layout-compatible",
            );
            None
        })
    }

    fn expression_size(
        &self,
        expression: &Expression,
        visiting: &mut BTreeSet<(String, String)>,
    ) -> Option<SlotSize> {
        match expression {
            Expression::Apply { function, .. } => self.expression_size(function, visiting),
            Expression::Name { path, target, .. } => match target {
                NameTarget::Builtin => builtin_size(path.first()?),
                NameTarget::GenericParameter { .. } => Some(SlotSize::Pointer),
                NameTarget::Declaration {
                    module,
                    qualified_name,
                } => self.declaration_size(module, qualified_name, visiting),
                _ => None,
            },
            Expression::Import {
                target:
                    NameTarget::Declaration {
                        module,
                        qualified_name,
                    },
                ..
            } => self.declaration_size(module, qualified_name, visiting),
            Expression::Member {
                target:
                    NameTarget::Declaration {
                        module,
                        qualified_name,
                    },
                ..
            } => self.declaration_size(module, qualified_name, visiting),
            Expression::Import { .. } => None,
            _ => None,
        }
    }

    fn declaration_size(
        &self,
        module: &str,
        qualified_name: &str,
        visiting: &mut BTreeSet<(String, String)>,
    ) -> Option<SlotSize> {
        let key = (module.to_owned(), qualified_name.to_owned());
        if !visiting.insert(key.clone()) {
            return None;
        }
        let declaration = self
            .program
            .module(module)?
            .declarations
            .iter()
            .find(|item| item.qualified_name == qualified_name)?;
        let result = match declaration.kind {
            DeclarationKind::Enum => Some(SlotSize::Data { log2_bits: 4 }),
            DeclarationKind::Struct | DeclarationKind::Interface => Some(SlotSize::Pointer),
            DeclarationKind::Alias => declaration
                .expression
                .as_ref()
                .and_then(|value| self.expression_size(value, visiting)),
            _ => None,
        };
        visiting.remove(&key);
        result
    }

    fn error(&mut self, module: &ResolvedModule, range: SourceRange, message: &str) {
        self.diagnostics.push(SemanticDiagnostic {
            module: module.path.clone(),
            range,
            message: message.to_owned(),
        });
    }
}

fn builtin_size(name: &str) -> Option<SlotSize> {
    match name {
        "Void" => Some(SlotSize::Void),
        "Bool" => Some(SlotSize::Data { log2_bits: 0 }),
        "Int8" | "UInt8" => Some(SlotSize::Data { log2_bits: 3 }),
        "Int16" | "UInt16" => Some(SlotSize::Data { log2_bits: 4 }),
        "Int32" | "UInt32" | "Float32" => Some(SlotSize::Data { log2_bits: 5 }),
        "Int64" | "UInt64" | "Float64" => Some(SlotSize::Data { log2_bits: 6 }),
        "Text" | "Data" | "List" | "AnyPointer" | "AnyStruct" | "AnyList" | "Capability" => {
            Some(SlotSize::Pointer)
        }
        _ => None,
    }
}

fn is_descendant(declaration: &ResolvedDeclaration, root: &str) -> bool {
    declaration
        .parent
        .as_deref()
        .is_some_and(|parent| parent == root || parent.starts_with(&format!("{root}.")))
}

fn declaration_node_id(module: &ResolvedModule, declaration: &ResolvedDeclaration) -> Option<u64> {
    if let Some(DeclarationId::Uid(id)) = declaration.id {
        return Some(id);
    }
    let parent_id = match declaration.parent.as_deref() {
        None => module.file_id?,
        Some(parent) => {
            let parent = module
                .declarations
                .iter()
                .find(|item| item.qualified_name == parent)?;
            declaration_node_id(module, parent)?
        }
    };
    Some(generate_child_id(parent_id, &declaration.name))
}

fn belongs_to_struct_layout(
    module: &ResolvedModule,
    declaration: &ResolvedDeclaration,
    root: &ResolvedDeclaration,
) -> bool {
    if !is_descendant(declaration, &root.qualified_name) {
        return false;
    }
    let mut parent = declaration.parent.as_deref();
    while let Some(name) = parent {
        if name == root.qualified_name {
            return true;
        }
        let Some(parent_declaration) = module
            .declarations
            .iter()
            .find(|item| item.qualified_name == name)
        else {
            return false;
        };
        if matches!(
            parent_declaration.kind,
            DeclarationKind::Struct
                | DeclarationKind::Enum
                | DeclarationKind::Interface
                | DeclarationKind::Const
                | DeclarationKind::Annotation
        ) {
            return false;
        }
        parent = parent_declaration.parent.as_deref();
    }
    false
}

fn nearest_union(
    module: &ResolvedModule,
    declaration: &ResolvedDeclaration,
    root: &str,
) -> Option<(String, String)> {
    union_scopes(module, declaration, root).pop()
}

fn union_scopes(
    module: &ResolvedModule,
    declaration: &ResolvedDeclaration,
    root: &str,
) -> Vec<(String, String)> {
    let mut output = Vec::new();
    let mut child = declaration.qualified_name.as_str();
    let mut parent = declaration.parent.as_deref();
    while let Some(parent_name) = parent {
        let Some(parent_declaration) = module
            .declarations
            .iter()
            .find(|item| item.qualified_name == parent_name)
        else {
            break;
        };
        if parent_declaration.kind == DeclarationKind::Union {
            output.push((parent_name.to_owned(), child.to_owned()));
        }
        if parent_name == root {
            break;
        }
        child = parent_name;
        parent = parent_declaration.parent.as_deref();
    }
    output.reverse();
    output
}

fn visible_children<'a>(module: &'a ResolvedModule, parent: &str) -> Vec<&'a ResolvedDeclaration> {
    let mut output = Vec::new();
    for declaration in module
        .declarations
        .iter()
        .filter(|item| item.parent.as_deref() == Some(parent))
        .filter(|item| {
            matches!(
                item.kind,
                DeclarationKind::Field | DeclarationKind::Group | DeclarationKind::Union
            )
        })
    {
        if declaration.kind == DeclarationKind::Union && declaration.is_unnamed_union {
            output.extend(visible_children(module, &declaration.qualified_name));
        } else {
            output.push(declaration);
        }
    }
    output
}

fn visible_parent<'a>(
    module: &'a ResolvedModule,
    declaration: &'a ResolvedDeclaration,
) -> Option<&'a String> {
    let mut parent = declaration.parent.as_deref();
    while let Some(parent_name) = parent {
        let Some(parent_declaration) = module
            .declarations
            .iter()
            .find(|item| item.qualified_name == parent_name)
        else {
            break;
        };
        if parent_declaration.kind != DeclarationKind::Union || !parent_declaration.is_unnamed_union
        {
            break;
        }
        parent = parent_declaration.parent.as_deref();
    }
    parent.and_then(|name| {
        module
            .declarations
            .iter()
            .find(|item| item.qualified_name == name)
            .map(|item| &item.qualified_name)
    })
}

fn visible_code_order(module: &ResolvedModule, declaration: &ResolvedDeclaration) -> u16 {
    let Some(parent) = visible_parent(module, declaration) else {
        return u16::MAX;
    };
    visible_children(module, parent)
        .iter()
        .position(|item| item.qualified_name == declaration.qualified_name)
        .and_then(|index| u16::try_from(index).ok())
        .unwrap_or(u16::MAX)
}

fn activation_ordinal(module: &ResolvedModule, declaration: &ResolvedDeclaration) -> u16 {
    if let Some(DeclarationId::Ordinal(value)) = declaration.id {
        return value;
    }
    let prefix = format!("{}.", declaration.qualified_name);
    module
        .declarations
        .iter()
        .filter(|item| item.qualified_name.starts_with(&prefix))
        .filter_map(|item| match item.id {
            Some(DeclarationId::Ordinal(value)) => Some(value),
            Some(DeclarationId::Uid(_)) | None => None,
        })
        .min()
        .unwrap_or(u16::MAX)
}

fn generate_group_id(parent_id: u64, group_index: u16) -> u64 {
    let mut input = [0u8; 10];
    input[..8].copy_from_slice(&parent_id.to_le_bytes());
    input[8..].copy_from_slice(&group_index.to_le_bytes());
    digest_id(&input)
}

/// Derives the stable ID for an implicitly-IDed nested declaration.
pub fn generate_child_id(parent_id: u64, child_name: &str) -> u64 {
    let mut input = parent_id.to_le_bytes().to_vec();
    input.extend_from_slice(child_name.as_bytes());
    digest_id(&input)
}

/// Derives the stable detached parameter or result struct ID for a method.
pub fn generate_method_params_id(parent_id: u64, method_ordinal: u16, is_results: bool) -> u64 {
    let mut input = [0u8; 11];
    input[..8].copy_from_slice(&parent_id.to_le_bytes());
    input[8..10].copy_from_slice(&method_ordinal.to_le_bytes());
    input[10] = u8::from(is_results);
    digest_id(&input)
}

fn digest_id(input: &[u8]) -> u64 {
    let digest = md5(input);
    u64::from_be_bytes(digest[..8].try_into().expect("eight digest bytes")) | (1 << 63)
}

fn md5(input: &[u8]) -> [u8; 16] {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, // round 1
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, // round 2
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, // round 3
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, // round 4
    ];
    const TABLE: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let padded_len = (input.len() + 9).div_ceil(64) * 64;
    let mut bytes = vec![0u8; padded_len];
    bytes[..input.len()].copy_from_slice(input);
    bytes[input.len()] = 0x80;
    bytes[padded_len - 8..].copy_from_slice(&bit_len.to_le_bytes());
    let mut state = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in bytes.chunks_exact(64) {
        let mut words = [0u32; 16];
        for (word, bytes) in words.iter_mut().zip(chunk.chunks_exact(4)) {
            *word = u32::from_le_bytes(bytes.try_into().expect("four chunk bytes"));
        }
        let [mut a, mut b, mut c, mut d] = state;
        for index in 0..64 {
            let (function, word) = match index {
                0..=15 => ((b & c) | (!b & d), index),
                16..=31 => ((d & b) | (!d & c), (5 * index + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * index + 5) % 16),
                _ => (c ^ (b | !d), (7 * index) % 16),
            };
            let next = b.wrapping_add(
                a.wrapping_add(function)
                    .wrapping_add(TABLE[index])
                    .wrapping_add(words[word])
                    .rotate_left(SHIFTS[index]),
            );
            (a, b, c, d) = (d, next, b, c);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }
    let mut output = [0u8; 16];
    for (bytes, word) in output.chunks_exact_mut(4).zip(state) {
        bytes.copy_from_slice(&word.to_le_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{ModuleSources, ResolveLimits};
    use capnp_schema::{
        CompiledSchema, ElementSize, FieldKind, LoadLimits, Node, NodeKind, Ordinal,
    };

    macro_rules! source {
        ($name:literal) => {
            include_str!(concat!("../../../conformance/schemas/", $name, ".capnp"))
        };
    }

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

    fn compile(name: &str, source: &str) -> CompiledLayouts {
        let path = format!("/{name}.capnp");
        let mut sources = ModuleSources::default();
        sources.insert_explicit(&path, source);
        let program = sources.resolve(&path, ResolveLimits::default());
        assert!(program.is_valid(), "{program:#?}");
        program.compile_layouts()
    }

    fn oracle_node_for_layout<'a>(schema: &'a CompiledSchema, layout: &CompiledStruct) -> &'a Node {
        if let Some(node) = layout.id.and_then(|id| schema.node(id)) {
            return node;
        }
        let mut names = layout.qualified_name.split('.');
        let root_name = names.next().expect("root name");
        let root = schema
            .nodes()
            .iter()
            .find(|node| node.short_name() == Some(root_name));
        let mut node = root.expect("oracle root struct");
        for name in names {
            let NodeKind::Struct(structure) = &node.kind else {
                assert!(matches!(node.kind, NodeKind::Struct(_)));
                return node;
            };
            let type_id = match structure.field(name).map(|field| &field.kind) {
                Some(FieldKind::Group { type_id }) => *type_id,
                _ => {
                    assert!(
                        matches!(
                            structure.field(name).map(|field| &field.kind),
                            Some(FieldKind::Group { .. })
                        ),
                        "missing group path component `{name}` for {layout:#?} in {node:#?}"
                    );
                    return node;
                }
            };
            node = schema.node(type_id).expect("oracle group node");
        }
        node
    }

    fn assert_matches_oracle(layouts: &CompiledLayouts, request: &[u8]) {
        assert!(layouts.is_valid(), "{layouts:#?}");
        let schema = CompiledSchema::from_code_generator_request(request, LoadLimits::default())
            .expect("pinned compiler request");
        for layout in &layouts.structs {
            let node = oracle_node_for_layout(&schema, layout);
            let NodeKind::Struct(oracle) = &node.kind else {
                assert!(matches!(node.kind, NodeKind::Struct(_)));
                continue;
            };
            assert_eq!(layout.id, Some(node.id), "{layout:#?}");
            assert_eq!(
                layout.data_word_count, oracle.data_word_count,
                "{layout:#?}"
            );
            assert_eq!(layout.pointer_count, oracle.pointer_count, "{layout:#?}");
            assert_eq!(layout.is_group, oracle.is_group, "{layout:#?}");
            assert_eq!(oracle.preferred_list_encoding, ElementSize::InlineComposite);
            assert_eq!(
                layout.discriminant_count, oracle.discriminant_count,
                "{layout:#?}"
            );
            assert_eq!(
                layout.discriminant_offset.unwrap_or(0),
                oracle.discriminant_offset,
                "{layout:#?}"
            );
            assert_eq!(layout.fields.len(), oracle.fields.len(), "{layout:#?}");
            for field in &layout.fields {
                let oracle_field = oracle.field(&field.name).expect("oracle field by name");
                assert_eq!(field.code_order, oracle_field.code_order, "{field:#?}");
                assert_eq!(
                    field.ordinal,
                    match oracle_field.ordinal {
                        Ordinal::Implicit => None,
                        Ordinal::Explicit(value) => Some(value),
                    },
                    "{field:#?}"
                );
                assert_eq!(
                    field.discriminant_value, oracle_field.discriminant_value,
                    "{field:#?}"
                );
                match (&field.kind, &oracle_field.kind) {
                    (
                        CompiledFieldKind::Slot { offset, .. },
                        FieldKind::Slot {
                            offset: oracle_offset,
                            ..
                        },
                    ) => assert_eq!(offset, oracle_offset, "{field:#?}"),
                    (CompiledFieldKind::Group { .. }, FieldKind::Group { .. }) => {}
                    _ => assert!(
                        matches!(
                            (&field.kind, &oracle_field.kind),
                            (CompiledFieldKind::Slot { .. }, FieldKind::Slot { .. })
                                | (CompiledFieldKind::Group { .. }, FieldKind::Group { .. })
                        ),
                        "field kind differs: {field:#?} {oracle_field:#?}"
                    ),
                }
            }
        }
    }

    #[test]
    fn wire_and_evolution_layouts_match_pinned_cpp_compiler() {
        for (name, source, request) in [
            (
                "wire-fixture",
                source!("wire-fixture"),
                request!("wire-fixture").as_slice(),
            ),
            (
                "evolution-v1",
                source!("evolution-v1"),
                request!("evolution-v1").as_slice(),
            ),
            (
                "evolution-v2",
                source!("evolution-v2"),
                request!("evolution-v2").as_slice(),
            ),
            (
                "evolution-v3",
                source!("evolution-v3"),
                request!("evolution-v3").as_slice(),
            ),
        ] {
            assert_matches_oracle(&compile(name, source), request);
        }
    }

    #[test]
    fn group_id_matches_the_pinned_type_id_vector() {
        assert_eq!(
            generate_child_id(0xa93f_c509_624c_72d9, "Node"),
            0xe682_ab4c_f923_a417
        );
        assert_eq!(
            generate_group_id(0xe682_ab4c_f923_a417, 7),
            0x9ea0_b19b_37fb_4435
        );
        assert_eq!(
            generate_method_params_id(0x88eb_12a0_e0af_92b2, 0, false),
            0xb874_edc0_d559_b391
        );
        assert_eq!(
            generate_method_params_id(0x88eb_12a0_e0af_92b2, 0, true),
            0xb04f_cadd_ab71_4ba4
        );
    }

    #[test]
    fn appended_fields_preserve_every_existing_offset() {
        let versions = [
            compile("evolution-v1", source!("evolution-v1")),
            compile("evolution-v2", source!("evolution-v2")),
            compile("evolution-v3", source!("evolution-v3")),
        ];
        let offset_map = |layouts: &CompiledLayouts| {
            layouts
                .structs
                .iter()
                .filter(|structure| {
                    structure.qualified_name == "Record"
                        || structure.qualified_name.starts_with("Record.")
                })
                .flat_map(|structure| &structure.fields)
                .filter_map(|field| match field.kind {
                    CompiledFieldKind::Slot { offset, .. } => Some((field.ordinal?, offset)),
                    CompiledFieldKind::Group { .. } => None,
                })
                .collect::<BTreeMap<_, _>>()
        };
        let first = offset_map(&versions[0]);
        let second = offset_map(&versions[1]);
        let third = offset_map(&versions[2]);
        for (ordinal, offset) in first {
            assert_eq!(second.get(&ordinal), Some(&offset));
            assert_eq!(third.get(&ordinal), Some(&offset));
        }
        for (ordinal, offset) in second {
            assert_eq!(third.get(&ordinal), Some(&offset));
        }
    }

    #[test]
    fn padding_is_reused_at_the_smallest_power_of_two_hole() {
        let layouts = compile(
            "padding",
            r#"
                @0x8000000000000100;
                struct Padding @0x8000000000000101 {
                    first @0 :UInt32;
                    full @1 :UInt64;
                    reused @2 :UInt32;
                    small @3 :UInt16;
                    bit @4 :Bool;
                }
            "#,
        );
        let structure = layouts
            .structure("/padding.capnp", "Padding")
            .expect("padding layout");
        assert_eq!(structure.data_word_count, 3);
        let offsets = structure
            .fields
            .iter()
            .map(|field| match field.kind {
                CompiledFieldKind::Slot { offset, .. } => (field.name.as_str(), offset),
                CompiledFieldKind::Group { .. } => (field.name.as_str(), u32::MAX),
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(offsets["first"], 0);
        assert_eq!(offsets["full"], 1);
        assert_eq!(offsets["reused"], 1);
        assert_eq!(offsets["small"], 8);
        assert_eq!(offsets["bit"], 144);
    }

    #[test]
    fn missing_and_nonsequential_ordinals_are_rejected() {
        let layouts = compile(
            "invalid-layout",
            r#"
                @0x8000000000000110;
                struct Invalid @0x8000000000000111 {
                    first @0 :UInt32;
                    skipped @2 :UInt32;
                    empty :group {}
                    lonely :union { only @3 :Void; }
                }
            "#,
        );
        assert!(
            layouts
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message
                    == "field ordinals must be sequential from zero")
        );
        assert!(
            layouts
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "group must have at least one member")
        );
        assert!(
            layouts
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message == "union must have at least two members")
        );
    }

    #[test]
    fn explicit_union_ordinal_allocates_the_discriminant_in_order() {
        let layouts = compile(
            "union-ordinal",
            r#"
                @0x8000000000000120;
                struct Retro @0x8000000000000121 {
                    before @0 :UInt32;
                    choice @1 :union {
                        number @2 :UInt64;
                        text @3 :Text;
                    }
                }
            "#,
        );
        assert!(layouts.is_valid(), "{layouts:#?}");
        let union = layouts
            .structure("/union-ordinal.capnp", "Retro.choice")
            .expect("named union layout");
        assert_eq!(union.discriminant_offset, Some(2));
        assert_eq!(union.discriminant_count, 2);
        let root = layouts
            .structure("/union-ordinal.capnp", "Retro")
            .expect("root layout");
        assert_eq!(root.data_word_count, 2);
        assert_eq!(root.pointer_count, 1);

        let late = compile(
            "late-union-ordinal",
            r#"
                @0x8000000000000130;
                struct Late @0x8000000000000131 {
                    choice @2 :union {
                        first @0 :UInt32;
                        second @1 :UInt32;
                    }
                }
            "#,
        );
        assert!(
            late.diagnostics.iter().any(|diagnostic| diagnostic.message
                == "union ordinal may follow at most one arm ordinal")
        );
    }

    #[test]
    fn unnamed_union_fields_are_flattened_into_the_parent() {
        let layouts = compile(
            "unnamed-union",
            r#"
                @0x8000000000000140;
                struct Flat @0x8000000000000141 {
                    before @0 :UInt32;
                    union {
                        small @1 :UInt16;
                        large @2 :UInt64;
                    }
                    after @3 :UInt32;
                }
            "#,
        );
        assert!(layouts.is_valid(), "{layouts:#?}");
        assert_eq!(layouts.structs.len(), 1, "{layouts:#?}");
        let root = layouts
            .structure("/unnamed-union.capnp", "Flat")
            .expect("flat root");
        assert_eq!(root.discriminant_count, 2);
        assert_eq!(root.discriminant_offset, Some(3));
        assert_eq!(
            root.fields
                .iter()
                .map(|field| (field.name.as_str(), field.code_order))
                .collect::<Vec<_>>(),
            [("before", 0), ("small", 1), ("large", 2), ("after", 3)]
        );
        assert_eq!(root.fields[1].discriminant_value, Some(0));
        assert_eq!(root.fields[2].discriminant_value, Some(1));
    }

    #[test]
    fn union_group_arms_share_lanes_without_tagging_nested_fields() {
        let layouts = compile(
            "union-groups",
            r#"
                @0x8000000000000150;
                struct GroupArms @0x8000000000000151 {
                    choice :union {
                        first :group {
                            a @0 :UInt32;
                            b @2 :UInt16;
                        }
                        second :group {
                            x @1 :UInt64;
                            y @3 :Text;
                        }
                    }
                }
            "#,
        );
        assert!(layouts.is_valid(), "{layouts:#?}");
        let union = layouts
            .structure("/union-groups.capnp", "GroupArms.choice")
            .expect("union layout");
        assert_eq!(union.discriminant_count, 2);
        assert_eq!(union.fields[0].discriminant_value, Some(0));
        assert_eq!(union.fields[1].discriminant_value, Some(1));
        for group_name in ["GroupArms.choice.first", "GroupArms.choice.second"] {
            let group = layouts
                .structure("/union-groups.capnp", group_name)
                .expect("arm group");
            assert!(
                group
                    .fields
                    .iter()
                    .all(|field| field.discriminant_value.is_none())
            );
        }
        let first = layouts
            .structure("/union-groups.capnp", "GroupArms.choice.first")
            .expect("first arm");
        assert!(matches!(
            first.fields[1].kind,
            CompiledFieldKind::Slot { offset: 4, .. }
        ));
    }
}
