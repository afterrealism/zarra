use std::io::{self, Write};

use clap::Args as ClapArgs;
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Serialize, Serializer};
use zarra::{Error, Result, Window};

use crate::source::Source;

/// Arguments for array-relative raster window queries.
#[derive(Debug, ClapArgs)]
pub(crate) struct QueryArgs {
    #[command(flatten)]
    source: Source,
    /// XYZ level to read.
    #[arg(long)]
    level: u8,
    /// Array-relative window origin X.
    #[arg(long, allow_negative_numbers = true)]
    x: i64,
    /// Array-relative window origin Y.
    #[arg(long, allow_negative_numbers = true)]
    y: i64,
    /// Window width in pixels.
    #[arg(long)]
    width: u32,
    /// Window height in pixels.
    #[arg(long)]
    height: u32,
    /// Ordered comma-separated band indices; repeats are allowed.
    #[arg(long)]
    bands: Option<String>,
}

struct JsonWindow<'a> {
    level: u8,
    x: i64,
    y: i64,
    window: &'a Window,
}

impl Serialize for JsonWindow<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Window", 7)?;
        state.serialize_field("level", &self.level)?;
        state.serialize_field("x", &self.x)?;
        state.serialize_field("y", &self.y)?;
        state.serialize_field("width", &self.window.width)?;
        state.serialize_field("height", &self.window.height)?;
        state.serialize_field("bands", &self.window.bands)?;
        state.serialize_field("values", &JsonValues(&self.window.values))?;
        state.end()
    }
}

struct JsonValues<'a>(&'a [f64]);

impl Serialize for JsonValues<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for value in self.0 {
            if value.is_nan() {
                sequence.serialize_element("NaN")?;
            } else if *value == f64::INFINITY {
                sequence.serialize_element("Infinity")?;
            } else if *value == f64::NEG_INFINITY {
                sequence.serialize_element("-Infinity")?;
            } else {
                sequence.serialize_element(value)?;
            }
        }
        sequence.end()
    }
}

pub(crate) fn run(arguments: QueryArgs) -> Result<()> {
    let mut store = arguments.source.open()?;
    let window = match &arguments.bands {
        Some(text) => {
            let bands = zarra::parse_band_list(text)?;
            store.read_bands(
                arguments.level,
                arguments.x,
                arguments.y,
                arguments.width,
                arguments.height,
                &bands,
            )?
        }
        None => store.read_window(
            arguments.level,
            arguments.x,
            arguments.y,
            arguments.width,
            arguments.height,
        )?,
    };
    let json = JsonWindow {
        level: arguments.level,
        x: arguments.x,
        y: arguments.y,
        window: &window,
    };
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer_pretty(&mut stdout, &json)
        .map_err(|error| Error::Format(format!("cannot stream query JSON: {error}")))?;
    stdout.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::JsonValues;

    #[test]
    fn serializes_nonfinite_values_as_exact_json_strings() -> Result<(), Box<dyn std::error::Error>>
    {
        let values = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.5];
        assert_eq!(
            serde_json::to_string(&JsonValues(&values))?,
            r#"["NaN","Infinity","-Infinity",1.5]"#
        );
        Ok(())
    }
}
