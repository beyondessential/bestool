use std::path::PathBuf;

use leon::Template;
use miette::{bail, Result};

pub(crate) fn inout_files(inoutput: Vec<PathBuf>, files: &Vec<PathBuf>) -> Result<Vec<PathBuf>> {
	Ok(if inoutput.is_empty() {
		files
			.iter()
			.map(|f| {
				let mut ext = f
					.extension()
					.unwrap_or_default()
					.to_string_lossy()
					.to_string();
				ext.push_str(".sig");
				f.with_extension(ext)
			})
			.collect()
	} else if inoutput.len() == 1 {
		let maybe_template = inoutput[0].to_string_lossy();
		let template = Template::parse(&maybe_template)?;
		if template.has_any_of_keys(&["filename", "n"]) {
			files
				.iter()
				.enumerate()
				.map(|(n, f)| {
					template
						.render(&[
							("filename", f.to_string_lossy().as_ref()),
							("num", (n + 1).to_string().as_ref()),
						])
						.map(PathBuf::from)
				})
				.collect::<Result<Vec<_>, _>>()?
		} else if files.len() == 1 {
			inoutput
		} else {
			bail!("a single --output must be a template if signing multiple files");
		}
	} else if inoutput.len() == files.len() {
		inoutput
	} else {
		bail!("output file count does not match input file count");
	})
}
