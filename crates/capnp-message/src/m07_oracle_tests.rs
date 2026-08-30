use crate::{
    DataSection, EnumValue, LocalTraversalBudget, MessageSegments, NestingLimit, TraversalBudget,
    WireLocation,
};

const CPP_FRAME: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
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

fn cpp_segment() -> &'static [u8] {
    let segment_words = u32::from_le_bytes(CPP_FRAME[4..8].try_into().expect("frame header"));
    let segment_bytes = usize::try_from(segment_words).expect("segment size fits") * 8;
    &CPP_FRAME[8..8 + segment_bytes]
}

fn root_fixture() -> (
    MessageSegments<'static>,
    crate::StructRef,
    LocalTraversalBudget,
) {
    let segment = cpp_segment();
    let segments = MessageSegments::new(&[segment]).expect("C++ segment is aligned");
    let budget = LocalTraversalBudget::new(1_000);
    let root = segments
        .validate_pointer_with_limits(
            WireLocation {
                segment_id: 0,
                word_offset: 0,
            },
            &budget,
            NestingLimit::new(64),
        )
        .expect("C++ root validates");
    let root = root.pointer.structure().expect("fixture root is a struct");
    (segments, root, budget)
}

fn pointer_location(root: crate::StructRef, index: u32) -> WireLocation {
    let word_offset = root
        .content
        .word_offset
        .checked_add(u32::from(root.data_words))
        .and_then(|offset| offset.checked_add(index))
        .expect("fixture pointer offset fits");
    WireLocation {
        segment_id: root.content.segment_id,
        word_offset,
    }
}

#[test]
fn pinned_cpp_scalars_match_generated_field_offsets_and_default_xor() {
    let (segments, root, _budget) = root_fixture();
    let segment = segments.segment(0).expect("segment zero exists");
    let start = usize::try_from(root.content.word_offset).expect("offset fits") * 8;
    let end = start + usize::from(root.data_words) * 8;
    let data = DataSection::new(&segment[start..end]).expect("root data is word aligned");

    data.read_void();
    assert_eq!(data.read_bool(0, false), Ok(true));
    assert_eq!(data.read_i8(1, 0), Ok(127));
    assert_eq!(data.read_i16(1, 0), Ok(12_345));
    assert_eq!(data.read_i32(1, 0), Ok(123_456_789));
    assert_eq!(data.read_i64(1, 0), Ok(1_234_567_890_123_456_789));
    assert_eq!(data.read_u8(16, 0), Ok(255));
    assert_eq!(data.read_u16(9, 0), Ok(65_535));
    assert_eq!(data.read_u32(5, 0), Ok(4_000_000_000));
    assert_eq!(data.read_u64(3, 0), Ok(18_000_000_000_000_000_000));
    assert_eq!(data.read_f32(8, 0.0), Ok(3.5));
    assert_eq!(data.read_f64(5, 0.0), Ok(1.25e100));
    assert_eq!(
        data.read_enum::<Color>(18, 0),
        Ok(EnumValue::Known(Color::Blue))
    );

    // The fixture's logical value is zero. Its wire bits contain 123456 because
    // Cap'n Proto stores `value XOR schema_default`.
    assert_eq!(data.read_u32(16, 123_456), Ok(0));
    assert!(core::ptr::eq(
        data.as_bytes().as_ptr(),
        segment[start..].as_ptr()
    ));
}

#[test]
fn pinned_cpp_text_and_data_are_borrowed_without_copying() {
    let (segments, root, budget) = root_fixture();
    let segment = segments.segment(0).expect("segment zero exists");

    let text_location = pointer_location(root, 0);
    let text_list = segments
        .validate_pointer(text_location)
        .expect("fixture Text pointer validates")
        .list()
        .expect("fixture Text is a list");
    let text = segments
        .read_text(text_location, &budget, NestingLimit::new(63))
        .expect("fixture Text is valid");
    assert_eq!(
        text.to_str(),
        Ok("Cap'n Proto \"wire\" fixture\nwith UTF-8: λ")
    );
    let text_offset = usize::try_from(text_list.content.word_offset).expect("offset fits") * 8;
    assert!(core::ptr::eq(
        text.as_bytes_with_nul().as_ptr(),
        segment[text_offset..].as_ptr()
    ));

    let data_location = pointer_location(root, 1);
    let data_list = segments
        .validate_pointer(data_location)
        .expect("fixture Data pointer validates")
        .list()
        .expect("fixture Data is a list");
    let data = segments
        .read_data(data_location, &budget, NestingLimit::new(63))
        .expect("fixture Data is valid");
    assert_eq!(data.as_bytes(), &[0, 1, 2, 0x7f, 0x80, 0xfe, 0xff]);
    let data_offset = usize::try_from(data_list.content.word_offset).expect("offset fits") * 8;
    assert!(core::ptr::eq(
        data.as_bytes().as_ptr(),
        segment[data_offset..].as_ptr()
    ));
    assert!(budget.remaining_words() < 1_000);
}
