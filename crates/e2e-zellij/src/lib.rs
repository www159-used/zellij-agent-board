//! Authoring surface for real Zellij E2E scenarios.
//!
//! One integration test file is one case. Cases use [`Zellij`] and
//! [`MockAgent`] with ordinary assertions. Implementation details of PTY
//! sockets and control channels stay behind this crate.

mod agent;
mod oracle;
mod pty;
mod util;
mod zellij;

pub use agent::{process_exists, MockAgent, MockAgentOptions, Snapshot};
pub use zellij::Zellij;
