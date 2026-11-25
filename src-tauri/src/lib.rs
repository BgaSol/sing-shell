pub mod app;
pub mod lifecycle;
pub mod logs;
mod platform;
pub mod runtime;
pub mod settings;
pub mod updates;
pub const TARGET: &str = env!("SING_SHELL_TARGET");
