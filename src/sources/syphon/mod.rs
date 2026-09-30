pub mod discovery;
pub mod output;
pub mod source;

pub use discovery::{Discovery, format_syphon_label};
#[allow(unused_imports)]
pub use source::{SyphonSource, SyphonSourceConfig};
#[allow(unused_imports)]
pub use output::{SyphonOutput, SyphonOutputConfig};
