mod builtins;
mod c;
mod lua;
mod rust;
mod sfc;
mod template;
mod ts;
mod utils;
mod walk_core;
mod workspace;

pub use builtins::{global as global_builtin_registry, standard as standard_builtin_registry};
pub use c::CProvider;
pub use lua::LuaProvider;
pub use rust::RustProvider;
pub use ts::TsProvider;
pub use workspace::WorkspaceProvider;
