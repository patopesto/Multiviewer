mod discovery;
mod output;
mod source;

pub use discovery::{Discovery, format_syphon_label};
pub use source::{SyphonSource, SyphonSourceConfig};
pub use output::{SyphonOutput, SyphonOutputConfig};
