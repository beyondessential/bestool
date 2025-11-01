use winnow::{
	Parser,
	ascii::{space0, space1},
	combinator::{eof, opt, preceded},
	error::ErrMode,
	token::{literal, take_while},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResultFormat {
	Table,
	Expanded,
	Json,
	JsonLine,
	JsonArray,
	Csv,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResultSubcommand {
	Format {
		index: Option<usize>,
		format: ResultFormat,
	},
	Show {
		index: usize,
	},
	List {
		limit: Option<usize>,
		detail: bool,
	},
	Write {
		index: Option<usize>,
		file: String,
	},
}

pub fn parse(
	input: &mut &str,
) -> winnow::error::Result<super::Metacommand, ErrMode<winnow::error::ContextError>> {
	literal('\\').parse_next(input)?;
	literal("re").parse_next(input)?;

	let cmd: Option<&str> = opt(preceded(
		space1,
		take_while(1.., |c: char| !c.is_whitespace()),
	))
	.parse_next(input)?;

	let res = if let Some(cmd_str) = cmd {
		match cmd_str {
			"format" => {
				let index_str: Option<&str> = opt(preceded(
					space1,
					take_while(1.., |c: char| c.is_ascii_digit()),
				))
				.parse_next(input)?;

				let index = index_str.and_then(|s| s.parse::<usize>().ok());

				let format_str: Option<&str> = opt(preceded(
					space1,
					take_while(1.., |c: char| !c.is_whitespace()),
				))
				.parse_next(input)?;

				space0.parse_next(input)?;
				eof.parse_next(input)?;

				if let Some(format_name) = format_str {
					let format = match format_name {
						"table" => ResultFormat::Table,
						"expanded" => ResultFormat::Expanded,
						"json" => ResultFormat::Json,
						"json-line" => ResultFormat::JsonLine,
						"json-array" => ResultFormat::JsonArray,
						"csv" => ResultFormat::Csv,
						_ => return Ok(super::Metacommand::Help),
					};

					super::Metacommand::Result {
						subcommand: ResultSubcommand::Format { index, format },
					}
				} else {
					return Ok(super::Metacommand::Help);
				}
			}
			"show" => {
				let index_str: Option<&str> = opt(preceded(
					space1,
					take_while(1.., |c: char| c.is_ascii_digit()),
				))
				.parse_next(input)?;

				space0.parse_next(input)?;
				eof.parse_next(input)?;

				if let Some(idx_str) = index_str {
					if let Ok(index) = idx_str.parse::<usize>() {
						super::Metacommand::Result {
							subcommand: ResultSubcommand::Show { index },
						}
					} else {
						return Ok(super::Metacommand::Help);
					}
				} else {
					return Ok(super::Metacommand::Help);
				}
			}
			"list" | "list+" => {
				let detail = cmd_str == "list+";

				let limit_str: Option<&str> = opt(preceded(
					space1,
					take_while(1.., |c: char| c.is_ascii_digit()),
				))
				.parse_next(input)?;

				space0.parse_next(input)?;
				eof.parse_next(input)?;

				let limit = limit_str.and_then(|s| s.parse::<usize>().ok());

				super::Metacommand::Result {
					subcommand: ResultSubcommand::List { limit, detail },
				}
			}
			"write" => {
				let index_str: Option<&str> = opt(preceded(
					space1,
					take_while(1.., |c: char| c.is_ascii_digit()),
				))
				.parse_next(input)?;

				let index = index_str.and_then(|s| s.parse::<usize>().ok());

				let file_path: Option<&str> = opt(preceded(
					space1,
					take_while(1.., |c: char| !c.is_whitespace()),
				))
				.parse_next(input)?;

				space0.parse_next(input)?;
				eof.parse_next(input)?;

				if let Some(file) = file_path {
					super::Metacommand::Result {
						subcommand: ResultSubcommand::Write {
							index,
							file: file.to_string(),
						},
					}
				} else {
					return Ok(super::Metacommand::Help);
				}
			}
			_ => {
				space0.parse_next(input)?;
				eof.parse_next(input)?;
				super::Metacommand::Help
			}
		}
	} else {
		space0.parse_next(input)?;
		eof.parse_next(input)?;
		super::Metacommand::Help
	};

	Ok(res)
}

#[cfg(test)]
mod tests {
	use super::super::*;
	use super::*;

	#[test]
	fn test_parse_re_format_with_index() {
		let result = parse_metacommand(r"\re format 1 json").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Format {
					index: Some(1),
					format: ResultFormat::Json
				}
			})
		));
	}

	#[test]
	fn test_parse_re_format_without_index() {
		let result = parse_metacommand(r"\re format table").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Format {
					index: None,
					format: ResultFormat::Table
				}
			})
		));
	}

	#[test]
	fn test_parse_re_format_all_formats() {
		let formats = vec![
			("table", ResultFormat::Table),
			("expanded", ResultFormat::Expanded),
			("json", ResultFormat::Json),
			("json-line", ResultFormat::JsonLine),
			("json-array", ResultFormat::JsonArray),
			("csv", ResultFormat::Csv),
		];

		for (name, expected) in formats {
			let result = parse_metacommand(&format!(r"\re format {}", name)).unwrap();
			assert!(matches!(
				result,
				Some(Metacommand::Result {
					subcommand: ResultSubcommand::Format {
						index: None,
						format: f
					}
				}) if f == expected
			));
		}
	}

	#[test]
	fn test_parse_re_show() {
		let result = parse_metacommand(r"\re show 5").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show { index: 5 }
			})
		));
	}

	#[test]
	fn test_parse_re_show_no_index() {
		let result = parse_metacommand(r"\re show").unwrap();
		assert!(matches!(result, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_re_list_no_limit() {
		let result = parse_metacommand(r"\re list").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::List {
					limit: None,
					detail: false
				}
			})
		));
	}

	#[test]
	fn test_parse_re_list_with_limit() {
		let result = parse_metacommand(r"\re list 10").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::List {
					limit: Some(10),
					detail: false
				}
			})
		));
	}

	#[test]
	fn test_parse_re_list_plus() {
		let result = parse_metacommand(r"\re list+").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::List {
					limit: None,
					detail: true
				}
			})
		));
	}

	#[test]
	fn test_parse_re_list_plus_with_limit() {
		let result = parse_metacommand(r"\re list+ 5").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::List {
					limit: Some(5),
					detail: true
				}
			})
		));
	}

	#[test]
	fn test_parse_re_write_with_index() {
		let result = parse_metacommand(r"\re write 2 output.json").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Write {
					index: Some(2),
					file
				}
			}) if file == "output.json"
		));
	}

	#[test]
	fn test_parse_re_write_without_index() {
		let result = parse_metacommand(r"\re write results.csv").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Write {
					index: None,
					file
				}
			}) if file == "results.csv"
		));
	}

	#[test]
	fn test_parse_re_write_no_file() {
		let result = parse_metacommand(r"\re write").unwrap();
		assert!(matches!(result, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_re_no_subcommand() {
		let result = parse_metacommand(r"\re").unwrap();
		assert!(matches!(result, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_re_invalid_subcommand() {
		let result = parse_metacommand(r"\re invalid").unwrap();
		assert!(matches!(result, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_re_format_invalid_format() {
		let result = parse_metacommand(r"\re format invalid").unwrap();
		assert!(matches!(result, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_re_with_whitespace() {
		let result = parse_metacommand(r"  \re format 1 json  ").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Format {
					index: Some(1),
					format: ResultFormat::Json
				}
			})
		));
	}
}
