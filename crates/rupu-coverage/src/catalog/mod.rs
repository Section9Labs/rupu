pub mod builtin;
pub mod filter;
pub mod flatten;
pub mod mode_selection;
pub mod parse;
pub mod render;
pub mod snapshot;
pub mod types;
pub use builtin::{builtin_names, resolve_builtin};
pub use filter::ConcernFilter;
pub use flatten::{flatten, flatten_with_resolver, FlattenError};
pub use mode_selection::{partition_by_mode, resolve_modes, DEFAULT_FULL_MODE_THRESHOLD};
pub use parse::{parse_template_file, parse_template_str, ParseError};
pub use render::{render_full_mode, render_index_mode, render_prompt_section};
pub use snapshot::{read_snapshot, write_snapshot, SnapshotError};
pub use types::{
    CatalogMode, Concern, ConcernOverride, ConcernsBlock, ConcernsEntry, FlatCatalog,
    IncludeDirective, Severity, Template, TouchStrength,
};
