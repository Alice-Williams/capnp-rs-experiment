use crate::{
    EnumValue, LocalTraversalBudget, MessageSegments, NestingLimit, StructReader, TraversalBudget,
    UnionDiscriminant, WireLocation,
};

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
enum V1State {
    Unknown,
    Active,
}

impl TryFrom<u16> for V1State {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Active),
            _ => Err(()),
        }
    }
}

fn segment(frame: &'static [u8]) -> &'static [u8] {
    assert_eq!(&frame[..4], &[0, 0, 0, 0]);
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
fn v2_schema_view_of_v1_message_defaults_absent_sections() {
    let bytes = segment(CPP_V1_FRAME);
    let segments = MessageSegments::new(&[bytes]).expect("oracle segment is aligned");
    let budget = LocalTraversalBudget::new(1_000);
    let root = root(&segments, &budget);
    let data = root.data_section().expect("v1 data is present");

    assert_eq!(root.reference().map(|value| value.data_words), Some(1));
    assert_eq!(root.reference().map(|value| value.pointer_count), Some(2));
    assert_eq!(data.read_u32(0, 0), Ok(u32::MAX));
    assert_eq!(
        data.read_enum::<V1State>(2, 0),
        Ok(EnumValue::Known(V1State::Active))
    );
    assert_eq!(
        root.read_text(0, None)
            .expect("v1 name is readable")
            .to_str(),
        Ok("written with evolution-v1")
    );

    // Fields introduced by v2 are outside the v1 sections and therefore use
    // zero/default semantics rather than indexing past the old message.
    assert_eq!(
        root.read_text(2, None)
            .expect("absent email is empty")
            .as_bytes(),
        b""
    );
    assert_eq!(root.union_discriminant(3), Ok(UnionDiscriminant(0)));
    let audit = root.group();
    assert_eq!(
        audit
            .data_section()
            .expect("same group data")
            .read_u64(1, 0),
        Ok(0)
    );
    assert_eq!(
        audit
            .data_section()
            .expect("same group data")
            .read_u64(2, 0),
        Ok(0)
    );
}

#[test]
fn v1_schema_view_of_v2_message_reads_old_fields_and_preserves_new_ordinals() {
    let bytes = segment(CPP_V2_FRAME);
    let segments = MessageSegments::new(&[bytes]).expect("oracle segment is aligned");
    let budget = LocalTraversalBudget::new(1_000);
    let root = root(&segments, &budget);
    let data = root.data_section().expect("v2 data is present");

    assert_eq!(root.reference().map(|value| value.data_words), Some(3));
    assert_eq!(root.reference().map(|value| value.pointer_count), Some(4));
    assert_eq!(data.read_u32(0, 0), Ok(17));
    assert_eq!(data.read_enum::<V1State>(2, 0), Ok(EnumValue::Unknown(2)));
    assert_eq!(
        root.read_text(0, None)
            .expect("old name field remains readable")
            .to_str(),
        Ok("written with evolution-v2")
    );

    // New v2 group and union storage is still available without changing the
    // parent's coordinates; an old reader can ignore it without data loss.
    assert_eq!(root.union_discriminant(3), Ok(UnionDiscriminant(1)));
    assert_eq!(
        root.read_text(3, None)
            .expect("phone union payload is readable")
            .to_str(),
        Ok("+44 20 7946 0000")
    );
    let audit = root.group();
    assert_eq!(
        audit.data_section().expect("audit data").read_u64(1, 0),
        Ok(123_456_789)
    );
    assert_eq!(
        audit.data_section().expect("audit data").read_u64(2, 0),
        Ok(987_654_321)
    );
    assert!(budget.remaining_words() < 1_000);
}
