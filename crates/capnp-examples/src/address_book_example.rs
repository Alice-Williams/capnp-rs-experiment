//! Address-book construction and standard/packed persistence.

use std::io;
use std::sync::Arc;

use capnp_io::{FrameLimits, FrameRead, encode_frame, pack, parse_frame, unpack};
use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};

use crate::addressbook::employment::Which;
use crate::addressbook::{Type_, address_book, person, phone_number};
use crate::{ExampleResult, addressbook_schema};

const MESSAGE_WORD_LIMIT: u32 = 4096;
const MESSAGE_BYTE_LIMIT: usize = 32_768;

/// Both persistence encodings plus their deterministic decoded summaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddressBookRoundTrip {
    pub standard: Vec<u8>,
    pub packed: Vec<u8>,
    pub standard_summary: Vec<String>,
    pub packed_summary: Vec<String>,
}

/// Builds the pinned sample data and verifies standard and packed read-back.
pub fn run() -> ExampleResult<AddressBookRoundTrip> {
    let schema = addressbook_schema()?;
    let message = build_message(&schema)?;
    let segments = (0..message.segment_count())
        .map(|index| {
            u32::try_from(index)
                .ok()
                .and_then(|id| message.segment(id))
                .ok_or_else(|| io::Error::other("owned message segment index overflow"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let standard = encode_frame(&segments, frame_limits())?;
    let packed = pack(&standard, MESSAGE_BYTE_LIMIT)?;
    let standard_summary = decode_summary(&schema, &standard)?;
    let unpacked = unpack(&packed, MESSAGE_BYTE_LIMIT)?;
    let packed_summary = decode_summary(&schema, &unpacked)?;
    if standard_summary != packed_summary {
        return Err(io::Error::other("standard and packed address books differ").into());
    }
    Ok(AddressBookRoundTrip {
        standard,
        packed,
        standard_summary,
        packed_summary,
    })
}

fn build_message(schema: &capnp_schema::CompiledSchema) -> ExampleResult<Arc<OwnedMessage>> {
    let mut arena = ExclusiveArena::new(64, MESSAGE_WORD_LIMIT)?;
    let mut book = address_book::Builder::init_root(schema, &mut arena)?;
    let mut people = book.init_people(2)?;

    let mut alice = person::Builder::from_dynamic(people.struct_element(0)?);
    alice.set_id(123)?;
    alice.set_name("Alice")?;
    alice.set_email("alice@example.com")?;
    let mut phones = alice.init_phones(1)?;
    let mut phone = phone_number::Builder::from_dynamic(phones.struct_element(0)?);
    phone.set_number("555-1212")?;
    phone.set_type_(Type_::Mobile)?;
    alice.employment()?.set_school("MIT")?;

    let mut bob = person::Builder::from_dynamic(people.struct_element(1)?);
    bob.set_id(456)?;
    bob.set_name("Bob")?;
    bob.set_email("bob@example.com")?;
    let mut phones = bob.init_phones(2)?;
    let mut home = phone_number::Builder::from_dynamic(phones.struct_element(0)?);
    home.set_number("555-4567")?;
    home.set_type_(Type_::Home)?;
    let mut work = phone_number::Builder::from_dynamic(phones.struct_element(1)?);
    work.set_number("555-7654")?;
    work.set_type_(Type_::Work)?;
    bob.employment()?.set_unemployed(())?;

    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}

fn decode_summary(
    schema: &Arc<capnp_schema::CompiledSchema>,
    bytes: &[u8],
) -> ExampleResult<Vec<String>> {
    let frame = match parse_frame(bytes, frame_limits())? {
        FrameRead::Message {
            frame,
            remaining: [],
        } => frame,
        FrameRead::Message { .. } => {
            return Err(io::Error::other("address book has trailing bytes").into());
        }
        FrameRead::EndOfInput => return Err(io::Error::other("address book is empty").into()),
    };
    let message = OwnedMessage::new(
        frame
            .segments()
            .iter()
            .map(|segment| Box::<[u8]>::from(segment.bytes()))
            .collect::<Vec<_>>(),
        ReaderLimits::default(),
    )?;
    let book = address_book::Reader::from_root(Arc::clone(schema), message)?;
    let people = book
        .people()?
        .ok_or_else(|| io::Error::other("address book people list is null"))?;
    let mut summary = Vec::new();
    for index in 0..people.len()? {
        let person = people.get(index)?;
        let phones = person
            .phones()?
            .ok_or_else(|| io::Error::other("person phones list is null"))?;
        let mut phone_summary = Vec::new();
        for phone_index in 0..phones.len()? {
            let phone = phones.get(phone_index)?;
            phone_summary.push(format!("{}:{:?}", phone.number()?, phone.type_()?));
        }
        let employment = person.employment()?;
        let work = match employment.which()? {
            Which::Unemployed => "unemployed".to_owned(),
            Which::Employer => format!("employer={}", employment.employer()?),
            Which::School => format!("school={}", employment.school()?),
            Which::SelfEmployed => "self-employed".to_owned(),
            Which::Unrecognized(value) => format!("unknown={value}"),
        };
        summary.push(format!(
            "{}|{}|{}|{}|{}",
            person.id()?,
            person.name()?,
            person.email()?,
            phone_summary.join(","),
            work
        ));
    }
    Ok(summary)
}

const fn frame_limits() -> FrameLimits {
    FrameLimits {
        max_segments: 8,
        max_total_words: MESSAGE_WORD_LIMIT as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_and_packed_address_books_round_trip_identically() -> ExampleResult<()> {
        let round_trip = run()?;
        assert_eq!(round_trip.standard_summary, round_trip.packed_summary);
        assert_eq!(
            round_trip.standard_summary,
            [
                "123|Alice|alice@example.com|555-1212:Mobile|school=MIT",
                "456|Bob|bob@example.com|555-4567:Home,555-7654:Work|unemployed",
            ]
        );
        assert!(round_trip.packed.len() < round_trip.standard.len());
        Ok(())
    }
}
