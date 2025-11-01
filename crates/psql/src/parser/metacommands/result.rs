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
	JsonPretty,
	Csv,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResultSubcommand {
	Show {
		n: Option<usize>,
		format: Option<ResultFormat>,
		to: Option<String>,
		cols: Vec<String>,
		limit: Option<usize>,
		offset: Option<usize>,
	},
	List {
		limit: Option<usize>,
		detail: bool,
	},
}

fn parse_parameter_value<'a>(
	input: &mut &'a str,
) -> winnow::error::Result<&'a str, ErrMode<winnow::error::ContextError>> {
	take_while(1.., |c: char| !c.is_whitespace()).parse_next(input)
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
			"show" => {
				let mut n = None;
				let mut format = None;
				let mut to = None;
				let mut cols = Vec::new();
				let mut limit = None;
				let mut offset = None;

				loop {
					// Check if we have at least one space followed by content
					let has_param =
						opt(preceded(space1, parse_parameter_value)).parse_next(input)?;

					let Some(param_or_value) = has_param else {
						// No more parameters
						space0.parse_next(input)?;
						eof.parse_next(input)?;
						break;
					};

					if let Some(value_str) = param_or_value.strip_prefix("n=") {
						n = value_str.parse::<usize>().ok();
					} else if let Some(value_str) = param_or_value.strip_prefix("format=") {
						format = match value_str {
							"table" => Some(ResultFormat::Table),
							"expanded" => Some(ResultFormat::Expanded),
							"json" => Some(ResultFormat::Json),
							"json-pretty" => Some(ResultFormat::JsonPretty),
							"csv" => Some(ResultFormat::Csv),
							_ => return Ok(super::Metacommand::Help),
						};
					} else if let Some(value_str) = param_or_value.strip_prefix("to=") {
						to = Some(value_str.to_string());
					} else if let Some(value_str) = param_or_value.strip_prefix("cols=") {
						cols = value_str.split(',').map(|s| s.to_string()).collect();
					} else if let Some(value_str) = param_or_value.strip_prefix("limit=") {
						limit = value_str.parse::<usize>().ok();
					} else if let Some(value_str) = param_or_value.strip_prefix("offset=") {
						offset = value_str.parse::<usize>().ok();
					} else {
						return Ok(super::Metacommand::Help);
					}
				}

				super::Metacommand::Result {
					subcommand: ResultSubcommand::Show {
						n,
						format,
						to,
						cols,
						limit,
						offset,
					},
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
	fn test_parse_re_show_no_params() {
		let result = parse_metacommand(r"\re show").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: None,
					format: None,
					to: None,
					cols,
					limit: None,
					offset: None,
				}
			}) if cols.is_empty()
		));
	}

	#[test]
	fn test_parse_re_show_with_n() {
		let result = parse_metacommand(r"\re show n=5").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: Some(5),
					format: None,
					to: None,
					cols,
					limit: None,
					offset: None,
				}
			}) if cols.is_empty()
		));
	}

	#[test]
	fn test_parse_re_show_with_format() {
		let result = parse_metacommand(r"\re show format=json").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: None,
					format: Some(ResultFormat::Json),
					to: None,
					cols,
					limit: None,
					offset: None,
				}
			}) if cols.is_empty()
		));
	}

	#[test]
	fn test_parse_re_show_all_formats() {
		let formats = vec![
			("table", ResultFormat::Table),
			("expanded", ResultFormat::Expanded),
			("json", ResultFormat::Json),
			("json-pretty", ResultFormat::JsonPretty),
			("csv", ResultFormat::Csv),
		];

		for (name, expected) in formats {
			let result = parse_metacommand(&format!(r"\re show format={}", name)).unwrap();
			assert!(matches!(
				result,
				Some(Metacommand::Result {
					subcommand: ResultSubcommand::Show {
						n: None,
						format: Some(f),
						to: None,
						cols,
						limit: None,
						offset: None,
					}
				}) if f == expected && cols.is_empty()
			));
		}
	}

	#[test]
	fn test_parse_re_show_with_to() {
		let result = parse_metacommand(r"\re show to=output.json").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: None,
					format: None,
					to: Some(ref path),
					cols,
					limit: None,
					offset: None,
				}
			}) if path == "output.json" && cols.is_empty()
		));
	}

	#[test]
	fn test_parse_re_show_with_cols() {
		let result = parse_metacommand(r"\re show cols=col1,col2,col3").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: None,
					format: None,
					to: None,
					cols,
					limit: None,
					offset: None,
				}
			}) if cols == vec!["col1", "col2", "col3"]
		));
	}

	#[test]
	fn test_parse_re_show_with_limit() {
		let result = parse_metacommand(r"\re show limit=10").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: None,
					format: None,
					to: None,
					cols,
					limit: Some(10),
					offset: None,
				}
			}) if cols.is_empty()
		));
	}

	#[test]
	fn test_parse_re_show_with_offset() {
		let result = parse_metacommand(r"\re show offset=5").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: None,
					format: None,
					to: None,
					cols,
					limit: None,
					offset: Some(5),
				}
			}) if cols.is_empty()
		));
	}

	#[test]
	fn test_parse_re_show_multiple_params_any_order() {
		let result = parse_metacommand(r"\re show format=csv n=3 limit=100 offset=10").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: Some(3),
					format: Some(ResultFormat::Csv),
					to: None,
					cols,
					limit: Some(100),
					offset: Some(10),
				}
			}) if cols.is_empty()
		));
	}

	#[test]
	fn test_parse_re_show_different_order() {
		let result = parse_metacommand(r"\re show limit=50 format=expanded n=2").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: Some(2),
					format: Some(ResultFormat::Expanded),
					to: None,
					cols,
					limit: Some(50),
					offset: None,
				}
			}) if cols.is_empty()
		));
	}

	#[test]
	fn test_parse_re_show_all_params() {
		let result = parse_metacommand(
			r"\re show n=1 format=json-pretty to=/tmp/out.json cols=id,name limit=20 offset=5",
		)
		.unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: Some(1),
					format: Some(ResultFormat::JsonPretty),
					to: Some(ref path),
					cols,
					limit: Some(20),
					offset: Some(5),
				}
			}) if path == "/tmp/out.json" && cols == vec!["id", "name"]
		));
	}

	#[test]
	fn test_parse_re_show_invalid_format() {
		let result = parse_metacommand(r"\re show format=invalid").unwrap();
		assert!(matches!(result, Some(Metacommand::Help)));
	}

	#[test]
	fn test_parse_re_show_invalid_param() {
		let result = parse_metacommand(r"\re show invalid=value").unwrap();
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
	fn test_parse_re_with_whitespace() {
		let result = parse_metacommand(r"  \re show n=1 format=json  ").unwrap();
		assert!(matches!(
			result,
			Some(Metacommand::Result {
				subcommand: ResultSubcommand::Show {
					n: Some(1),
					format: Some(ResultFormat::Json),
					to: None,
					cols,
					limit: None,
					offset: None,
				}
			}) if cols.is_empty()
		));
	}
}
