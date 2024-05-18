use winnow::{
	binary,
	combinator::{alt, fail},
	token::take,
	PResult, Parser,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Rpc<'data> {
	pub command: Command,
	pub data: &'data [u8],
}

impl<'a> Rpc<'a> {
	fn inner_parse_data(input: &mut &'a [u8]) -> PResult<(&'a [u8], u8)> {
		let length = binary::u8(input)?;
		let data = take(length).parse_next(input)?;
		let checksum = binary::u8(input)?;
		Ok((data, checksum))
	}

	fn parse_next(input: &mut &'a [u8]) -> PResult<Self> {
		let command = Command::parse(input)?;
		let data = Self::inner_parse_data
			.verify_map(|(data, checksum)| {
				if checksum == data.iter().fold(0, |acc, &x| acc.wrapping_add(x)) {
					Some(data)
				} else {
					None
				}
			})
			.parse_next(input)?;

		Ok(Self { command, data })
	}

	pub fn parse(input: &'a [u8]) -> Result<Self, String> {
		Self::parse_next.parse(input).map_err(|e| e.to_string())
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
	SendWifiSettings = 0x01,
	Identify = 0x02,
}

impl Command {
	fn parse(input: &mut &[u8]) -> PResult<Self> {
		alt((
			|input: &mut &[u8]| 0x01.parse_next(input).map(|_| Self::SendWifiSettings),
			|input: &mut &[u8]| 0x02.parse_next(input).map(|_| Self::Identify),
			fail,
		))
		.parse_next(input)
	}
}
