pub mod claude;
pub mod codex;
pub(crate) mod discovery;
pub mod ipc;
pub mod mcp;
mod process;
pub mod protocol;
pub mod service;
pub mod settings;
pub mod store;
pub use protocol::*;
pub use service::AgentService;
pub use store::{MemoryStore, SqliteStore, TaskStore};

pub use process::dispatch_process_owner;
