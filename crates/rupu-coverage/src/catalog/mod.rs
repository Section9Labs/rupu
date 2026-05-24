pub mod builtin;
pub mod flatten;
pub mod parse;
pub mod render;
pub mod snapshot;
pub mod types;
pub use builtin::{builtin_names, resolve_builtin};
pub use flatten::{flatten, flatten_with_resolver, FlattenError};
pub use parse::{parse_template_file, parse_template_str, ParseError};
pub use render::render_full_mode;
pub use snapshot::{read_snapshot, write_snapshot, SnapshotError};
pub use types::{
    Concern, ConcernOverride, ConcernsBlock, ConcernsEntry, FlatCatalog, IncludeDirective,
    Severity, Template, TouchStrength,
};
