use capnp_wire::{ElementSize, WirePointer};

use crate::{
    EnumValue, ListReadError, LocalTraversalBudget, MessageSegments, NestingLimit, StructReader,
    TraversalBudget, WireLocation,
};

const CPP_WIRE_FRAME: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
));
const CPP_V1_FRAME: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/evolution-v1-unpacked.bin"
));
const CPP_V2_FRAME: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/evolution-v2-unpacked.bin"
));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Color {
    Red,
    Green,
    Blue,
}

impl TryFrom<u16> for Color {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Red),
            1 => Ok(Self::Green),
            2 => Ok(Self::Blue),
            _ => Err(()),
        }
    }
}

fn segment(frame: &'static [u8]) -> &'static [u8] {
    let words = u32::from_le_bytes(frame[4..8].try_into().expect("frame header"));
    let bytes = usize::try_from(words).expect("segment length fits") * 8;
    &frame[8..8 + bytes]
}

fn root<'context>(
    segments: &'context MessageSegments<'static>,
    budget: &'context LocalTraversalBudget,
) -> StructReader<'context, 'static, LocalTraversalBudget> {
    segments
        .read_struct(
            WireLocation {
                segment_id: 0,
                word_offset: 0,
            },
            budget,
            NestingLimit::new(64),
        )
        .expect("oracle root validates")
}

#[test]
fn pinned_cpp_fixture_covers_every_primitive_and_enum_list_encoding() {
    let segments = MessageSegments::new(&[segment(CPP_WIRE_FRAME)]).expect("fixture is aligned");
    let budget = LocalTraversalBudget::new(10_000);
    let root = root(&segments, &budget);

    let bools: Vec<_> = root
        .read_list(3, None)
        .expect("bool list")
        .as_primitive::<bool>()
        .expect("bit elements")
        .iter()
        .collect::<Result<_, _>>()
        .expect("bool values");
    assert_eq!(bools, [true, false, true, true, false]);

    macro_rules! values {
        ($pointer:expr, $ty:ty) => {{
            root.read_list($pointer, None)
                .expect("primitive list")
                .as_primitive::<$ty>()
                .expect("compatible element type")
                .iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("primitive values")
        }};
    }

    assert_eq!(values!(4, i8), [-128, -1, 0, 1, 127]);
    assert_eq!(values!(5, i16), [-32_768, -1, 0, 32_767]);
    assert_eq!(values!(6, i32), [i32::MIN, -1, 0, i32::MAX]);
    assert_eq!(values!(7, i64), [i64::MIN, -1, 0, i64::MAX]);
    assert_eq!(values!(8, u8), [0, 1, u8::MAX]);
    assert_eq!(values!(9, u16), [0, 1, u16::MAX]);
    assert_eq!(values!(10, u32), [0, 1, u32::MAX]);
    assert_eq!(values!(11, u64), [0, 1, u64::MAX]);

    let float32 = values!(12, f32);
    assert_eq!(float32[..4], [-0.0, 1.5, f32::INFINITY, f32::NEG_INFINITY]);
    assert!(float32[4].is_nan());
    assert_eq!(values!(13, f64), [-0.0, 2.25, 1e200]);

    let colors: Vec<_> = root
        .read_list(14, None)
        .expect("enum list")
        .as_enum::<Color>()
        .expect("u16 enum elements")
        .iter()
        .collect::<Result<_, _>>()
        .expect("enum values");
    assert_eq!(
        colors,
        [
            EnumValue::Known(Color::Red),
            EnumValue::Known(Color::Green),
            EnumValue::Known(Color::Blue),
        ]
    );
}

#[test]
fn pointer_nested_and_inline_struct_lists_borrow_the_cpp_fixture() {
    let segments = MessageSegments::new(&[segment(CPP_WIRE_FRAME)]).expect("fixture is aligned");
    let budget = LocalTraversalBudget::new(10_000);
    let root = root(&segments, &budget);

    let texts = root
        .read_list(15, None)
        .expect("text list")
        .as_pointers()
        .expect("pointer elements");
    assert_eq!(texts.read_text(0).expect("empty text").to_str(), Ok(""));
    assert_eq!(texts.read_text(1).expect("alpha").to_str(), Ok("alpha"));
    assert_eq!(texts.read_text(2).expect("beta").to_str(), Ok("βeta"));

    let text_structs = root
        .read_list(15, None)
        .expect("text list")
        .as_structs()
        .expect("pointer list upgrades to structs");
    assert_eq!(
        text_structs
            .get(1)
            .expect("second pointer struct")
            .read_text(0, None)
            .expect("first pointer field")
            .to_str(),
        Ok("alpha")
    );

    let blobs = root
        .read_list(16, None)
        .expect("data list")
        .as_pointers()
        .expect("pointer elements");
    assert_eq!(blobs.read_data(0).expect("first blob").as_bytes(), &[0]);
    assert_eq!(
        blobs.read_data(1).expect("second blob").as_bytes(),
        &[0xde, 0xad, 0xbe, 0xef]
    );

    let structs = root
        .read_list(17, None)
        .expect("struct list")
        .as_structs()
        .expect("inline structs");
    assert_eq!(
        structs
            .get(0)
            .expect("first struct")
            .data_section()
            .expect("first data")
            .read_u32(0, 0),
        Ok(1)
    );
    let second = structs.get(1).expect("second struct");
    assert_eq!(
        second.data_section().expect("second data").read_u32(0, 0),
        Ok(2)
    );
    let next = second.read_struct(0, None).expect("nested struct pointer");
    assert_eq!(
        next.data_section().expect("next data").read_u32(0, 0),
        Ok(3)
    );

    let nested = root
        .read_list(18, None)
        .expect("nested lists")
        .as_pointers()
        .expect("list pointers");
    assert_eq!(
        nested
            .get_list(0)
            .expect("first nested list")
            .as_primitive::<u16>()
            .expect("u16 elements")
            .iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("nested values"),
        [0, 1, u16::MAX]
    );

    let empty_structs = root
        .read_list(24, None)
        .expect("empty struct list")
        .as_structs()
        .expect("zero-sized inline structs");
    assert_eq!(empty_structs.len(), 3);
    assert!(
        empty_structs
            .get(2)
            .expect("third empty struct")
            .data_section()
            .expect("empty data")
            .as_bytes()
            .is_empty()
    );
}

