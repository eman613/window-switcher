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
mod icon_cache;
mod icon_loader;
mod keyboard;
mod layout;
mod localization;
mod painter;
mod pixels;
mod process_metadata;
mod render_surface;
mod restart;
mod startup;
mod trayicon;
mod window_snapshot;
mod window_target;
mod worker;

pub use crate::app::{run, start};
pub use crate::config::{load_config, prepare_log_file, Config, LoadedConfig};
