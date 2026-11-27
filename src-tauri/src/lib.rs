pub mod app;
pub mod core;
pub mod lifecycle;
pub mod logs;
mod platform;
#[cfg(feature = "test-fixtures")]
pub use platform::network_worker as prepare_network_test_processes;
pub mod runtime;
pub mod settings;
pub mod updates;
pub const TARGET: &str = env!("SING_SHELL_TARGET");

pub mod desktop;
pub mod instance;
pub mod login_startup;
pub mod startup;

pub mod activity;
mod private_files;
pub mod profile;
pub mod rulesets;
pub mod session;