#[test]
fn reference_primitive_struct_upgrades_work_in_both_directions() {
    let v1_segments = MessageSegments::new(&[segment(CPP_V1_FRAME)]).expect("v1 is aligned");
    let v1_budget = LocalTraversalBudget::new(1_000);
    let v1_root = root(&v1_segments, &v1_budget);
    let upgraded = v1_root
        .read_list(1, None)
        .expect("v1 primitive values")
        .as_structs()
        .expect("UInt32 list upgrades to structs");
    let upgraded_values: Vec<_> = upgraded
        .iter()
        .map(|element| {
            element?
                .data_section()?
                .read_u32(0, 0)
                .map_err(ListReadError::from)
        })
        .collect::<Result<_, _>>()
        .expect("upgraded struct data");
    assert_eq!(upgraded_values, [0, 1, 42, 65_535, u32::MAX]);

    let v2_segments = MessageSegments::new(&[segment(CPP_V2_FRAME)]).expect("v2 is aligned");
    let v2_budget = LocalTraversalBudget::new(1_000);
    let v2_root = root(&v2_segments, &v2_budget);
    let values = v2_root.read_list(1, None).expect("v2 struct values");
    assert_eq!(
        values
            .as_primitive::<u32>()
            .expect("struct first data field upgrades to UInt32")
            .iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("primitive upgrade values"),
        [7, 42]
    );
    let labels = values
        .as_pointers()
        .expect("struct first pointer field upgrades to pointers");
    assert_eq!(
        labels.read_text(0).expect("first label").to_str(),
        Ok("seven")
    );
    assert_eq!(
        labels.read_text(1).expect("second label").to_str(),
        Ok("forty-two")
    );
}

#[test]
fn incompatible_and_void_list_evolution_is_explicit() {
    let mut void_bytes = vec![0u8; 8];
    WirePointer::new_list(0, ElementSize::Void, 3)
        .expect("void list pointer fits")
        .write_to(&mut void_bytes, 0)
        .expect("void pointer word fits");
    let segments = MessageSegments::new(&[&void_bytes]).expect("segment is aligned");
    let budget = LocalTraversalBudget::new(3);
    let values = segments
        .read_list(
            WireLocation {
                segment_id: 0,
                word_offset: 0,
            },
            &budget,
            NestingLimit::new(2),
        )
        .expect("void list validates")
        .as_primitive::<()>()
        .expect("void elements");
    assert_eq!(
        values.iter().collect::<Result<Vec<_>, _>>(),
        Ok(vec![(), (), ()])
    );

    let fixture = MessageSegments::new(&[segment(CPP_WIRE_FRAME)]).expect("fixture is aligned");
    let budget = LocalTraversalBudget::new(10_000);
    let root = root(&fixture, &budget);
    assert!(matches!(
        root.read_list(3, None).expect("bit list").as_structs(),
        Err(ListReadError::IncompatibleElementSize {
            actual: ElementSize::Bit,
            expected: ElementSize::InlineComposite,
        })
    ));
    assert!(matches!(
        root.read_list(17, None)
            .expect("inline list")
            .as_primitive::<bool>(),
        Err(ListReadError::IncompatibleElementSize {
            actual: ElementSize::InlineComposite,
            expected: ElementSize::Bit,
        })
    ));
    assert!(
        root.read_list(8, None)
            .expect("byte list")
            .as_primitive::<u16>()
            .is_err()
    );

    let null_bytes = [0u8; 8];
    let null_segments = MessageSegments::new(&[&null_bytes]).expect("null segment is aligned");
    let null_budget = LocalTraversalBudget::new(0);
    let null = null_segments
        .read_list(
            WireLocation {
                segment_id: 0,
                word_offset: 0,
            },
            &null_budget,
            NestingLimit::new(0),
        )
        .expect("null list is empty");
    assert!(
        null.as_primitive::<u64>()
            .expect("typed null list")
            .is_empty()
    );
    assert_eq!(null.as_pointers().expect("pointer null list").len(), 0);
    assert_eq!(null.as_structs().expect("struct null list").len(), 0);
}

#[test]
fn indexing_and_iteration_have_identical_list_charge() {
    fn remaining_after(iterate: bool) -> u64 {
        let segments =
            MessageSegments::new(&[segment(CPP_WIRE_FRAME)]).expect("fixture is aligned");
        let budget = LocalTraversalBudget::new(100);
        let root = root(&segments, &budget);
        let values = root
            .read_list(10, None)
            .expect("u32 list")
            .as_primitive::<u32>()
            .expect("u32 elements");
        if iterate {
            assert_eq!(
                values.iter().collect::<Result<Vec<_>, _>>(),
                Ok(vec![0, 1, u32::MAX])
            );
        } else {
            for index in 0..values.len() {
                values.get(index).expect("indexed value");
            }
        }
        budget.remaining_words()
    }

    assert_eq!(remaining_after(false), remaining_after(true));
}
