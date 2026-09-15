pub mod utils;
#[macro_use]
pub mod macros;
#[macro_use]
extern crate log;

mod app;
mod badge;
mod config;
mod diagnostics;
mod foreground;
mod keyboard;
mod localization;
mod painter;
mod restart;
mod startup;
mod trayicon;
mod window_target;

pub use crate::app::{run, start};
pub use crate::config::{load_config, prepare_log_file, Config, LoadedConfig};
