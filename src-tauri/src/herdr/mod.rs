pub mod client;
#[path = "../../../native/herdr/process.rs"]
pub mod process;
pub mod protocol;
pub mod types;

pub use client::{AttachHandle, HerdrClient};
pub use process::{HerdrDaemon, HerdrLaunchConfig};
pub use types::{
    CoreEvent, CoreStatus, DisplayFacts, EventsSubscribeResult, ForegroundProcess, IpcError,
    OutputStreamFrame, TerminalAttachParams, TerminalAttachResult, TerminalCreateParams,
    TerminalInfo,
};
