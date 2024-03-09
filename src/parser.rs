use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::character::complete::alphanumeric1;
use nom::combinator::recognize;
use nom::multi::{many1_count, separated_list0};
use nom::number::complete::float;
use nom::sequence::separated_pair;
use nom::IResult;

//parses gas id, must be an alphanumeric
fn parse_gas_id(input: &str) -> IResult<&str, &str> {
	recognize(many1_count(alt((alphanumeric1, tag("_")))))(input)
}

//parses moles in floating point form
fn parse_moles(input: &str) -> IResult<&str, f32> {
	float(input)
}

/// Parses gas strings, invalid patterns will be ignored
/// E.g: "o2=2500;plasma=5000;TEMP=370" will return vec![("o2", 2500_f32), ("plasma", 5000_f32), ("TEMP", 370_f32)]
pub(crate) fn parse_gas_string(input: &str) -> IResult<&str, Vec<(&str, f32)>> {
	separated_list0(
		tag(";"),
		separated_pair(parse_gas_id, tag("="), parse_moles),
	)(input)
}

#[test]
fn test_parser() {
	let test_str = "o2=2500;plasma=5000;TEMP=370";
	let result = parse_gas_string(test_str).unwrap();

	assert_eq!(
		result,
		(
			"",
			vec![("o2", 2500_f32), ("plasma", 5000_f32), ("TEMP", 370_f32)]
		)
	);
}
