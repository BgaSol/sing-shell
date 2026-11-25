pub mod app;
pub mod core;
pub mod lifecycle;
pub mod logs;
mod platform;
pub mod runtime;
pub mod settings;
pub mod updates;
pub const TARGET: &str = env!("SING_SHELL_TARGET");

pub mod desktop;
pub mod instance;
pub mod login_startup;
pub mod startup;
